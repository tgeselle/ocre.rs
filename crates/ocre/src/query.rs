//! A small, explicit SQL builder for one table: conditions with bound
//! parameters, order, limits, grouping, and the statements to count, check,
//! pluck, update or delete the matching rows. Pure Rust: nothing runs until a
//! terminal method (`all`, `first`, `count`, ... in `runtime::query`) sends
//! the built SQL to D1.

use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

use crate::{IntoParam, Page, Param, Statement};

/// Sort direction for [`Query::order_by`], read from a query string as `asc` or `desc`.
///
/// Deserializes from `"asc"`/`"desc"` (any case), so a handler can take it
/// straight from `?direction=desc`; the column must still come from your code
/// (see [`Query::order_by`]).
///
/// # Examples
///
/// ```
/// use ocre::Direction;
///
/// let direction: Direction = serde_json::from_str(r#""desc""#).unwrap();
/// assert_eq!(direction, Direction::Desc);
/// assert_eq!(direction.as_sql(), "DESC");
/// assert_eq!(Direction::default(), Direction::Asc);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Smallest first (`ASC`).
    #[default]
    #[serde(alias = "ASC", alias = "Asc")]
    Asc,
    /// Largest first (`DESC`).
    #[serde(alias = "DESC", alias = "Desc")]
    Desc,
}

impl Direction {
    /// `ASC` or `DESC`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::Direction::Asc.as_sql(), "ASC");
    /// ```
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// The non-generic state of a [`Query`]: SQL fragments using bare `?`
/// placeholders, numbered `?1, ?2...` when a statement is built.
#[derive(Debug, Clone, Default)]
struct Parts {
    table: &'static str,
    select: Option<String>,
    distinct: bool,
    joins: Vec<&'static str>,
    conditions: Vec<String>,
    params: Vec<Param>,
    group: Option<&'static str>,
    having: Vec<String>,
    having_params: Vec<Param>,
    order: Vec<String>,
    order_params: Vec<Param>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// A `SELECT` on one table, built step by step, whose values are always bound parameters.
///
/// Generated models start every query with `query()` (e.g.
/// `post::query()`, which is `Query::table("posts")`), so the row type is
/// known: [`all`](Self::all) returns `Vec<Post>`. Scopes are plain functions
/// taking and returning a `Query` (see [`scope`](Self::scope)).
///
/// - **Values** (`eq`, `is_in`, `contains`...) are always bound as
///   parameters, never written into the SQL.
/// - **Column names and SQL fragments** are `&'static str`: they come from
///   your code, so a request cannot inject SQL through them. To sort by a
///   column the user picks, `match` their input onto a fixed column name.
/// - Conditions combine with `AND`; [`any`](Self::any) groups conditions with
///   `OR`, [`not`](Self::not) negates a group.
/// - Raw fragments ([`where_sql`](Self::where_sql), [`having`](Self::having))
///   use bare `?` placeholders; the builder numbers every placeholder
///   `?1, ?2...` in the final SQL, in order.
///
/// Building costs nothing on the free plan. Running it reads the rows D1
/// scans (see [`Db`](crate::Db)): filter and order on indexed columns, and
/// bound lists with [`limit`](Self::limit) or [`page`](Self::page).
///
/// # Examples
///
/// ```
/// use ocre::{Direction, Page, Query, params};
///
/// #[derive(serde::Deserialize)]
/// struct Post {
///     id: i64,
///     title: String,
/// }
///
/// let query: Query<Post> = Query::table("posts")
///     .eq("published", true)
///     .any(|q| q.contains("title", "rust").contains("body", "rust"))
///     .order_by("created_at", Direction::Desc)
///     .page(Page { limit: 20, offset: 40 });
/// let stmt = query.to_statement();
/// assert_eq!(
///     stmt.sql,
///     "SELECT * FROM posts WHERE published = ?1 AND (title LIKE ?2 ESCAPE '\\' OR body LIKE ?3 ESCAPE '\\') \
///      ORDER BY created_at DESC LIMIT ?4 OFFSET ?5"
/// );
/// assert_eq!(stmt.params, params![true, "%rust%", "%rust%", 20, 40]);
/// ```
///
/// Running it, in a handler or a model function:
///
/// ```no_run
/// # use ocre::{Ctx, Query, Result};
/// # #[derive(serde::Deserialize)] struct Post { id: i64 }
/// async fn published(ctx: &Ctx) -> Result<Vec<Post>> {
///     Query::table("posts").eq("published", true).order_desc("id").limit(20).all(&ctx.db()?).await
/// }
/// ```
pub struct Query<T> {
    parts: Parts,
    row: PhantomData<fn() -> T>,
}

impl<T> Clone for Query<T> {
    fn clone(&self) -> Self {
        Self { parts: self.parts.clone(), row: PhantomData }
    }
}

impl<T> std::fmt::Debug for Query<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stmt = self.parts.select_statement();
        f.debug_struct("Query").field("sql", &stmt.sql).field("params", &stmt.params).finish()
    }
}

impl<T> Query<T> {
    /// Starts a query on `table`: `SELECT * FROM <table>`.
    ///
    /// # Examples
    ///
    /// ```
    /// let query: ocre::Query<serde_json::Value> = ocre::Query::table("posts");
    /// assert_eq!(query.to_statement().sql, "SELECT * FROM posts");
    /// ```
    pub fn table(table: &'static str) -> Self {
        Self { parts: Parts { table, ..Parts::default() }, row: PhantomData }
    }

    /// Applies a scope: `f(self)`. Scopes are plain functions, so they chain
    /// like Rails scopes and take arguments like any function.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// # struct Post;
    /// fn published(query: Query<Post>) -> Query<Post> {
    ///     query.eq("published", true)
    /// }
    /// fn by_author(query: Query<Post>, author_id: i64) -> Query<Post> {
    ///     query.eq("author_id", author_id)
    /// }
    ///
    /// let query = Query::<Post>::table("posts").scope(published).scope(|q| by_author(q, 7));
    /// assert_eq!(query.to_statement().sql, "SELECT * FROM posts WHERE published = ?1 AND author_id = ?2");
    /// ```
    pub fn scope(self, f: impl FnOnce(Self) -> Self) -> Self {
        f(self)
    }

