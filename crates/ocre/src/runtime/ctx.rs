use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use axum::http::{HeaderMap, Method, header};
use serde::de::DeserializeOwned;
use worker::{D1Database, D1DatabaseSession, Env, js_sys::Array, send::SendWrapper};

use super::{Db, d1::Handle};
use crate::{
    Error, Result, cache::QUERY_CACHE_LIMIT, errors::Reporter, events::Events, instrument::Timings, log::Logger,
};

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
/// A `Ctx` and its clones share two per-request memories, like Rails' query
/// cache and local cache: identical `SELECT`s through [`db`](Self::db) are
/// answered from memory until a write (see [`Db`]), and `ocre::cache`
/// reads each KV key at most once. Each request, queued job and cron run
/// gets its own.
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
    memo: Arc<Memo>,
    log: Logger,
    errors: Reporter,
    events: Events,
    timings: Timings,
}

/// What one request remembers: KV texts read or written (the local cache),
/// `SELECT` results (the query cache) and, with read replicas on, its D1
/// sessions.
#[derive(Default)]
pub(crate) struct Memo {
    kv: Mutex<HashMap<String, Option<String>>>,
    queries: Mutex<HashMap<String, SendWrapper<Array>>>,
    replicas: Mutex<Option<Replicas>>,
}

/// The D1 sessions of a request ([`crate::replicas`]).
struct Replicas {
    method: Method,
    /// The request's `Cookie` headers, holding the visitor's bookmarks.
    cookies: HeaderMap,
    /// One session per binding, opened on first use.
    sessions: Vec<(String, Arc<D1DatabaseSession>)>,
    /// Bindings that ran a statement that may write.
    wrote: Vec<String>,
}

impl Memo {
    /// The text of a KV key this request already read or wrote.
    pub(crate) fn kv(&self, key: &str) -> Option<Option<String>> {
        self.kv.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }

    pub(crate) fn remember_kv(&self, key: &str, text: Option<String>) {
        self.kv.lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_owned(), text);
    }

    /// The rows of a `SELECT` this request already ran.
    pub(crate) fn query(&self, key: &str) -> Option<Array> {
        self.queries.lock().unwrap_or_else(PoisonError::into_inner).get(key).map(|rows| Array::clone(rows))
    }

    pub(crate) fn remember_query(&self, key: String, rows: Array) {
        let mut queries = self.queries.lock().unwrap_or_else(PoisonError::into_inner);
        if queries.len() >= QUERY_CACHE_LIMIT {
            queries.clear();
        }
        queries.insert(key, SendWrapper::new(rows));
    }

    /// Forgets every `SELECT` result, after a statement that may write.
    pub(crate) fn clear_queries(&self) {
        self.queries.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    /// Queries of this request go through D1 sessions ([`crate::replicas`]).
    pub(crate) fn start_replicas(&self, method: &Method, headers: &HeaderMap) {
        let mut cookies = HeaderMap::new();
        for value in headers.get_all(header::COOKIE) {
            cookies.append(header::COOKIE, value.clone());
        }
        let replicas = Replicas { method: method.clone(), cookies, sessions: Vec::new(), wrote: Vec::new() };
        *self.replicas.lock().unwrap_or_else(PoisonError::into_inner) = Some(replicas);
    }

    /// The request's session on `binding` (opened on first use), or `None`
    /// when replicas are off: then queries go to the database itself.
    fn session(&self, binding: &str, db: &D1Database) -> Option<worker::Result<Arc<D1DatabaseSession>>> {
        let mut replicas = self.replicas.lock().unwrap_or_else(PoisonError::into_inner);
        let replicas = replicas.as_mut()?;
        if let Some((_, session)) = replicas.sessions.iter().find(|(name, _)| name == binding) {
            return Some(Ok(Arc::clone(session)));
        }
        let start = crate::replicas::session_start(&replicas.cookies, &replicas.method, binding);
        Some(db.with_session(Some(&start)).map(|session| {
            let session = Arc::new(session);
            replicas.sessions.push((binding.to_owned(), Arc::clone(&session)));
            session
        }))
    }

    /// Notes that `binding` ran a statement that may write.
    pub(crate) fn wrote(&self, binding: &str) {
        if let Some(replicas) = self.replicas.lock().unwrap_or_else(PoisonError::into_inner).as_mut()
            && !replicas.wrote.iter().any(|name| name == binding)
        {
            replicas.wrote.push(binding.to_owned());
        }
    }

    /// The bookmark of each session that wrote, for the response's cookies.
    pub(crate) fn bookmarks(&self) -> Vec<(String, String)> {
        let replicas = self.replicas.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(replicas) = replicas.as_ref() else { return Vec::new() };
        let wrote = replicas.sessions.iter().filter(|(name, _)| replicas.wrote.contains(name));
        wrote.filter_map(|(name, session)| Some((name.clone(), session.get_bookmark().ok()??))).collect()
    }
}

