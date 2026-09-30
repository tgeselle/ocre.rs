use std::sync::Arc;

use serde::de::DeserializeOwned;
use wasm_bindgen::{JsCast, JsValue};
use worker::{
    D1Database, D1DatabaseSession, D1PreparedStatement,
    js_sys::{Array, Reflect},
    send::{SendFuture, SendWrapper},
    wasm_bindgen_futures::JsFuture,
};

use super::ctx::Memo;
use crate::{
    Error, Param, Result, Statement,
    cache::{is_read_query, query_key},
    sql::Value,
};
use crate::{instrument::Timings, log::Logger};

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
/// # Query cache
///
/// Like Rails' query cache, a `SELECT` run twice with the same parameters
/// during one request (or one job, or one cron run) is answered from memory
/// the second time: no D1 round trip and no rows read. Any other statement
/// ([`execute`](Self::execute), [`batch`](Self::batch), `INSERT ...
/// RETURNING` through [`first`](Self::first)) empties the cache, before and
/// after it runs, so a request always reads its own writes. The cache holds
/// up to 100 results and is emptied when full. Writes by other requests are
/// not seen until the next request; use [`uncached`](Self::uncached) for a
/// query that must hit D1 (polling in a loop, a row another Worker updates).
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
    inner: SendWrapper<Handle>,
    binding: String,
    memo: Option<Arc<Memo>>,
    /// Whether `SELECT`s may be served from `memo` (false after [`Db::uncached`]).
    cached: bool,
    probe: Option<Probe>,
}

/// The database itself, or the request's session on it (read replicas).
#[derive(Clone)]
pub(crate) enum Handle {
    Database(Arc<D1Database>),
    Session(Arc<D1DatabaseSession>),
}

impl Handle {
    fn prepare(&self, sql: &str) -> D1PreparedStatement {
        match self {
            Self::Database(db) => db.prepare(sql),
            Self::Session(session) => session.prepare(sql),
        }
    }

    async fn batch(&self, statements: Vec<D1PreparedStatement>) -> worker::Result<Vec<worker::D1Result>> {
        match self {
            Self::Database(db) => db.batch(statements).await,
            Self::Session(session) => session.batch(statements).await,
        }
    }
}

/// Where a handle reports its statements: the request's logger (a `debug`
/// line per statement) and timings (`Server-Timing`, the development error page).
#[derive(Clone)]
struct Probe {
    log: Logger,
    timings: Timings,
}

impl Db {
    pub(crate) fn new(db: Handle, binding: &str, memo: Option<Arc<Memo>>) -> Self {
        Self { inner: SendWrapper::new(db), binding: binding.to_owned(), memo, cached: true, probe: None }
    }

    /// This handle, timing and logging each statement for the request.
    pub(crate) fn probed(self, log: Logger, timings: Timings) -> Self {
        Self { probe: Some(Probe { log, timings }), ..self }
    }

