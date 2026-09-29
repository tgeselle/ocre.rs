use async_graphql::{ObjectType, Schema, SubscriptionType};
use axum::{Router, body::Bytes, extract::State, http::header, routing::get};

use super::Ctx;
use crate::graphql::{graphiql, respond};

/// GraphiQL loads React and its editor from unpkg.com and starts with an
/// inline script: its page gets this policy instead of the app's
/// `ocre::security::ContentSecurityPolicy` (a handler's header wins).
const GRAPHIQL_CSP: &str = "default-src 'self'; script-src 'self' https://unpkg.com 'unsafe-inline'; \
                            style-src 'self' https://unpkg.com 'unsafe-inline'; img-src 'self' data: https:; \
                            font-src 'self' data: https://unpkg.com; connect-src 'self'";

/// Returns a router serving GraphiQL on `GET /graphql` and queries on `POST /graphql`.
///
/// `schema` returns a schema built once per Worker instance (a `static
/// LazyLock`), because building it costs CPU (part of the 20-60 ms a new
/// instance spends with GraphQL, against the free plan's 10 ms per request).
/// `POST` runs [`respond`](crate::graphql::respond) with the request's
/// [`Ctx`](crate::Ctx) as resolver data, so resolvers reach D1 and the other
/// bindings through `ctx.data::<ocre::Ctx>()?`. `GET` serves
/// [`graphiql`](crate::graphql::graphiql). Merge it into the app's router.
///
/// # Examples
///
/// ```
/// use std::sync::LazyLock;
/// use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
/// use axum::Router;
/// use ocre::Ctx;
///
/// struct Query;
///
/// #[Object]
/// impl Query {
///     async fn version(&self) -> &'static str {
///         "1"
///     }
/// }
///
/// static SCHEMA: LazyLock<Schema<Query, EmptyMutation, EmptySubscription>> =
///     LazyLock::new(|| Schema::new(Query, EmptyMutation, EmptySubscription));
///
/// let _app: Router<Ctx> = Router::new().merge(ocre::graphql::routes(|| &*SCHEMA));
/// ```
pub fn routes<Q, M, S>(schema: fn() -> &'static Schema<Q, M, S>) -> Router<Ctx>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    Router::new().route(
        "/graphql",
        get(|| async { ([(header::CONTENT_SECURITY_POLICY, GRAPHIQL_CSP)], graphiql()) })
            .post(move |State(ctx): State<Ctx>, body: Bytes| async move { respond(schema(), &body, ctx).await }),
    )
}