    /// Selects `columns` (an SQL select list) instead of `*`; the rows become `U`.
    ///
    /// The row type changes because the columns do: read them into a struct
    /// with matching field names (use `AS` for expressions).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Title {
    ///     title: String,
    /// }
    ///
    /// let query: Query<Title> = Query::<()>::table("posts").select("title");
    /// assert_eq!(query.to_statement().sql, "SELECT title FROM posts");
    /// ```
    pub fn select<U>(mut self, columns: &'static str) -> Query<U> {
        self.parts.select = Some(columns.to_owned());
        Query { parts: self.parts, row: PhantomData }
    }

    /// `SELECT DISTINCT`: drops duplicate rows.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// let query: Query<()> = Query::<()>::table("posts").select("author_id").distinct();
    /// assert_eq!(query.to_statement().sql, "SELECT DISTINCT author_id FROM posts");
    /// ```
    pub fn distinct(mut self) -> Self {
        self.parts.distinct = true;
        self
    }

    /// Adds a join clause, written in full: `JOIN ...` or `LEFT JOIN ...`.
    ///
    /// Select the columns you need with [`select`](Self::select) (`SELECT *`
    /// would mix both tables' columns) and qualify ambiguous names.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// let query: Query<()> = Query::<()>::table("posts")
    ///     .select("posts.*")
    ///     .join("JOIN taggings ON taggings.post_id = posts.id")
    ///     .eq("taggings.tag_id", 3);
    /// assert_eq!(
    ///     query.to_statement().sql,
    ///     "SELECT posts.* FROM posts JOIN taggings ON taggings.post_id = posts.id WHERE taggings.tag_id = ?1"
    /// );
    /// ```
    pub fn join(mut self, clause: &'static str) -> Self {
        self.parts.joins.push(clause);
        self
    }

    /// `column = value`. A `None` value matches nothing (SQL `= NULL`); use
    /// [`is_null`](Self::is_null) for missing values.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Query, params};
    ///
    /// let stmt = Query::<()>::table("users").eq("email", "ada@example.com").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM users WHERE email = ?1");
    /// assert_eq!(stmt.params, params!["ada@example.com"]);
    /// ```
    pub fn eq(mut self, column: &'static str, value: impl IntoParam) -> Self {
        self.parts.compare(column, "=", value.into_param());
        self
    }

    /// `column != value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").ne("status", "archived").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE status != ?1");
    /// ```
    pub fn ne(mut self, column: &'static str, value: impl IntoParam) -> Self {
        self.parts.compare(column, "!=", value.into_param());
        self
    }

    /// `column > value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").gt("id", 100).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE id > ?1");
    /// ```
    pub fn gt(mut self, column: &'static str, value: impl IntoParam) -> Self {
        self.parts.compare(column, ">", value.into_param());
        self
    }

    /// `column >= value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").gte("created_at", "2026-01-01").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE created_at >= ?1");
    /// ```
    pub fn gte(mut self, column: &'static str, value: impl IntoParam) -> Self {
        self.parts.compare(column, ">=", value.into_param());
        self
    }

    /// `column < value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").lt("views", 10).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE views < ?1");
    /// ```
    pub fn lt(mut self, column: &'static str, value: impl IntoParam) -> Self {
        self.parts.compare(column, "<", value.into_param());
        self
    }

    /// `column <= value`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").lte("views", 10).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE views <= ?1");
    /// ```
    pub fn lte(mut self, column: &'static str, value: impl IntoParam) -> Self {
        self.parts.compare(column, "<=", value.into_param());
        self
    }

    /// `column BETWEEN low AND high`, both bounds included. For a date range
    /// with an optional bound, use [`gte`](Self::gte)/[`lt`](Self::lt) under
    /// `if let Some(..)`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("events").between("starts_on", "2026-01-01", "2026-12-31").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM events WHERE starts_on BETWEEN ?1 AND ?2");
    /// ```
    pub fn between(mut self, column: &'static str, low: impl IntoParam, high: impl IntoParam) -> Self {
        self.parts.push_condition(format!("{column} BETWEEN ? AND ?"), vec![low.into_param(), high.into_param()]);
        self
    }

    /// `column IN (?, ?, ...)`. An empty list matches no row (`0`), like
    /// Rails' `where(id: [])`. D1 binds at most 100 parameters per query.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// let stmt = Query::<()>::table("posts").is_in("id", [1, 2, 3]).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE id IN (?1, ?2, ?3)");
    /// let none = Query::<()>::table("posts").is_in("id", Vec::<i64>::new()).to_statement();
    /// assert_eq!(none.sql, "SELECT * FROM posts WHERE 0");
    /// ```
    pub fn is_in<V: IntoParam>(mut self, column: &'static str, values: impl IntoIterator<Item = V>) -> Self {
        self.parts.list(column, "IN", values.into_iter().map(IntoParam::into_param).collect());
        self
    }

    /// `column NOT IN (?, ?, ...)`. An empty list excludes nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").not_in("status", ["draft", "archived"]).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE status NOT IN (?1, ?2)");
    /// ```
    pub fn not_in<V: IntoParam>(mut self, column: &'static str, values: impl IntoIterator<Item = V>) -> Self {
        self.parts.list(column, "NOT IN", values.into_iter().map(IntoParam::into_param).collect());
        self
    }

    /// `column IS NULL`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").is_null("deleted_at").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE deleted_at IS NULL");
    /// ```
    pub fn is_null(mut self, column: &'static str) -> Self {
        self.parts.push_condition(format!("{column} IS NULL"), vec![]);
        self
    }

    /// `column IS NOT NULL`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").is_not_null("published_at").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE published_at IS NOT NULL");
    /// ```
    pub fn is_not_null(mut self, column: &'static str) -> Self {
        self.parts.push_condition(format!("{column} IS NOT NULL"), vec![]);
        self
    }

    /// `column LIKE pattern`, with the pattern as given: `%` and `_` are
    /// wildcards. For user input, use [`contains`](Self::contains),
    /// [`starts_with`](Self::starts_with) or [`ends_with`](Self::ends_with),
    /// which escape them. SQLite's `LIKE` ignores ASCII case.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").like("slug", "2026-%").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE slug LIKE ?1");
    /// ```
    pub fn like(mut self, column: &'static str, pattern: impl IntoParam) -> Self {
        self.parts.compare(column, "LIKE", pattern.into_param());
        self
    }

