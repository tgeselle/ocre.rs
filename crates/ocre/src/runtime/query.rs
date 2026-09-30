//! The terminal methods of [`Query`]: they send the built statement to D1.

use serde::{Deserialize, de::DeserializeOwned};

use super::Db;
use crate::{Batches, Error, Page, Paginated, Param, Query, Result};

#[derive(Deserialize)]
struct Count {
    count: i64,
}

#[derive(Deserialize)]
struct Value<V> {
    value: V,
}

#[derive(Deserialize)]
struct Plan {
    detail: String,
}

impl<T: DeserializeOwned> Query<T> {
    /// Runs the query and returns every matching row.
    ///
    /// All rows are buffered in memory: bound the query with
    /// [`limit`](Self::limit) or [`page`](Self::page).
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500, logged with the SQL)
    /// when D1 rejects the statement or a row does not deserialize into `T`.
    ///
    /// # Free plan
    ///
    /// Counts every row D1 scans as a row read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, Query, Result};
    /// # #[derive(serde::Deserialize)] struct Post { id: i64 }
    /// async fn drafts(ctx: &Ctx) -> Result<Vec<Post>> {
    ///     Query::table("posts").eq("published", false).order_desc("id").limit(50).all(&ctx.db()?).await
    /// }
    /// ```
    pub async fn all(&self, db: &Db) -> Result<Vec<T>> {
        let stmt = self.to_statement();
        db.all(&stmt.sql, stmt.params).await
    }

    /// Runs the query with `LIMIT 1` and returns the first row, if any
    /// (Rails' `first`, `take` and `find_by`).
    ///
    /// Rows come in the query's order; add one ([`order_asc`](Self::order_asc))
    /// for a predictable row.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the
    /// statement or the row does not deserialize into `T`.
    ///
    /// # Free plan
    ///
    /// Reads the rows D1 scans before the first match: one with an index on
    /// the filtered column.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, OptionExt, Query, Result};
    /// # #[derive(serde::Deserialize)] struct Post { id: i64 }
    /// async fn by_slug(ctx: &Ctx, slug: &str) -> Result<Post> {
    ///     Query::table("posts").eq("slug", slug).first(&ctx.db()?).await?.or_404()
    /// }
    /// ```
    pub async fn first(&self, db: &Db) -> Result<Option<T>> {
        let stmt = self.clone().limit(1).to_statement();
        db.first(&stmt.sql, stmt.params).await
    }

    /// Runs the query and its [`count_statement`](Self::count_statement) for
    /// `page`: the rows plus the total for pagination links.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects
    /// either statement or a row does not deserialize.
    ///
    /// # Free plan
    ///
    /// Two queries: the count reads every matching row (see [`Paginated`]).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{ApiResult, Ctx, Json, Page, Paginated, Query};
    ///
    /// #[derive(serde::Deserialize, serde::Serialize)]
    /// struct Post {
    ///     id: i64,
    ///     title: String,
    /// }
    ///
    /// async fn index(State(ctx): State<Ctx>, page: Page) -> ApiResult<Json<Paginated<Post>>> {
    ///     let posts = Query::table("posts").order_desc("id").paginate(&ctx.db()?, page).await?;
    ///     Ok(Json(posts))
    /// }
    /// # let _ = index;
    /// ```
    pub async fn paginate(&self, db: &Db, page: Page) -> Result<Paginated<T>> {
        let total = self.count(db).await?;
        let items = self.clone().page(page).all(db).await?;
        Ok(Paginated { items, total, limit: page.limit, offset: page.offset })
    }

    /// Returns the first matching row, or runs `create` when there is none
    /// (Rails' `find_or_create_by`; with a `New...` value built in memory
    /// instead of saved, `find_or_initialize_by`).
    ///
    /// Two requests can both miss and both create: back the lookup with a
    /// `UNIQUE` index and use [`create_or_first`](Self::create_or_first)
    /// when duplicates must not happen.
    ///
    /// # Errors
    ///
    /// The lookup's errors, or those of `create` (e.g. [`Error::Invalid`]).
    ///
    /// # Free plan
    ///
    /// One query when the row exists, plus `create`'s otherwise.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::{Ctx, Error, Query, Result, params};
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Tag {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// async fn tag_named(ctx: &Ctx, name: &str) -> Result<Tag> {
    ///     let db = ctx.db()?;
    ///     Query::table("tags")
    ///         .eq("name", name)
    ///         .first_or_create(&db, || async {
    ///             let sql = "INSERT INTO tags (name) VALUES (?1) RETURNING *";
    ///             db.first(sql, params![name]).await?.ok_or_else(|| Error::internal("no row returned"))
    ///         })
    ///         .await
    /// }
    /// ```
    pub async fn first_or_create<F, Fut>(&self, db: &Db, create: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        match self.first(db).await? {
            Some(row) => Ok(row),
            None => create().await,
        }
    }

    /// Runs `create` first and, when it fails because the value is already
    /// taken, returns the existing row instead (Rails' `create_or_find_by`).
    ///
    /// Safe against two requests racing: the table's `UNIQUE` index decides
    /// and the loser reads the winner's row. "Taken" means
    /// [`Error::is_taken`]: a generated model's "has already been taken"
    /// validation, or D1's `UNIQUE constraint failed`. The query must match
    /// the conflicting row (usually `eq` on the unique column).
    ///
    /// # Errors
    ///
    /// Any other error of `create`; [`Error::NotFound`] when the conflicting
    /// row cannot be found by this query.
    ///
    /// # Free plan
    ///
    /// `create`'s queries, plus one lookup after a conflict.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::{Ctx, Error, Query, Result, params};
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Subscriber {
    ///     id: i64,
    ///     email: String,
    /// }
    ///
    /// // `email` has a UNIQUE index (`email:string^`).
    /// async fn subscribe(ctx: &Ctx, email: &str) -> Result<Subscriber> {
    ///     let db = ctx.db()?;
    ///     Query::table("subscribers")
    ///         .eq("email", email)
    ///         .create_or_first(&db, || async {
    ///             let sql = "INSERT INTO subscribers (email) VALUES (?1) RETURNING *";
    ///             db.first(sql, params![email]).await?.ok_or_else(|| Error::internal("no row returned"))
    ///         })
    ///         .await
    /// }
    /// ```
    pub async fn create_or_first<F, Fut>(&self, db: &Db, create: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        match create().await {
            Err(err) if err.is_taken() => self.first(db).await?.ok_or(Error::NotFound),
            other => other,
        }
    }
}

impl<T: DeserializeOwned> Batches<T> {
    /// Runs the next batch: up to the batch size of rows after the last id
    /// read, or `None` once every row was read.
    ///
    /// See [`Query::batches`].
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the
    /// statement or a row does not deserialize into `T`.
    ///
    /// # Free plan
    ///
    /// One query reading at most the batch size of rows (plus the rows the
    /// conditions skip without an index).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ocre::{Ctx, Query, Result};
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Post {
    ///     id: i64,
    /// }
    ///
    /// async fn count_by_hand(ctx: &Ctx) -> Result<usize> {
    ///     let db = ctx.db()?;
    ///     let mut batches = Query::<Post>::table("posts").batches(500, |post| post.id);
    ///     let mut total = 0;
    ///     while let Some(posts) = batches.next(&db).await? {
    ///         total += posts.len();
    ///     }
    ///     Ok(total)
    /// }
    /// ```
    pub async fn next(&mut self, db: &Db) -> Result<Option<Vec<T>>> {
        if self.is_done() {
            return Ok(None);
        }
        let stmt = self.statement();
        let rows: Vec<T> = db.all(&stmt.sql, stmt.params).await?;
        self.advance(&rows);
        Ok(if rows.is_empty() { None } else { Some(rows) })
    }
}

impl<T> Query<T> {
    /// The query plan, one line per step (Rails' `explain`): `SEARCH` means
    /// an index is used, `SCAN` a full table read.
    ///
    /// Runs [`explain_statement`](Self::explain_statement). Check a query
    /// while developing (log it, or return it from a debug route), not on
    /// every request.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the statement.
    ///
    /// # Free plan
    ///
    /// One query that reads no table row.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, Query, Result};
    /// async fn plan(ctx: &Ctx) -> Result<String> {
    ///     let steps = Query::<()>::table("posts").eq("author_id", 3).explain(&ctx.db()?).await?;
    ///     Ok(steps.join("\n")) // "SEARCH posts USING INDEX index_posts_on_author_id (author_id=?)"
    /// }
    /// ```
    pub async fn explain(&self, db: &Db) -> Result<Vec<String>> {
        let stmt = self.explain_statement();
        let rows: Vec<Plan> = db.all(&stmt.sql, stmt.params).await?;
        Ok(rows.into_iter().map(|row| row.detail).collect())
    }

    /// Number of matching rows (`COUNT(*)`), ignoring order and limits.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the statement.
    ///
    /// # Free plan
    ///
    /// Reads every matching row (an index on the filtered columns keeps it to those).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, Query, Result};
    /// async fn published_count(ctx: &Ctx) -> Result<i64> {
    ///     Query::<()>::table("posts").eq("published", true).count(&ctx.db()?).await
    /// }
    /// ```
    pub async fn count(&self, db: &Db) -> Result<i64> {
        let stmt = self.count_statement();
        let row: Option<Count> = db.first(&stmt.sql, stmt.params).await?;
        Ok(row.map_or(0, |row| row.count))
    }

    /// Whether any row matches (`SELECT 1 ... LIMIT 1`; Rails' `exists?`/`any?`).
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the statement.
    ///
    /// # Free plan
    ///
    /// Stops at the first match: one row read with an index.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, Query, Result};
    /// async fn email_taken(ctx: &Ctx, email: &str) -> Result<bool> {
    ///     Query::<()>::table("users").eq("email", email).exists(&ctx.db()?).await
    /// }
    /// ```
    pub async fn exists(&self, db: &Db) -> Result<bool> {
        let stmt = self.exists_statement();
        db.exists(&stmt.sql, stmt.params).await
    }

    /// Values of one column (or expression) of the matching rows (Rails' `pluck`/`ids`).
    ///
    /// Keeps the query's order and limits; with [`first`](Self::first)-like
    /// use, add `.limit(1)` (Rails' `pick`).
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the
    /// statement or a value does not deserialize into `V`.
    ///
    /// # Free plan
    ///
    /// Same rows read as [`all`](Self::all); less memory and CPU than full rows.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, Query, Result};
    /// async fn published_ids(ctx: &Ctx) -> Result<Vec<i64>> {
    ///     Query::<()>::table("posts").eq("published", true).limit(100).pluck(&ctx.db()?, "id").await
    /// }
    /// ```
    pub async fn pluck<V: DeserializeOwned>(&self, db: &Db, expression: &'static str) -> Result<Vec<V>> {
        let stmt = self.value_statement(expression);
        let rows: Vec<Value<V>> = db.all(&stmt.sql, stmt.params).await?;
        Ok(rows.into_iter().map(|row| row.value).collect())
    }

    /// An aggregate over the matching rows: `SUM(price)`, `AVG(rating)`,
    /// `MIN(created_at)`, `MAX(views)`, `COUNT(DISTINCT author_id)`...
    ///
    /// `None` when SQL returns `NULL` (e.g. `SUM` or `MAX` of no row), so
    /// read it as `Option<V>`.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the
    /// statement or the value does not deserialize into `V`.
    ///
    /// # Free plan
    ///
    /// Reads every matching row.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, Query, Result};
    /// async fn average_rating(ctx: &Ctx, book_id: i64) -> Result<Option<f64>> {
    ///     Query::<()>::table("reviews").eq("book_id", book_id).aggregate(&ctx.db()?, "AVG(stars)").await
    /// }
    /// ```
    pub async fn aggregate<V: DeserializeOwned>(&self, db: &Db, expression: &'static str) -> Result<Option<V>> {
        let stmt = self.aggregate_statement(expression);
        let row: Option<Value<Option<V>>> = db.first(&stmt.sql, stmt.params).await?;
        Ok(row.and_then(|row| row.value))
    }

    /// Updates every matching row without validation (Rails' `update_all`)
    /// and returns how many changed.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the
    /// statement (e.g. a constraint).
    ///
    /// # Free plan
    ///
    /// Counts the rows written, plus the rows scanned to find them.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, IntoParam, Query, Result};
    /// async fn unpublish_author(ctx: &Ctx, author_id: i64) -> Result<usize> {
    ///     Query::<()>::table("posts")
    ///         .eq("author_id", author_id)
    ///         .update_all(&ctx.db()?, vec![("published", false.into_param())])
    ///         .await
    /// }
    /// ```
    pub async fn update_all(&self, db: &Db, sets: Vec<(&'static str, Param)>) -> Result<usize> {
        let stmt = self.update_statement(sets);
        db.execute(&stmt.sql, stmt.params).await
    }

    /// Deletes every matching row (Rails' `delete_all`) and returns how many were deleted.
    ///
    /// Runs no model code: attachments in R2 stay (see `storage::delete_attachments`).
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when D1 rejects the
    /// statement (e.g. a foreign key).
    ///
    /// # Free plan
    ///
    /// Counts the rows written (deleted, plus cascaded ones), plus the rows scanned.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ocre::{Ctx, Query, Result};
    /// async fn purge_expired(ctx: &Ctx) -> Result<usize> {
    ///     Query::<()>::table("sessions").lt("expires_at", ocre::now()).delete_all(&ctx.db()?).await
    /// }
    /// ```
    pub async fn delete_all(&self, db: &Db) -> Result<usize> {
        let stmt = self.delete_statement();
        db.execute(&stmt.sql, stmt.params).await
    }
}
