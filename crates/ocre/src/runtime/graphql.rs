use async_graphql::{ObjectType, Schema, SubscriptionType};
use axum::{Router, body::Bytes, extract::State, routing::get};

use super::Ctx;
use crate::graphql::{graphiql, respond};

/// `GET /graphql` (GraphiQL) and `POST /graphql`. `schema` returns a schema
/// built once per Worker instance (a `static LazyLock`), because building it
/// costs CPU.
pub fn routes<Q, M, S>(schema: fn() -> &'static Schema<Q, M, S>) -> Router<Ctx>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    Router::new().route(
        "/graphql",
        get(|| async { graphiql() })
            .post(move |State(ctx): State<Ctx>, body: Bytes| async move { respond(schema(), &body, ctx).await }),
    )
}