    /// `column NOT LIKE pattern` (wildcards as given, see [`like`](Self::like)).
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("users").not_like("email", "%@example.com").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM users WHERE email NOT LIKE ?1");
    /// ```
    pub fn not_like(mut self, column: &'static str, pattern: impl IntoParam) -> Self {
        self.parts.compare(column, "NOT LIKE", pattern.into_param());
        self
    }

    /// `column` contains `text` (case-insensitive for ASCII), with `%`, `_`
    /// and `\` in `text` matched literally (see [`escape_like`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Query, params};
    ///
    /// let stmt = Query::<()>::table("posts").contains("title", "100%").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE title LIKE ?1 ESCAPE '\\'");
    /// assert_eq!(stmt.params, params!["%100\\%%"]);
    /// ```
    pub fn contains(mut self, column: &'static str, text: &str) -> Self {
        self.parts.escaped_like(column, format!("%{}%", escape_like(text)));
        self
    }

    /// `column` starts with `text` (wildcards escaped, see [`contains`](Self::contains)).
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("users").starts_with("name", "Ad").to_statement();
    /// assert_eq!(stmt.params, ocre::params!["Ad%"]);
    /// ```
    pub fn starts_with(mut self, column: &'static str, text: &str) -> Self {
        self.parts.escaped_like(column, format!("{}%", escape_like(text)));
        self
    }

    /// `column` ends with `text` (wildcards escaped, see [`contains`](Self::contains)).
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("users").ends_with("email", "@example.com").to_statement();
    /// assert_eq!(stmt.params, ocre::params!["%@example.com"]);
    /// ```
    pub fn ends_with(mut self, column: &'static str, text: &str) -> Self {
        self.parts.escaped_like(column, format!("%{}", escape_like(text)));
        self
    }

    /// Adds a raw SQL condition with bare `?` placeholders, bound to `params` in order.
    ///
    /// The fragment is wrapped in parentheses, so an `OR` inside stays
    /// grouped. Use it for anything the other methods do not cover: SQL
    /// functions, subqueries (`EXISTS (...)` for "has at least one
    /// comment"), date arithmetic.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Query, params};
    ///
    /// let stmt = Query::<()>::table("posts")
    ///     .eq("published", true)
    ///     .where_sql("created_at > datetime('now', ?)", params!["-7 days"])
    ///     .where_sql("EXISTS (SELECT 1 FROM comments WHERE comments.post_id = posts.id)", params![])
    ///     .to_statement();
    /// assert_eq!(
    ///     stmt.sql,
    ///     "SELECT * FROM posts WHERE published = ?1 AND (created_at > datetime('now', ?2)) \
    ///      AND (EXISTS (SELECT 1 FROM comments WHERE comments.post_id = posts.id))"
    /// );
    /// ```
    pub fn where_sql(mut self, fragment: &'static str, params: Vec<Param>) -> Self {
        self.parts.push_condition(format!("({fragment})"), params);
        self
    }

    /// Keeps rows with at least one row in `table` pointing to them through
    /// `foreign_key` (Rails' `where.associated` on a has-many side).
    ///
    /// Writes `EXISTS (SELECT 1 FROM <table> WHERE <table>.<foreign_key> = <this table>.id)`,
    /// which stops at the first child: with an index on the foreign key
    /// (generated for every `references` field) it reads one child row per
    /// parent. For the belongs-to side, test the column itself:
    /// `is_not_null("author_id")`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").where_associated("comments", "post_id").to_statement();
    /// assert_eq!(
    ///     stmt.sql,
    ///     "SELECT * FROM posts WHERE EXISTS (SELECT 1 FROM comments WHERE comments.post_id = posts.id)"
    /// );
    /// ```
    pub fn where_associated(mut self, table: &'static str, foreign_key: &'static str) -> Self {
        let condition = self.parts.child_exists(table, foreign_key);
        self.parts.push_condition(condition, vec![]);
        self
    }

    /// Keeps rows that no row of `table` points to through `foreign_key`
    /// (Rails' `where.missing` on a has-many side): posts without comments.
    ///
    /// The belongs-to side is `is_null("author_id")`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").where_missing("comments", "post_id").to_statement();
    /// assert_eq!(
    ///     stmt.sql,
    ///     "SELECT * FROM posts WHERE NOT EXISTS (SELECT 1 FROM comments WHERE comments.post_id = posts.id)"
    /// );
    /// ```
    pub fn where_missing(mut self, table: &'static str, foreign_key: &'static str) -> Self {
        let condition = format!("NOT {}", self.parts.child_exists(table, foreign_key));
        self.parts.push_condition(condition, vec![]);
        self
    }

    /// Filters `column` between two optional bounds, like Loco's `DateRangeBuilder`.
    ///
    /// Both bounds: `BETWEEN from AND to` (inclusive). One bound: strictly
    /// after `from` (`>`) or strictly before `to` (`<`). No bound: no
    /// condition. Made for `?from=&to=` filters, whose values arrive as
    /// `Option`s; works on any comparable column (dates stored as ISO text
    /// compare correctly).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Query, params};
    ///
    /// let both = Query::<()>::table("posts").date_range("created_at", Some("2026-01-01"), Some("2026-01-31"));
    /// assert_eq!(both.to_statement().sql, "SELECT * FROM posts WHERE created_at BETWEEN ?1 AND ?2");
    /// let from = Query::<()>::table("posts").date_range("created_at", Some("2026-01-01"), None);
    /// assert_eq!(from.to_statement().sql, "SELECT * FROM posts WHERE created_at > ?1");
    /// let to = Query::<()>::table("posts").date_range("created_at", None, Some("2026-01-31"));
    /// assert_eq!(to.to_statement().sql, "SELECT * FROM posts WHERE created_at < ?1");
    /// let none = Query::<()>::table("posts").date_range::<&str>("created_at", None, None);
    /// assert_eq!(none.to_statement().sql, "SELECT * FROM posts");
    /// ```
    pub fn date_range<V: IntoParam>(self, column: &'static str, from: Option<V>, to: Option<V>) -> Self {
        match (from, to) {
            (Some(from), Some(to)) => self.between(column, from, to),
            (Some(from), None) => self.gt(column, from),
            (None, Some(to)) => self.lt(column, to),
            (None, None) => self,
        }
    }