impl Ctx {
    pub(crate) fn new(env: Env) -> Self {
        // Once per Worker instance: the keys of encrypted model columns.
        crate::encryption::ensure_installed(&|name| env.secret(name).ok().map(|secret| secret.to_string()));
        let var = |name: &str| env.var(name).ok().map(|var| var.to_string());
        crate::log::configure(var(crate::log::LOG_LEVEL).as_deref(), var(crate::log::LOG_FORMAT).as_deref());
        install_panic_hook();
        let log = Logger::new();
        Self {
            env: SendWrapper::new(env),
            memo: Arc::default(),
            errors: Reporter::new(log.clone()),
            events: Events::new(log.clone()),
            log,
            timings: Timings::default(),
        }
    }

    /// The same context whose log lines and error reports carry `log`'s fields (the request id...).
    pub(crate) fn with_log(mut self, log: Logger) -> Self {
        self.errors = Reporter::new(log.clone());
        self.events = Events::new(log.clone());
        self.log = log;
        self
    }

    /// The same environment with empty per-request memories, e.g. for each job of a queue batch.
    pub(crate) fn fresh(&self) -> Self {
        Self { memo: Arc::default(), ..self.clone() }
    }

    pub(crate) fn memo(&self) -> &Arc<Memo> {
        &self.memo
    }

    pub(crate) fn timings(&self) -> &Timings {
        &self.timings
    }

    /// The logger of this request, job batch or cron run (see [`ocre::log`](crate::log)).
    ///
    /// In a request its lines carry `request_id`, `method` and `path`, the
    /// same request id as the [`RequestId`](crate::RequestId) extractor and
    /// the `X-Request-Id` response header. Add fields with
    /// [`Logger::with`]. No binding call; each line is one Workers Logs event.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::Ctx;
    ///
    /// async fn import(State(ctx): State<Ctx>) -> &'static str {
    ///     let log = ctx.log().with("import_id", 12);
    ///     log.info("import started");
    ///     log.warn(format_args!("{} rows skipped", 3));
    ///     "OK"
    /// }
    /// # let _ = import;
    /// ```
    pub fn log(&self) -> &Logger {
        &self.log
    }

    /// The error reporter of this request, job batch or cron run (see [`ocre::errors`](crate::errors)).
    ///
    /// Reports are logged at once and sent to the registered subscribers
    /// when the response is ready (or the job or cron run ends).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result};
    ///
    /// async fn refresh(State(ctx): State<Ctx>) -> Result<&'static str> {
    ///     ctx.errors().set_context("feed", "rss");
    ///     let body: Option<String> = ctx.errors().handle("<rss/>".parse::<String>());
    ///     Ok(if body.is_some() { "refreshed" } else { "kept the old feed" })
    /// }
    /// # let _ = refresh;
    /// ```
    pub fn errors(&self) -> &Reporter {
        &self.errors
    }

    /// Structured events of this request or job (Rails' `Rails.event`), see [`ocre::events`](crate::events).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::Ctx;
    ///
    /// async fn signup(State(ctx): State<Ctx>) -> &'static str {
    ///     ctx.events().notify("user.signed_up", serde_json::json!({ "plan": "free" }));
    ///     "welcome"
    /// }
    /// # let _ = signup;
    /// ```
    pub fn events(&self) -> &Events {
        &self.events
    }

    /// The app's settings, read from Worker variables and secrets into `T` (see [`ocre::config`](crate::config)).
    ///
    /// Each field reads the variable of the same name in upper case, or
    /// else the secret. No binding call: one environment lookup per field.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] (500, logged) naming the variable when a required
    /// one is missing or does not convert to its field's type.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use axum::extract::State;
    /// use ocre::{Ctx, Result};
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Settings {
    ///     support_email: String,
    ///     max_uploads: Option<u32>,
    /// }
    ///
    /// async fn contact(State(ctx): State<Ctx>) -> Result<String> {
    ///     let settings: Settings = ctx.config()?;
    ///     Ok(settings.support_email)
    /// }
    /// # let _ = contact;
    /// ```
    pub fn config<T: DeserializeOwned>(&self) -> Result<T> {
        crate::config::from_lookup(&|name| super::errors::lookup(&self.env, name))
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
        let handle = self.env.d1(binding).and_then(|db| match self.memo.session(binding, &db) {
            Some(session) => session.map(Handle::Session),
            None => Ok(Handle::Database(Arc::new(db))),
        });
        let db = handle.map(|handle| Db::new(handle, binding, Some(Arc::clone(&self.memo))));
        db.map(|db| db.probed(self.log.clone(), self.timings.clone())).map_err(|err| {
            Error::internal(format!(
                "D1 binding `{binding}` is missing ({err}). Fix: add `{binding}: bindings.d1({{ name: \"<database>\" }}),` to worker.env in cloudflare.config.ts"
            ))
        })
    }
}

/// Logs panics as `error` lines with their location: on Workers a panic
/// otherwise ends the request with only `RuntimeError: unreachable`.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let payload = info.payload();
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("Box<dyn Any>");
            let location = info.location().map(|at| (at.file(), at.line(), at.column()));
            Logger::new().error(crate::log::panic_line(message, location));
        }));
    });
}
