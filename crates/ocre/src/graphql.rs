//! GraphQL support (feature `graphql`).
//!
//! Built on [async-graphql](https://docs.rs/async-graphql): [`routes`] serves
//! `POST /graphql` and GraphiQL on `GET /graphql`; `ocre g api Post ...
//! --graphql` generates `posts(limit, offset)`, `post(id)`, `createPost`,
//! `updatePost` and `deletePost` resolvers that call the model, so they share
//! its rules with the JSON API.
//!
//! Opt-in because it costs on the free plan: the WebAssembly binary grows by
//! about 1.1 MB and each new Worker instance spends 20-60 ms of CPU loading
//! it and building the schema (measured with `wrangler tail`; the free plan
//! allows 10 ms per request, with tolerance for infrequent overruns). Build
//! the schema once per instance and keep resolvers thin; each resolver's D1
//! queries cost the same rows read and written as in a JSON handler.
//!
//! Resolvers get the request context with `ctx.data::<ocre::Ctx>()?`, and
//! Ocre errors convert with `?`: clients see `Not found` or the bad-request
//! message with `extensions.status` (and `extensions.fields` for validation
//! errors), never internal details, which are logged.
//!
//! ```no_run
//! use std::sync::LazyLock;
//! use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema};
//! use axum::Router;
//! use ocre::{Ctx, Error};
//!
//! struct Query;
//!
//! #[Object]
//! impl Query {
//!     async fn post_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
//!         let db = ctx.data::<Ctx>()?.db()?;
//!         # let _ = db;
//!         // ... SELECT COUNT(*) FROM posts
//!         Err(Error::NotFound.into())
//!     }
//! }
//!
//! type AppSchema = Schema<Query, EmptyMutation, EmptySubscription>;
//! static SCHEMA: LazyLock<AppSchema> = LazyLock::new(|| Schema::new(Query, EmptyMutation, EmptySubscription));
//!
//! fn routes() -> Router<Ctx> {
//!     Router::new().merge(ocre::graphql::routes(|| &*SCHEMA))
//! }
//! # let _ = routes;
//! ```

use std::any::Any;

use async_graphql::{ErrorExtensions, ObjectType, Schema, SubscriptionType};
use axum::{
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};

use crate::Error;

pub use crate::runtime::graphql_routes as routes;
pub use async_graphql;

impl From<Error> for async_graphql::Error {
    fn from(err: Error) -> Self {
        let public = err.into_public();
        let fields = (!public.fields.is_empty()).then(|| public.fields_json());
        async_graphql::Error::new(public.message).extend_with(|_, extensions| {
            extensions.set("status", public.status.as_u16());
            if let Some(fields) = fields {
                extensions.set("fields", async_graphql::Value::from_json(fields).expect("field errors are plain JSON"));
            }
        })
    }
}

/// Renders GraphiQL, the in-browser query editor, pointed at `/graphql`.
///
/// [`routes`] serves it on `GET /graphql`; use it directly to mount the
/// editor elsewhere or behind a check. The page loads GraphiQL's scripts
/// from a CDN.
///
/// # Examples
///
/// ```
/// let page = ocre::graphql::graphiql();
/// assert!(page.0.contains("/graphql"));
/// ```
pub fn graphiql() -> Html<String> {
    Html(async_graphql::http::GraphiQLSource::build().endpoint("/graphql").finish())
}

/// Executes a `POST /graphql` JSON body against `schema` and returns the JSON response.
///
/// `data` is available to resolvers through `ctx.data::<T>()` ([`routes`]
/// passes the [`Ctx`](crate::Ctx)). A body that is not a GraphQL request
/// (`{"query": ..., "variables": ...}`) gets a 400 with
/// `{"errors": [{"message": "invalid GraphQL request: ..."}]}`. Otherwise the
/// status is 200 and resolver errors are in the body's `errors`, as GraphQL
/// expects.
///
/// # Examples
///
/// ```
/// use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
///
/// struct Query;
///
/// #[Object]
/// impl Query {
///     async fn hello(&self) -> &'static str {
///         "world"
///     }
/// }
///
/// let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
/// let response = pollster::block_on(ocre::graphql::respond(&schema, br#"{"query": "{ hello }"}"#, ()));
/// assert_eq!(response.status(), 200);
/// let body = pollster::block_on(axum::body::to_bytes(response.into_body(), usize::MAX)).unwrap();
/// assert_eq!(body, r#"{"data":{"hello":"world"}}"#);
///
/// let response = pollster::block_on(ocre::graphql::respond(&schema, b"not json", ()));
/// assert_eq!(response.status(), 400);
/// ```
pub async fn respond<Q, M, S>(schema: &Schema<Q, M, S>, body: &[u8], data: impl Any + Send + Sync) -> Response
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    let request: async_graphql::Request = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => {
            let body = serde_json::json!({ "errors": [{ "message": format!("invalid GraphQL request: {err}") }] });
            return (StatusCode::BAD_REQUEST, axum::Json(body)).into_response();
        }
    };
    let response = schema.execute(request.data(data)).await;
    let body = serde_json::to_string(&response).expect("GraphQL responses serialize");
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

#[cfg(test)]
#[path = "../tests/graphql.rs"]
mod tests;