    /// Removes every condition added so far (Rails' `unscope(:where)`);
    /// chain new ones after it for Rails' `rewhere`.
    ///
    /// Useful to reuse a scoped query (a model's `query()` that hides
    /// soft-deleted rows, say) without its conditions. Joins, order and
    /// limits stay.
    ///
    /// # Examples
    ///
    /// ```
    /// let visible = ocre::Query::<()>::table("posts").is_null("deleted_at").order_desc("id");
    /// let stmt = visible.unscope_where().eq("author_id", 3).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE author_id = ?1 ORDER BY id DESC");
    /// ```
    pub fn unscope_where(mut self) -> Self {
        self.parts.conditions.clear();
        self.parts.params.clear();
        self
    }

    /// Removes the limit and offset set so far (Rails' `unscope(:limit, :offset)`).
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").limit(10).offset(20).unscope_limit().to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts");
    /// ```
    pub fn unscope_limit(mut self) -> Self {
        self.parts.limit = None;
        self.parts.offset = None;
        self
    }

    /// Reverses the order (Rails' `reverse_order`): `ASC` terms become
    /// `DESC` and back; terms without a direction ([`order_in`](Self::order_in),
    /// a bare [`order_sql`](Self::order_sql)) get `DESC`. Without any order,
    /// sorts by `<table>.id DESC`.
    ///
    /// Write raw terms with `NULLS FIRST/LAST` in full instead: they cannot
    /// be flipped by appending a direction.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// let stmt = Query::<()>::table("posts").order_asc("title").order_desc("id").reverse_order().to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts ORDER BY title DESC, id ASC");
    /// assert_eq!(Query::<()>::table("posts").reverse_order().to_statement().sql, "SELECT * FROM posts ORDER BY posts.id DESC");
    /// ```
    pub fn reverse_order(mut self) -> Self {
        self.parts.reverse_order();
        self
    }

    /// Matches rows meeting at least one of the conditions `f` adds: `(a OR b ...)`.
    ///
    /// `f` receives an empty query to add conditions to; its table, order and
    /// limits are ignored. No condition added: nothing changes.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts")
    ///     .eq("published", true)
    ///     .any(|q| q.eq("author_id", 1).is_null("author_id"))
    ///     .to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE published = ?1 AND (author_id = ?2 OR author_id IS NULL)");
    /// ```
    pub fn any(mut self, f: impl FnOnce(Self) -> Self) -> Self {
        let group = f(Self::table(self.parts.table)).parts;
        self.parts.group(group, " OR ", "");
        self
    }

    /// Matches rows that do not meet all the conditions `f` adds: `NOT (a AND b ...)`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").not(|q| q.eq("status", "draft").eq("author_id", 3)).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE NOT (status = ?1 AND author_id = ?2)");
    /// ```
    pub fn not(mut self, f: impl FnOnce(Self) -> Self) -> Self {
        let group = f(Self::table(self.parts.table)).parts;
        self.parts.group(group, " AND ", "NOT ");
        self
    }

    /// Matches no row (`WHERE 0`), like Rails' `none`: a scope can return
    /// it when a filter makes the result empty. D1 still runs the query, but
    /// reads no row.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").none().to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE 0");
    /// ```
    pub fn none(mut self) -> Self {
        self.parts.push_condition("0".to_owned(), vec![]);
        self
    }

    /// `GROUP BY columns`; combine with [`select`](Self::select) for the
    /// aggregates and [`having`](Self::having) to filter groups.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Query, params};
    ///
    /// #[derive(serde::Deserialize)]
    /// struct PerAuthor {
    ///     author_id: i64,
    ///     count: i64,
    /// }
    ///
    /// let query: Query<PerAuthor> = Query::<()>::table("posts")
    ///     .select("author_id, COUNT(*) AS count")
    ///     .group_by("author_id")
    ///     .having("COUNT(*) >= ?", params![5])
    ///     .order_desc("count");
    /// assert_eq!(
    ///     query.to_statement().sql,
    ///     "SELECT author_id, COUNT(*) AS count FROM posts GROUP BY author_id HAVING (COUNT(*) >= ?1) ORDER BY count DESC"
    /// );
    /// ```
    pub fn group_by(mut self, columns: &'static str) -> Self {
        self.parts.group = Some(columns);
        self
    }

    /// Adds a `HAVING` condition (raw SQL with bare `?` placeholders) on the groups.
    ///
    /// See [`group_by`](Self::group_by).
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").group_by("author_id").having("COUNT(*) > ?", ocre::params![1]).to_statement();
    /// assert!(stmt.sql.ends_with("HAVING (COUNT(*) > ?1)"));
    /// ```
    pub fn having(mut self, fragment: &'static str, params: Vec<Param>) -> Self {
        self.parts.having.push(format!("({fragment})"));
        self.parts.having_params.extend(params);
        self
    }

    /// `ORDER BY column ASC`, after any order already set.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").order_asc("title").order_desc("id").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts ORDER BY title ASC, id DESC");
    /// ```
    pub fn order_asc(self, column: &'static str) -> Self {
        self.order_by(column, Direction::Asc)
    }

    /// `ORDER BY column DESC`, after any order already set.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").order_desc("id").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts ORDER BY id DESC");
    /// ```
    pub fn order_desc(self, column: &'static str) -> Self {
        self.order_by(column, Direction::Desc)
    }

    /// `ORDER BY column <direction>`, after any order already set.
    ///
    /// The direction may come from the request; the column may not, so map
    /// the user's choice onto a fixed name.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Direction, Query};
    ///
    /// // `sort` and `direction` from `?sort=title&direction=desc`.
    /// let (sort, direction) = ("title", Direction::Desc);
    /// let column = match sort {
    ///     "title" => "title",
    ///     _ => "created_at",
    /// };
    /// let stmt = Query::<()>::table("posts").order_by(column, direction).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts ORDER BY title DESC");
    /// ```
    pub fn order_by(mut self, column: &'static str, direction: Direction) -> Self {
        self.parts.order.push(format!("{column} {}", direction.as_sql()));
        self
    }

    /// Orders `column` by an explicit list of values (Rails' `in_order_of`):
    /// rows whose value is not listed come last.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("tasks").order_in("status", ["urgent", "open"]).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM tasks ORDER BY CASE status WHEN ?1 THEN 0 WHEN ?2 THEN 1 ELSE 2 END");
    /// ```
    pub fn order_in<V: IntoParam>(mut self, column: &'static str, values: impl IntoIterator<Item = V>) -> Self {
        self.parts.order_in(column, values.into_iter().map(IntoParam::into_param).collect());
        self
    }

    /// Adds a raw `ORDER BY` term, e.g. `lower(title)` or `published_at DESC NULLS LAST`.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").order_sql("lower(title) ASC").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts ORDER BY lower(title) ASC");
    /// ```
    pub fn order_sql(mut self, term: &'static str) -> Self {
        self.parts.order.push(term.to_owned());
        self
    }

    /// Removes the order set so far (Rails' `reorder` when followed by a new order).
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").order_desc("id").reorder().order_asc("title").to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts ORDER BY title ASC");
    /// ```
    pub fn reorder(mut self) -> Self {
        self.parts.order.clear();
        self.parts.order_params.clear();
        self
    }

    /// Returns at most `limit` rows.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").limit(5).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts LIMIT ?1");
    /// ```
    pub fn limit(mut self, limit: i64) -> Self {
        self.parts.limit = Some(limit);
        self
    }

    /// Skips the first `offset` rows.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").offset(10).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts LIMIT -1 OFFSET ?1");
    /// ```
    pub fn offset(mut self, offset: i64) -> Self {
        self.parts.offset = Some(offset);
        self
    }

    /// Limit and offset from a [`Page`] (`?limit=&offset=`).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Page, Query, params};
    ///
    /// let stmt = Query::<()>::table("posts").page(Page { limit: 20, offset: 40 }).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts LIMIT ?1 OFFSET ?2");
    /// assert_eq!(stmt.params, params![20, 40]);
    /// ```
    pub fn page(self, page: Page) -> Self {
        self.limit(page.limit).offset(page.offset)
    }

    /// `EXPLAIN QUERY PLAN` of the `SELECT` (Rails' `explain`): how SQLite
    /// finds the rows, e.g. `SEARCH posts USING INDEX index_posts_on_author_id (author_id=?)`
    /// or a full `SCAN posts`.
    ///
    /// Run it with [`explain`](Self::explain), or print the SQL and run it with
    /// `ocre sql "EXPLAIN QUERY PLAN ..."` on the local database.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").eq("author_id", 3).explain_statement();
    /// assert_eq!(stmt.sql, "EXPLAIN QUERY PLAN SELECT * FROM posts WHERE author_id = ?1");
    /// ```
    pub fn explain_statement(&self) -> Statement {
        let stmt = self.parts.select_statement();
        Statement { sql: format!("EXPLAIN QUERY PLAN {}", stmt.sql), params: stmt.params }
    }

    /// Walks the matching rows in batches of `size`, ordered by id (Rails'
    /// `find_in_batches` / `in_batches`, and `find_each` with a loop over each batch).
    ///
    /// `id` reads a row's primary key (`|post| post.id`): each batch starts
    /// after the last id of the previous one (keyset pagination: `WHERE id >
    /// last ORDER BY id LIMIT size`), so batches stay cheap however far they
    /// go, unlike `OFFSET`. The query's own order and limits are replaced;
    /// for Rails' `start:` / `finish:` add `gte("id", start)` / `lte("id", finish)`.
    /// [`Batches::next`] runs one batch.
    ///
    /// # Free plan
    ///
    /// Each batch is one query reading `size` rows. A Worker invocation may
    /// run 50 D1 queries on the free plan and has 10 ms of CPU: walk large
    /// tables from a job or scheduled task, a few batches per invocation
    /// (keep [`Batches::after`] to resume in the next one).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Post {
    ///     id: i64,
    /// }
    ///
    /// let mut batches = Query::<Post>::table("posts").eq("published", true).batches(500, |post| post.id);
    /// assert_eq!(
    ///     batches.statement().sql,
    ///     "SELECT * FROM posts WHERE published = ?1 ORDER BY posts.id ASC LIMIT ?2"
    /// );
    /// batches.advance(&[Post { id: 7 }, Post { id: 9 }]);
    /// assert_eq!(batches.after(), Some(9));
    /// assert_eq!(
    ///     batches.statement().sql,
    ///     "SELECT * FROM posts WHERE published = ?1 AND posts.id > ?2 ORDER BY posts.id ASC LIMIT ?3"
    /// );
    /// ```
    pub fn batches(self, size: i64, id: fn(&T) -> i64) -> Batches<T> {
        Batches { query: self, size: size.max(1), id, after: None, done: false }
    }

    /// The `SELECT` statement, with placeholders numbered `?1, ?2...`.
    ///
    /// Terminal methods ([`all`](Self::all), [`first`](Self::first)...) run
    /// it; use it directly for [`Db::batch`](crate::Db::batch) or logging.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("posts").eq("id", 1).to_statement();
    /// assert_eq!(stmt.sql, "SELECT * FROM posts WHERE id = ?1");
    /// ```
    pub fn to_statement(&self) -> Statement {
        self.parts.select_statement()
    }

    /// `SELECT COUNT(*) AS count` of the matching rows, ignoring order, limit
    /// and offset. With [`group_by`](Self::group_by) or
    /// [`distinct`](Self::distinct), counts the groups or distinct rows.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// let query = Query::<()>::table("posts").eq("published", true).order_desc("id").limit(10);
    /// assert_eq!(query.count_statement().sql, "SELECT COUNT(*) AS count FROM posts WHERE published = ?1");
    /// let authors = Query::<()>::table("posts").select::<()>("author_id").distinct();
    /// assert_eq!(
    ///     authors.count_statement().sql,
    ///     "SELECT COUNT(*) AS count FROM (SELECT DISTINCT author_id FROM posts)"
    /// );
    /// ```
    pub fn count_statement(&self) -> Statement {
        self.parts.count_statement()
    }

    /// `SELECT 1 ... LIMIT 1`: whether any row matches, stopping at the first.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("users").eq("email", "a@b.co").exists_statement();
    /// assert_eq!(stmt.sql, "SELECT 1 FROM users WHERE email = ?1 LIMIT 1");
    /// ```
    pub fn exists_statement(&self) -> Statement {
        self.parts.exists_statement()
    }

    /// `SELECT <expression> AS value`, keeping conditions, order and limits:
    /// one column of every matching row (Rails' `pluck`), or with an
    /// aggregate expression (`SUM(price)`) the calculation over the matching
    /// rows (order and limits are dropped then, as they do not apply).
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Query;
    ///
    /// let query = Query::<()>::table("posts").eq("published", true).order_desc("id").limit(3);
    /// assert_eq!(
    ///     query.value_statement("title").sql,
    ///     "SELECT title AS value FROM posts WHERE published = ?1 ORDER BY id DESC LIMIT ?2"
    /// );
    /// assert_eq!(query.aggregate_statement("SUM(views)").sql, "SELECT SUM(views) AS value FROM posts WHERE published = ?1");
    /// ```
    pub fn value_statement(&self, expression: &'static str) -> Statement {
        self.parts.value_statement(expression, false)
    }

    /// `SELECT <aggregate> AS value` over the matching rows, without order or limits.
    ///
    /// See [`value_statement`](Self::value_statement).
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("products").aggregate_statement("MAX(price)");
    /// assert_eq!(stmt.sql, "SELECT MAX(price) AS value FROM products");
    /// ```
    pub fn aggregate_statement(&self, expression: &'static str) -> Statement {
        self.parts.value_statement(expression, true)
    }

    /// `UPDATE <table> SET ... WHERE ...` on the matching rows (Rails'
    /// `update_all`): no validation, no `updated_at` change unless listed.
    ///
    /// Joins, order and limits are not part of the statement.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{IntoParam, Query, params};
    ///
    /// let stmt = Query::<()>::table("posts")
    ///     .eq("author_id", 3)
    ///     .update_statement(vec![("published", false.into_param()), ("updated_at", "2026-09-29 10:00:00".into_param())]);
    /// assert_eq!(stmt.sql, "UPDATE posts SET published = ?1, updated_at = ?2 WHERE author_id = ?3");
    /// assert_eq!(stmt.params, params![false, "2026-09-29 10:00:00", 3]);
    /// ```
    pub fn update_statement(&self, sets: Vec<(&'static str, Param)>) -> Statement {
        self.parts.update_statement(sets)
    }

    /// `DELETE FROM <table> WHERE ...` on the matching rows (Rails' `delete_all`).
    ///
    /// Joins, order and limits are not part of the statement. Without any
    /// condition, it deletes every row of the table.
    ///
    /// # Examples
    ///
    /// ```
    /// let stmt = ocre::Query::<()>::table("sessions").lt("expires_at", 1_790_000_000).delete_statement();
    /// assert_eq!(stmt.sql, "DELETE FROM sessions WHERE expires_at < ?1");
    /// ```
    pub fn delete_statement(&self) -> Statement {
        self.parts.delete_statement()
    }
}

