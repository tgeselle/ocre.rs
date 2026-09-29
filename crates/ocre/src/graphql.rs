//! GraphQL (feature `graphql`), built on async-graphql.
//!
//! Opt-in because it costs: the WebAssembly binary grows by about 1.1 MB and
//! each new Worker instance spends 20-60 ms of CPU loading it and building the
//! schema (measured on the free plan). Build the schema once per instance and
//! keep resolvers thin.
//!
//! Resolvers get the request context with `ctx.data::<ocre::Ctx>()?`, and
//! Ocre errors convert with `?`: clients see `Not found` or the bad-request
//! message with `extensions.status`, never internal details.

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

/// GraphiQL, the in-browser query editor, for `GET /graphql`.
pub fn graphiql() -> Html<String> {
    Html(async_graphql::http::GraphiQLSource::build().endpoint("/graphql").finish())
}

/// Executes a `POST /graphql` JSON body against `schema`, with `data`
/// available to resolvers through `ctx.data()`.
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
