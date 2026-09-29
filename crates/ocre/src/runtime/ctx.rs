use worker::{Env, send::SendWrapper};

use super::Db;
use crate::{Error, Result};

/// Name of the D1 binding every Ocre app uses for its main database.
const DB_BINDING: &str = "DB";

/// Per-request application context: the Worker environment with typed access to its bindings.
///
/// [`serve`](crate::serve) builds one per request and passes it to the router
/// as axum state, so handlers take `State(ctx): State<Ctx>`. Queue consumers,
/// cron runs and inbound email handlers receive one too. Cloning is cheap (the
/// environment is a JavaScript handle) and the type is `Send`, so it can be
/// held across `.await` in plain axum handlers.
///
/// Creating a `Ctx` costs nothing on the free plan; each binding call
/// ([`db`](Self::db), `ocre::cache`, `ocre::storage`...) only looks the
/// binding up.
///
/// # Examples
///
/// ```no_run
/// use axum::{Router, extract::State, routing::get};
/// use ocre::{ApiResult, Ctx};
///
/// async fn count(State(ctx): State<Ctx>) -> ApiResult<String> {
///     let db = ctx.db()?;
///     let rows: Vec<serde_json::Value> = db.all("SELECT COUNT(*) AS n FROM posts", ocre::params![]).await?;
///     Ok(rows[0]["n"].to_string())
/// }
///
/// fn routes() -> Router<Ctx> {
///     Router::new().route("/count", get(count))
/// }
/// # let _ = routes;
/// ```
#[derive(Clone)]
pub struct Ctx {
    env: SendWrapper<Env>,
}

impl Ctx {
    pub(crate) fn new(env: Env) -> Self {
        // Once per Worker instance: the keys of encrypted model columns.
        crate::encryption::ensure_installed(&|name| env.secret(name).ok().map(|secret| secret.to_string()));
        Self { env: SendWrapper::new(env) }
    }

    /// The raw Workers environment, for bindings Ocre does not wrap yet.
    ///
    /// Use it for vars, secrets and bindings such as AI or Vectorize; prefer
    /// the Ocre helpers when one exists, since their errors name the
    /// cloudflare.config.ts fix.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result};
    ///
    /// async fn app_name(State(ctx): State<Ctx>) -> Result<String> {
    ///     let name = ctx.env().var("APP_NAME")?;
    ///     Ok(name.to_string())
    /// }
    /// # let _ = app_name;
    /// ```
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The application database: the D1 binding `DB`.
    ///
    /// Looking the binding up runs no query and costs no D1 rows; see [`Db`]
    /// for what each query reads and writes.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged) when the Worker has no `DB` binding;
    /// the message says to add `DB: bindings.d1({ name: "<app>" })` to
    /// cloudflare.config.ts.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result};
    ///
    /// async fn handler(State(ctx): State<Ctx>) -> Result<String> {
    ///     let db = ctx.db()?;
    ///     let changed = db.execute("DELETE FROM sessions WHERE expires_at < ?1", ocre::params![ocre::now()]).await?;
    ///     Ok(format!("{changed} expired"))
    /// }
    /// # let _ = handler;
    /// ```
    pub fn db(&self) -> Result<Db> {
        self.db_named(DB_BINDING)
    }

    /// Another D1 database of the app, by its binding name (Rails' multiple databases).
    ///
    /// Each database is a `KEY: bindings.d1({ name: "..." })` entry in
    /// cloudflare.config.ts with its own binding key and database name (see the Models
    /// guide, "Several databases"). Queries cannot join across databases:
    /// load ids from one, then `find_many` in the other.
    ///
    /// # Free plan
    ///
    /// Up to 10 databases per account, 5 GB of storage and the daily row
    /// quotas shared by all of them.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged) when the Worker has no D1 binding
    /// named `binding`; the message names the cloudflare.config.ts entry to add.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result, params};
    ///
    /// async fn track(State(ctx): State<Ctx>) -> Result<()> {
    ///     let analytics = ctx.db_named("ANALYTICS")?;
    ///     analytics.execute("INSERT INTO page_views (path) VALUES (?1)", params!["/"]).await?;
    ///     Ok(())
    /// }
    /// # let _ = track;
    /// ```
    pub fn db_named(&self, binding: &str) -> Result<Db> {
        self.env.d1(binding).map(Db::new).map_err(|err| {
            Error::internal(format!(
                "D1 binding `{binding}` is missing ({err}). Fix: add `{binding}: bindings.d1({{ name: \"<database>\" }}),` to worker.env in cloudflare.config.ts"
            ))
        })
    }
}