impl Parts {
    fn push_condition(&mut self, sql: String, params: Vec<Param>) {
        self.conditions.push(sql);
        self.params.extend(params);
    }

    fn compare(&mut self, column: &str, operator: &str, value: Param) {
        self.push_condition(format!("{column} {operator} ?"), vec![value]);
    }

    fn escaped_like(&mut self, column: &str, pattern: String) {
        self.push_condition(format!("{column} LIKE ? ESCAPE '\\'"), vec![pattern.into_param()]);
    }

    fn list(&mut self, column: &str, operator: &str, values: Vec<Param>) {
        match (values.is_empty(), operator) {
            (true, "IN") => self.push_condition("0".to_owned(), vec![]),
            (true, _) => {}
            (false, _) => {
                let placeholders = vec!["?"; values.len()].join(", ");
                self.push_condition(format!("{column} {operator} ({placeholders})"), values);
            }
        }
    }

    fn group(&mut self, group: Parts, joiner: &str, prefix: &str) {
        if group.conditions.is_empty() {
            return;
        }
        self.push_condition(format!("{prefix}({})", group.conditions.join(joiner)), group.params);
    }

    fn order_in(&mut self, column: &str, values: Vec<Param>) {
        let mut term = format!("CASE {column}");
        for i in 0..values.len() {
            term.push_str(&format!(" WHEN ? THEN {i}"));
        }
        term.push_str(&format!(" ELSE {} END", values.len()));
        self.order.push(term);
        self.order_params.extend(values);
    }