    /// This handle without the per-request query cache: every query goes to D1.
    ///
    /// Writes through it still empty the cache of the other handles of the
    /// request. Rails' `uncached` block.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result, params};
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Import {
    ///     status: String,
    /// }
    ///
    /// async fn status(State(ctx): State<Ctx>) -> Result<Option<String>> {
    ///     // Another Worker may have changed it since this request's last read.
    ///     let db = ctx.db()?.uncached();
    ///     let row: Option<Import> = db.first("SELECT status FROM imports WHERE id = ?1", params![1]).await?;
    ///     Ok(row.map(|import| import.status))
    /// }
    /// # let _ = status;
    /// ```
    pub fn uncached(self) -> Self {
        Self { cached: false, ..self }
    }

    /// Where a query's rows are remembered: `Some` for a `SELECT` when the
    /// cache is on. A statement that may write empties the cache instead.
    fn cache_slot(&self, sql: &str, params: &[Param]) -> Option<(Arc<Memo>, String)> {
        let memo = self.memo.as_ref()?;
        if !is_read_query(sql) {
            memo.clear_queries();
            memo.wrote(&self.binding);
            return None;
        }
        self.cached.then(|| (Arc::clone(memo), query_key(&self.binding, sql, params)))
    }

    /// The memo to empty once a statement that may write has run.
    fn write_memo(&self, sql: &str) -> Option<Arc<Memo>> {
        self.memo.as_ref().filter(|_| !is_read_query(sql)).map(Arc::clone)
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
        let slot = self.cache_slot(sql, &params);
        let written = self.write_memo(sql);
        let stmt = self.prepare(sql, params);
        SendFuture::new(timed(self.probe.clone(), sql, async move {
            let rows = rows(stmt?, sql, slot).await?;
            if let Some(memo) = written {
                memo.clear_queries();
            }
            rows.iter().map(|row| deserialize(&row, sql)).collect()
        }))
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
        let slot = self.cache_slot(sql, &params);
        let written = self.write_memo(sql);
        let stmt = self.prepare(sql, params);
        SendFuture::new(timed(self.probe.clone(), sql, async move {
            let stmt = stmt?;
            let row = match slot {
                Some(slot) => rows(stmt, sql, Some(slot)).await?.iter().next().map(|row| deserialize(&row, sql)),
                None => {
                    let row = stmt.first::<T>(None).await.map_err(|err| query_error(sql, err))?;
                    if let Some(memo) = written {
                        memo.clear_queries();
                    }
                    return Ok(row);
                }
            };
            row.transpose()
        }))
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
        self.cache_slot(sql, &params);
        let written = self.write_memo(sql);
        let stmt = self.prepare(sql, params);
        SendFuture::new(timed(self.probe.clone(), sql, async move {
            let result = stmt?.run().await.map_err(|err| query_error(sql, err))?;
            if let Some(memo) = written {
                memo.clear_queries();
            }
            let meta = result.meta().map_err(|err| query_error(sql, err))?;
            Ok(meta.and_then(|m| m.changes).unwrap_or(0))
        }))
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
        let slot = self.cache_slot(sql, &params);
        let stmt = self.prepare(sql, params);
        SendFuture::new(timed(self.probe.clone(), sql, async move {
            match slot {
                Some(slot) => Ok(rows(stmt?, sql, Some(slot)).await?.length() > 0),
                None => {
                    let row = stmt?.first::<serde_json::Value>(None).await.map_err(|err| query_error(sql, err))?;
                    Ok(row.is_some())
                }
            }
        }))
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
        let memo = self.memo.clone();
        if let Some(memo) = &memo {
            memo.clear_queries();
            memo.wrote(&self.binding);
        }
        let probe = self.probe.clone();
        SendFuture::new(async move {
            let batch = async { db.batch(prepared?).await.map_err(|err| query_error(&sql, err)) };
            let results = timed(probe, &sql, batch).await;
            if let Some(memo) = memo {
                memo.clear_queries();
            }
            let results = results?;
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

/// Awaits a statement, then records how long it took (Workers' clock
/// advances during I/O, so this is D1's time) and logs it at `debug`:
/// `SQL (1.2 ms) SELECT ...`, like Rails' `Post Load (0.3ms) SELECT ...`.
async fn timed<T>(probe: Option<Probe>, sql: &str, future: impl Future<Output = Result<T>>) -> Result<T> {
    let Some(probe) = probe else { return future.await };
    let started = crate::clock::now_millis();
    let result = future.await;
    let ms = crate::clock::now_millis() - started;
    probe.timings.record(sql, ms);
    if probe.log.enabled(crate::log::Level::Debug) {
        let log = probe.log.with("duration_ms", ms);
        let log = if result.is_err() { log.with("failed", true) } else { log };
        log.debug(format_args!("SQL ({ms} ms) {sql}"));
    }
    result
}

/// The rows of a query, from the request's cache when `slot` holds them.
/// A statement without a slot (uncached, or one that may write) runs as is.
async fn rows(stmt: D1PreparedStatement, sql: &str, slot: Option<(Arc<Memo>, String)>) -> Result<Array> {
    if let Some((memo, key)) = &slot
        && let Some(rows) = memo.query(key)
    {
        return Ok(rows);
    }
    let promise = stmt.inner().all().map_err(|err| query_error(sql, js_error(err)))?;
    let result = JsFuture::from(promise).await.map_err(|err| query_error(sql, js_error(err)))?;
    let results =
        Reflect::get(&result, &JsValue::from_str("results")).map_err(|err| query_error(sql, js_error(err)))?;
    let rows = results.dyn_into::<Array>().unwrap_or_else(|_| Array::new());
    if let Some((memo, key)) = slot {
        memo.remember_query(key, rows.clone());
    }
    Ok(rows)
}

/// A rejected D1 promise as an error naming D1's message (`D1_ERROR: no such table: posts...`).
fn js_error(err: JsValue) -> worker::Error {
    match err.dyn_ref::<worker::js_sys::Error>() {
        Some(err) => worker::Error::RustError(String::from(err.message())),
        None => err.into(),
    }
}

fn deserialize<T: DeserializeOwned>(row: &JsValue, sql: &str) -> Result<T> {
    serde_wasm_bindgen::from_value(row.clone()).map_err(|err| query_error(sql, err.into()))
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
