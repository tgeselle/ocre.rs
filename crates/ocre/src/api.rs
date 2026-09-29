//! JSON APIs: errors as JSON, a JSON body extractor whose failures are JSON
//! too, and `?limit=&offset=` pagination.

use axum::{
    extract::{FromRequest, FromRequestParts, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::Error;

/// Handler error for JSON endpoints:
/// `{"error": {"status": 404, "message": "Not found"}}`.
/// Converts from [`Error`], so `?` works on everything Ocre returns.
#[derive(Debug)]
pub struct ApiError(pub Error);

pub type ApiResult<T> = Result<T, ApiError>;

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        Self(err)
    }
}

impl From<worker::Error> for ApiError {
    fn from(err: worker::Error) -> Self {
        Self(err.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = self.0.into_public();
        let body = serde_json::json!({ "error": { "status": status.as_u16(), "message": message } });
        (status, axum::Json(body)).into_response()
    }
}

/// JSON request body or response. Unlike `axum::Json`, a body that is not
/// valid JSON for `T` is answered with a JSON 400 error.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError(Error::bad_request(rejection.body_text()))),
        }
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `201 Created` with a JSON body.
pub struct Created<T>(pub T);

impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, axum::Json(self.0)).into_response()
    }
}

/// `?limit=&offset=` for list endpoints. `limit` defaults to 50 and is at
/// most 100, to keep each request within the free plan's D1 read budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

impl Page {
    pub const DEFAULT_LIMIT: i64 = 50;
    pub const MAX_LIMIT: i64 = 100;

    /// Checks the bounds: `1..=100` for `limit`, `0..` for `offset`.
    pub fn new(limit: i64, offset: i64) -> Result<Self, Error> {
        if !(1..=Self::MAX_LIMIT).contains(&limit) {
            return Err(Error::bad_request(format!("limit must be between 1 and {}", Self::MAX_LIMIT)));
        }
        if offset < 0 {
            return Err(Error::bad_request("offset must be 0 or more"));
        }
        Ok(Self { limit, offset })
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Page {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        #[derive(Deserialize)]
        struct Raw {
            limit: Option<i64>,
            offset: Option<i64>,
        }
        let Query(raw) = Query::<Raw>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| ApiError(Error::bad_request(rejection.body_text())))?;
        Ok(Self::new(raw.limit.unwrap_or(Self::DEFAULT_LIMIT), raw.offset.unwrap_or(0))?)
    }
}

#[cfg(test)]
mod tests;