    /// `FROM <table> <joins> WHERE ...`, with its parameters.
    fn source(&self, sql: &mut String, params: &mut Vec<Param>) {
        sql.push_str(" FROM ");
        sql.push_str(self.table);
        for join in &self.joins {
            sql.push(' ');
            sql.push_str(join);
        }
        self.where_clause(sql, params);
    }

    fn where_clause(&self, sql: &mut String, params: &mut Vec<Param>) {
        if !self.conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.conditions.join(" AND "));
            params.extend(self.params.iter().cloned());
        }
    }

    /// `GROUP BY` and `HAVING`.
    fn grouping(&self, sql: &mut String, params: &mut Vec<Param>) {
        if let Some(group) = self.group {
            sql.push_str(" GROUP BY ");
            sql.push_str(group);
        }
        if !self.having.is_empty() {
            sql.push_str(" HAVING ");
            sql.push_str(&self.having.join(" AND "));
            params.extend(self.having_params.iter().cloned());
        }
    }

    /// `ORDER BY`, `LIMIT` and `OFFSET`.
    fn ordering(&self, sql: &mut String, params: &mut Vec<Param>) {
        if !self.order.is_empty() {
            sql.push_str(" ORDER BY ");
            sql.push_str(&self.order.join(", "));
            params.extend(self.order_params.iter().cloned());
        }
        match (self.limit, self.offset) {
            (Some(limit), Some(offset)) => {
                sql.push_str(" LIMIT ? OFFSET ?");
                params.extend([limit.into_param(), offset.into_param()]);
            }
            (Some(limit), None) => {
                sql.push_str(" LIMIT ?");
                params.push(limit.into_param());
            }
            (None, Some(offset)) => {
                sql.push_str(" LIMIT -1 OFFSET ?");
                params.push(offset.into_param());
            }
            (None, None) => {}
        }
    }

    fn select_head(&self, columns: &str) -> String {
        let distinct = if self.distinct { "DISTINCT " } else { "" };
        format!("SELECT {distinct}{columns}")
    }

    fn select_statement(&self) -> Statement {
        let mut params = Vec::new();
        let mut sql = self.select_head(self.select.as_deref().unwrap_or("*"));
        self.source(&mut sql, &mut params);
        self.grouping(&mut sql, &mut params);
        self.ordering(&mut sql, &mut params);
        numbered(sql, params)
    }

    fn count_statement(&self) -> Statement {
        let mut params = Vec::new();
        if self.group.is_some() || self.distinct {
            let mut inner = self.select_head(self.select.as_deref().unwrap_or("*"));
            self.source(&mut inner, &mut params);
            self.grouping(&mut inner, &mut params);
            return numbered(format!("SELECT COUNT(*) AS count FROM ({inner})"), params);
        }
        let mut sql = "SELECT COUNT(*) AS count".to_owned();
        self.source(&mut sql, &mut params);
        numbered(sql, params)
    }

    fn exists_statement(&self) -> Statement {
        let mut params = Vec::new();
        let mut sql = "SELECT 1".to_owned();
        self.source(&mut sql, &mut params);
        self.grouping(&mut sql, &mut params);
        sql.push_str(" LIMIT 1");
        numbered(sql, params)
    }

    fn value_statement(&self, expression: &str, aggregate: bool) -> Statement {
        let mut params = Vec::new();
        let mut sql = self.select_head(&format!("{expression} AS value"));
        self.source(&mut sql, &mut params);
        self.grouping(&mut sql, &mut params);
        if !aggregate {
            self.ordering(&mut sql, &mut params);
        }
        numbered(sql, params)
    }

    fn update_statement(&self, sets: Vec<(&'static str, Param)>) -> Statement {
        let mut params = Vec::with_capacity(sets.len() + self.params.len());
        let mut assignments = Vec::with_capacity(sets.len());
        for (column, value) in sets {
            assignments.push(format!("{column} = ?"));
            params.push(value);
        }
        let mut sql = format!("UPDATE {} SET {}", self.table, assignments.join(", "));
        self.where_clause(&mut sql, &mut params);
        numbered(sql, params)
    }

    fn delete_statement(&self) -> Statement {
        let mut params = Vec::new();
        let mut sql = format!("DELETE FROM {}", self.table);
        self.where_clause(&mut sql, &mut params);
        numbered(sql, params)
    }

    /// `EXISTS (SELECT 1 FROM <table> WHERE <table>.<fk> = <self.table>.id)`.
    fn child_exists(&self, table: &str, foreign_key: &str) -> String {
        format!("EXISTS (SELECT 1 FROM {table} WHERE {table}.{foreign_key} = {}.id)", self.table)
    }

    fn reverse_order(&mut self) {
        if self.order.is_empty() {
            self.order.push(format!("{}.id DESC", self.table));
            return;
        }
        for term in &mut self.order {
            if let Some(column) = term.strip_suffix(" ASC") {
                *term = format!("{column} DESC");
            } else if let Some(column) = term.strip_suffix(" DESC") {
                *term = format!("{column} ASC");
            } else {
                term.push_str(" DESC");
            }
        }
    }

    /// The next batch of [`Batches`]: after `after` by `<table>.id`, in id order.
    fn batch(&mut self, after: Option<i64>, size: i64) {
        let id = format!("{}.id", self.table);
        if let Some(after) = after {
            self.compare(&id, ">", after.into_param());
        }
        self.order = vec![format!("{id} ASC")];
        self.order_params.clear();
        self.limit = Some(size);
        self.offset = None;
    }
}

