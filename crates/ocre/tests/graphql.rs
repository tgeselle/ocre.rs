use async_graphql::{Context, EmptyMutation, EmptySubscription, Object};

use super::*;
use crate::support::{block_on, body_text};

struct Query;

#[Object]
impl Query {
    async fn greeting(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        Ok(format!("hello {}", ctx.data::<&'static str>()?))
    }

    async fn missing(&self) -> async_graphql::Result<i32> {
        Err(Error::NotFound)?
    }

    async fn invalid(&self) -> async_graphql::Result<i32> {
        Err(Error::bad_request("title is required"))?
    }

    async fn broken(&self) -> async_graphql::Result<i32> {
        Err(Error::internal("D1 query failed: secret"))?
    }

    async fn rejected(&self) -> async_graphql::Result<i32> {
        let mut v = crate::Validator::new();
        v.required("title", "").required("body", "");
        v.finish()?;
        Ok(1)
    }
}

fn run(body: &str) -> (u16, serde_json::Value) {
    let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
    let response = block_on(respond(&schema, body.as_bytes(), "ocre"));
    let status = response.status().as_u16();
    (status, serde_json::from_str(&body_text(response)).unwrap())
}

#[test]
fn executes_queries_with_request_data() {
    let (status, body) = run(r#"{"query": "{ greeting }"}"#);
    assert_eq!((status, body), (200, serde_json::json!({"data": {"greeting": "hello ocre"}})));
}

#[test]
fn ocre_errors_carry_a_status_and_hide_internals() {
    let (_, body) = run(r#"{"query": "{ missing }"}"#);
    assert_eq!(body["errors"][0]["message"], "Not found");
    assert_eq!(body["errors"][0]["extensions"]["status"], 404);
    let (_, body) = run(r#"{"query": "{ invalid }"}"#);
    assert_eq!(body["errors"][0]["message"], "title is required");
    assert_eq!(body["errors"][0]["extensions"]["status"], 400);
    let (_, body) = run(r#"{"query": "{ broken }"}"#);
    assert_eq!(body["errors"][0]["message"], "Internal server error");
    let (_, body) = run(r#"{"query": "{ rejected }"}"#);
    assert_eq!(body["errors"][0]["message"], "Validation failed");
    assert_eq!(body["errors"][0]["extensions"]["status"], 422);
    assert_eq!(
        body["errors"][0]["extensions"]["fields"],
        serde_json::json!({"title": ["can't be blank"], "body": ["can't be blank"]})
    );
}

#[test]
fn malformed_bodies_are_400s_in_graphql_error_format() {
    let (status, body) = run("not json");
    assert_eq!(status, 400);
    assert!(body["errors"][0]["message"].as_str().unwrap().starts_with("invalid GraphQL request"));
}

#[test]
fn graphiql_targets_the_endpoint() {
    let Html(page) = graphiql();
    assert!(page.contains("/graphql"));
}
