use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;
use worker::{
    D1Database, D1PreparedStatement,
    send::{SendFuture, SendWrapper},
};

use crate::{Error, Param, Result, Statement, sql::Value};

/// Handle to the application's D1 (SQLite) database, from [`Ctx::db`](crate::Ctx::db).
///
/// Every method returns a `Send` future, so plain axum handlers can await it.
/// Always pass values through [`params!`](crate::params) and `?1, ?2`
/// placeholders, never by formatting them into the SQL string. Rows are
/// deserialized with serde, by column name; SQLite booleans and JSON columns
/// need [`bool_from_sql`](crate::bool_from_sql) and
/// [`json_from_sql`](crate::json_from_sql).
///
/// A failed query is [`Error::Internal`] (500) with the D1 message and the SQL;
/// the details go to the Worker logs (Workers Logs), never to the client.
///
/// # Free plan
///
/// D1 bills rows read (every row a query scans, not only those returned) and
/// rows written. Add indexes for `WHERE` and `ORDER BY` columns, and bound
/// lists with `LIMIT` (see [`Page`](crate::Page)).
///
/// # Examples
///
/// ```no_run
/// use axum::extract::{Path, State};
/// use ocre::{Ctx, OptionExt, Result, params};
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Post {
///     id: i64,
///     title: String,
/// }
///
/// async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<String> {
///     let db = ctx.db()?;
///     let post: Post = db.first("SELECT id, title FROM posts WHERE id = ?1", params![id]).await?.or_404()?;
///     Ok(format!("#{} {}", post.id, post.title))
/// }
/// # let _ = show;
/// ```
pub struct Db {
    inner: SendWrapper<D1Database>,
}

impl Db {
    pub(crate) fn new(db: D1Database) -> Self {
        Self { inner: SendWrapper::new(db) }
    }

    /// Runs a query and returns every row, deserialized into `T`.
    ///
    /// All rows are buffered in memory; bound the query with `LIMIT`.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged with the SQL) when the statement does
    /// not prepare or bind, D1 rejects it, or a row does not deserialize into `T`.
    ///
    /// # Free plan
    ///
    /// Counts every row the query scans as a D1 row read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Page, Result, params};
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Post {
    ///     title: String,
    /// }
    ///
    /// async fn index(State(ctx): State<Ctx>, page: Page) -> Result<String> {
    ///     let sql = "SELECT title FROM posts ORDER BY id DESC LIMIT ?1 OFFSET ?2";
    ///     let posts: Vec<Post> = ctx.db()?.all(sql, params![page.limit, page.offset]).await?;
    ///     Ok(posts.into_iter().map(|p| p.title).collect::<Vec<_>>().join("\n"))
    /// }
    /// # let _ = index;
    /// ```
    pub fn all<'q, T: DeserializeOwned>(
        &self,
        sql: &'q str,
        params: Vec<Param>,
    ) -> impl Future<Output = Result<Vec<T>>> + Send + use<'q, T> {
        let stmt = self.prepare(sql, params);
        SendFuture::new(async move {
            let rows = stmt?.all().await.map_err(|err| query_error(sql, err))?;
            rows.results::<T>().map_err(|err| query_error(sql, err))
        })
    }

    /// Runs a query and returns its first row, if any, deserialized into `T`.
    ///
    /// Use with `INSERT ... RETURNING *` to get the created row back, and with
    /// [`OptionExt::or_404`](crate::OptionExt::or_404) to turn a missing record
    /// into a 404.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged with the SQL) when the statement does
    /// not prepare or bind, D1 rejects it (e.g. a `UNIQUE` or foreign-key
    /// constraint), or the row does not deserialize into `T`.
    ///
    /// # Free plan
    ///
    /// D1 counts the rows the query scans, not only the one returned: add
    /// `LIMIT 1` or look rows up by an indexed column. `INSERT ... RETURNING`
    /// also counts the rows written.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{ApiResult, Created, Ctx, Error, Json, params};
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Deserialize, Serialize)]
    /// struct Post {
    ///     id: i64,
    ///     title: String,
    /// }
    ///
    /// async fn create(State(ctx): State<Ctx>, Json(title): Json<String>) -> ApiResult<Created<Post>> {
    ///     let sql = "INSERT INTO posts (title) VALUES (?1) RETURNING *";
    ///     let post: Option<Post> = ctx.db()?.first(sql, params![title]).await?;
    ///     Ok(Created(post.ok_or_else(|| Error::internal("INSERT returned no row"))?))
    /// }
    /// # let _ = create;
    /// ```
    pub fn first<'q, T: DeserializeOwned>(
        &self,
        sql: &'q str,
        params: Vec<Param>,
    ) -> impl Future<Output = Result<Option<T>>> + Send + use<'q, T> {
        let stmt = self.prepare(sql, params);
        SendFuture::new(async move { stmt?.first::<T>(None).await.map_err(|err| query_error(sql, err)) })
    }

    /// Runs a statement that returns no rows (`INSERT`, `UPDATE`, `DELETE`) and gives the number of rows changed.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged with the SQL) when the statement does
    /// not prepare or bind, or D1 rejects it (constraint violation, syntax
    /// error, missing table: run `ocre migrate`).
    ///
    /// # Free plan
    ///
    /// Counts the rows written, plus the rows scanned to find them (rows read).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::{Path, State};
    /// use ocre::{Ctx, Error, Result, params};
    ///
    /// async fn destroy(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<()> {
    ///     match ctx.db()?.execute("DELETE FROM posts WHERE id = ?1", params![id]).await? {
    ///         0 => Err(Error::NotFound),
    ///         _ => Ok(()),
    ///     }
    /// }
    /// # let _ = destroy;
    /// ```
    pub fn execute<'q>(
        &self,
        sql: &'q str,
        params: Vec<Param>,
    ) -> impl Future<Output = Result<usize>> + Send + use<'q> {
        let stmt = self.prepare(sql, params);
        SendFuture::new(async move {
            let result = stmt?.run().await.map_err(|err| query_error(sql, err))?;
            let meta = result.meta().map_err(|err| query_error(sql, err))?;
            Ok(meta.and_then(|m| m.changes).unwrap_or(0))
        })
    }

    /// Whether a query returns at least one row.
    ///
    /// Write it as `SELECT 1 FROM ... WHERE ... LIMIT 1`, so D1 stops at the
    /// first match; the generated models use it for uniqueness checks ("has
    /// already been taken").
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged with the SQL) when the statement does
    /// not prepare or bind, or D1 rejects it.
    ///
    /// # Free plan
    ///
    /// Counts the rows scanned as rows read: with an index on the `WHERE`
    /// column and `LIMIT 1`, one row.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result, Validator, params};
    ///
    /// async fn check_email(State(ctx): State<Ctx>, email: String) -> Result<()> {
    ///     let taken = ctx.db()?.exists("SELECT 1 FROM users WHERE email = ?1 LIMIT 1", params![&email]).await?;
    ///     Validator::new().check("email", taken, "has already been taken").finish()
    /// }
    /// # let _ = check_email;
    /// ```
    pub fn exists<'q>(&self, sql: &'q str, params: Vec<Param>) -> impl Future<Output = Result<bool>> + Send + use<'q> {
        let stmt = self.prepare(sql, params);
        SendFuture::new(async move {
            let row = stmt?.first::<serde_json::Value>(None).await.map_err(|err| query_error(sql, err))?;
            Ok(row.is_some())
        })
    }

    /// Runs every statement in one transaction and returns the rows changed by each one.
    ///
    /// All statements succeed or none is applied (D1 `batch`). Results are in
    /// the order of `statements`. It is also one round trip to D1 instead of
    /// one per statement.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged with every statement's SQL joined by
    /// `; `) when a statement does not prepare or bind, or when D1 rejects
    /// any of them; nothing is written in that case.
    ///
    /// # Free plan
    ///
    /// Counts the rows read and written by every statement, as if each ran alone.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result, Statement, params};
    ///
    /// async fn transfer(State(ctx): State<Ctx>) -> Result<()> {
    ///     let changed = ctx
    ///         .db()?
    ///         .batch(vec![
    ///             Statement::new("UPDATE accounts SET balance = balance - ?1 WHERE id = ?2", params![10, 1]),
    ///             Statement::new("UPDATE accounts SET balance = balance + ?1 WHERE id = ?2", params![10, 2]),
    ///         ])
    ///         .await?;
    ///     assert_eq!(changed.len(), 2);
    ///     Ok(())
    /// }
    /// # let _ = transfer;
    /// ```
    pub fn batch(&self, statements: Vec<Statement>) -> impl Future<Output = Result<Vec<usize>>> + Send + '_ {
        let sql = statements.iter().map(|s| s.sql.as_str()).collect::<Vec<_>>().join("; ");
        let prepared: Result<Vec<D1PreparedStatement>> =
            statements.into_iter().map(|s| self.prepare(&s.sql, s.params)).collect();
        let db = &self.inner;
        SendFuture::new(async move {
            let results = db.batch(prepared?).await.map_err(|err| query_error(&sql, err))?;
            results
                .iter()
                .map(|result| {
                    Ok(result.meta().map_err(|err| query_error(&sql, err))?.and_then(|m| m.changes).unwrap_or(0))
                })
                .collect()
        })
    }

    fn prepare(&self, sql: &str, params: Vec<Param>) -> Result<D1PreparedStatement> {
        let values: Vec<JsValue> = params.into_iter().map(to_js).collect();
        self.inner.prepare(sql).bind(&values).map_err(|err| query_error(sql, err))
    }
}

fn to_js(param: Param) -> JsValue {
    match param.0 {
        Value::Null => JsValue::NULL,
        Value::Number(n) => JsValue::from_f64(n),
        Value::Text(s) => JsValue::from(s),
    }
}

fn query_error(sql: &str, err: worker::Error) -> Error {
    Error::internal(format!("D1 query failed: {err}. SQL: {sql}"))
}