/// Numbers the bare `?` placeholders of `sql` as `?1, ?2...`, skipping
/// quoted strings and identifiers.
fn numbered(sql: String, params: Vec<Param>) -> Statement {
    let mut out = String::with_capacity(sql.len() + 2 * params.len());
    let mut next = 1;
    let mut quote: Option<char> = None;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '?') if !chars.peek().is_some_and(char::is_ascii_digit) => {
                out.push_str(&next.to_string());
                next += 1;
            }
            (None, _) => {}
        }
    }
    Statement::new(out, params)
}

/// Escapes `%`, `_` and `\` so `text` matches literally in a `LIKE ... ESCAPE '\'`
/// pattern (Rails' `sanitize_sql_like`).
///
/// [`Query::contains`], [`starts_with`](Query::starts_with) and
/// [`ends_with`](Query::ends_with) call it; use it for hand-written SQL.
///
/// # Examples
///
/// ```
/// use ocre::{escape_like, params};
///
/// assert_eq!(escape_like("50%_off\\"), "50\\%\\_off\\\\");
/// let pattern = format!("%{}%", escape_like("50%"));
/// let sql = "SELECT * FROM products WHERE name LIKE ?1 ESCAPE '\\'";
/// # let _ = (sql, params![pattern]);
/// ```
pub fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A page of rows plus the total, for pagination links and JSON envelopes.
///
/// [`Query::paginate`] builds one with two queries (the rows and
/// `COUNT(*)`). Serializes as
/// `{"items": [...], "total": 42, "limit": 20, "offset": 0}`.
///
/// # Free plan
///
/// The count reads every matching row once more: on large tables, prefer
/// plain "next page" links without a total ([`Page::next`](crate::Page)
/// after [`Query::all`]) or cache the total.
///
/// # Examples
///
/// ```
/// use ocre::{Page, Paginated};
///
/// let page = Paginated { items: vec!["a", "b"], total: 5, limit: 2, offset: 2 };
/// assert_eq!(page.current_page(), 2);
/// assert_eq!(page.total_pages(), 3);
/// assert!(page.has_next() && page.has_previous());
/// assert_eq!(page.next_page(), Some(Page { limit: 2, offset: 4 }));
/// assert_eq!(page.previous_page(), Some(Page { limit: 2, offset: 0 }));
/// assert_eq!(serde_json::to_string(&page).unwrap(), r#"{"items":["a","b"],"total":5,"limit":2,"offset":2}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Paginated<T> {
    /// The rows of this page.
    pub items: Vec<T>,
    /// Rows matching the query, all pages together.
    pub total: i64,
    /// Rows per page.
    pub limit: i64,
    /// Rows skipped before this page.
    pub offset: i64,
}

impl<T> Paginated<T> {
    /// 1-based number of this page.
    ///
    /// # Examples
    ///
    /// ```
    /// let page = ocre::Paginated::<()> { items: vec![], total: 0, limit: 10, offset: 0 };
    /// assert_eq!(page.current_page(), 1);
    /// ```
    pub fn current_page(&self) -> i64 {
        self.offset / self.limit.max(1) + 1
    }

    /// Number of pages (at least 1, even with no row).
    ///
    /// # Examples
    ///
    /// ```
    /// let page = ocre::Paginated::<()> { items: vec![], total: 21, limit: 10, offset: 0 };
    /// assert_eq!(page.total_pages(), 3);
    /// ```
    pub fn total_pages(&self) -> i64 {
        ((self.total + self.limit - 1) / self.limit.max(1)).max(1)
    }

    /// Whether rows follow this page.
    ///
    /// # Examples
    ///
    /// ```
    /// let page = ocre::Paginated::<()> { items: vec![], total: 10, limit: 10, offset: 0 };
    /// assert!(!page.has_next());
    /// ```
    pub fn has_next(&self) -> bool {
        self.offset + self.limit < self.total
    }

    /// Whether rows come before this page.
    ///
    /// # Examples
    ///
    /// ```
    /// let page = ocre::Paginated::<()> { items: vec![], total: 10, limit: 10, offset: 0 };
    /// assert!(!page.has_previous());
    /// ```
    pub fn has_previous(&self) -> bool {
        self.offset > 0
    }

    /// The next page, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// let page = ocre::Paginated::<()> { items: vec![], total: 3, limit: 2, offset: 2 };
    /// assert_eq!(page.next_page(), None);
    /// ```
    pub fn next_page(&self) -> Option<Page> {
        self.has_next().then_some(Page { limit: self.limit, offset: self.offset + self.limit })
    }

    /// The previous page, if any (never a negative offset).
    ///
    /// # Examples
    ///
    /// ```
    /// let page = ocre::Paginated::<()> { items: vec![], total: 30, limit: 10, offset: 5 };
    /// assert_eq!(page.previous_page(), Some(ocre::Page { limit: 10, offset: 0 }));
    /// ```
    pub fn previous_page(&self) -> Option<Page> {
        self.has_previous().then_some(Page { limit: self.limit, offset: (self.offset - self.limit).max(0) })
    }

    /// Converts the items, keeping the counts (e.g. rows into view structs).
    ///
    /// # Examples
    ///
    /// ```
    /// let page = ocre::Paginated { items: vec![1, 2], total: 2, limit: 10, offset: 0 };
    /// assert_eq!(page.map(|n| n * 10).items, [10, 20]);
    /// ```
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> Paginated<U> {
        Paginated {
            items: self.items.into_iter().map(f).collect(),
            total: self.total,
            limit: self.limit,
            offset: self.offset,
        }
    }
}

/// Batches of rows by increasing id, from [`Query::batches`]: Rails'
/// `find_in_batches` without holding a cursor open.
///
/// [`next`](Self::next) runs one query and returns the next batch, or
/// `None` when every row was read. [`after`](Self::after) is the last id
/// seen: store it (in a job's arguments, in KV) to resume later with
/// [`resume_after`](Self::resume_after).
///
/// # Examples
///
/// ```no_run
/// use ocre::{Ctx, Query, Result};
///
/// #[derive(serde::Deserialize)]
/// struct User {
///     id: i64,
///     email: String,
/// }
///
/// // A scheduled task: at most 4 batches (4 queries) per run.
/// async fn send_digests(ctx: &Ctx, resume: Option<i64>) -> Result<Option<i64>> {
///     let db = ctx.db()?;
///     let mut batches = Query::<User>::table("users").batches(100, |user| user.id).resume_after(resume);
///     for _ in 0..4 {
///         let Some(users) = batches.next(&db).await? else { return Ok(None) };
///         for user in users {
///             // find_each: one row at a time.
///             let _ = user.email;
///         }
///     }
///     Ok(batches.after())
/// }
/// ```
pub struct Batches<T> {
    query: Query<T>,
    size: i64,
    id: fn(&T) -> i64,
    after: Option<i64>,
    done: bool,
}

impl<T> Batches<T> {
    /// Starts after `id` (Rails' `start:`, exclusive), or from the first row with `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[derive(serde::Deserialize)] struct Post { id: i64 }
    /// let batches = ocre::Query::<Post>::table("posts").batches(100, |p| p.id).resume_after(Some(41));
    /// assert_eq!(batches.after(), Some(41));
    /// assert!(batches.statement().sql.contains("posts.id > ?1"));
    /// ```
    pub fn resume_after(mut self, id: Option<i64>) -> Self {
        self.after = id;
        self
    }

    /// The last id read so far (`None` before the first batch).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[derive(serde::Deserialize)] struct Post { id: i64 }
    /// assert_eq!(ocre::Query::<Post>::table("posts").batches(10, |p| p.id).after(), None);
    /// ```
    pub fn after(&self) -> Option<i64> {
        self.after
    }

    /// Whether the last batch was shorter than the batch size: no row is left.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[derive(serde::Deserialize)] struct Post { id: i64 }
    /// let mut batches = ocre::Query::<Post>::table("posts").batches(2, |p| p.id);
    /// batches.advance(&[Post { id: 1 }]);
    /// assert!(batches.is_done());
    /// ```
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The `SELECT` of the next batch.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[derive(serde::Deserialize)] struct Post { id: i64 }
    /// let batches = ocre::Query::<Post>::table("posts").limit(3).batches(10, |p| p.id);
    /// assert_eq!(batches.statement().sql, "SELECT * FROM posts ORDER BY posts.id ASC LIMIT ?1");
    /// ```
    pub fn statement(&self) -> Statement {
        let mut parts = self.query.parts.clone();
        parts.batch(self.after, self.size);
        parts.select_statement()
    }

    /// Records a batch just read: its last id, and whether it was the last batch.
    ///
    /// [`next`](Self::next) calls it; call it yourself after running
    /// [`statement`](Self::statement) through [`Db::all`](crate::Db::all).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[derive(serde::Deserialize)] struct Post { id: i64 }
    /// let mut batches = ocre::Query::<Post>::table("posts").batches(2, |p| p.id);
    /// batches.advance(&[Post { id: 3 }, Post { id: 5 }]);
    /// assert_eq!((batches.after(), batches.is_done()), (Some(5), false));
    /// ```
    pub fn advance(&mut self, rows: &[T]) {
        if let Some(last) = rows.last() {
            self.after = Some((self.id)(last));
        }
        self.done = (rows.len() as i64) < self.size;
    }
}

#[cfg(test)]
#[path = "../tests/query.rs"]
mod tests;
