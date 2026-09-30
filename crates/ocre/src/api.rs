//! JSON APIs: errors as JSON, a JSON body extractor whose failures are JSON
//! too, and `?limit=&offset=` pagination.

use axum::{
    extract::{FromRequest, FromRequestParts, Query, Request},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, IntoResponseParts, Response, ResponseParts},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::Error;

/// Handler error for JSON endpoints: an [`Error`] rendered as a JSON body.
///
/// The body is `{"error": {"status": 404, "message": "Not found"}}`; a 422
/// adds `"fields": {"title": ["can't be blank"]}` (the shape Rails APIs use),
/// and a 401 adds the `WWW-Authenticate: Bearer` header. Internal messages are
/// logged and answered as `Internal server error`. Converts from [`Error`]
/// and [`worker::Error`], so `?` works on everything Ocre returns. It is also
/// the rejection of [`Json`] and [`Page`], and of [`Session`](crate::Session)
/// in API-only apps (without the `html` feature).
///
/// # Examples
///
/// ```
/// use axum::{http::StatusCode, response::IntoResponse};
/// use ocre::{ApiError, Error};
///
/// let response = ApiError(Error::NotFound).into_response();
/// assert_eq!(response.status(), StatusCode::NOT_FOUND);
/// let response = ApiError::from(Error::Unauthorized).into_response();
/// assert_eq!(response.headers()["www-authenticate"], "Bearer");
/// ```
#[derive(Debug)]
pub struct ApiError(pub Error);

/// `Result` for JSON handlers: errors become [`ApiError`] JSON responses.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::{Path, State};
/// use ocre::{ApiResult, Ctx, Json, OptionExt, params};
///
/// async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
///     let post = ctx.db()?.first("SELECT * FROM posts WHERE id = ?1", params![id]).await?.or_404()?;
///     Ok(Json(post))
/// }
/// # let _ = show;
/// ```
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
        let mut public = self.0.into_public();
        let internal = public.internal.take();
        let fields = (!public.fields.is_empty()).then(|| public.fields_json());
        let mut error = serde_json::json!({ "status": public.status.as_u16(), "message": public.message });
        if let Some(fields) = fields {
            error["fields"] = fields;
        }
        let mut response = (public.status, axum::Json(serde_json::json!({ "error": error }))).into_response();
        if public.status == StatusCode::UNAUTHORIZED {
            // RFC 9110: a 401 names the scheme the client should use.
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        crate::error::mark(internal, &mut response);
        response
    }
}

/// JSON request body extractor and response, whose failures are JSON too.
///
/// As an extractor it requires `Content-Type: application/json`; unlike
/// `axum::Json`, a body that is missing that header or is not valid JSON for
/// `T` is answered with a JSON 400 ([`ApiError`] with
/// [`Error::BadRequest`] carrying axum's explanation). As a response it
/// serializes `T` with status 200 and `Content-Type: application/json`.
///
/// # Examples
///
/// ```
/// use axum::{http::StatusCode, response::IntoResponse};
/// use ocre::Json;
///
/// let response = Json(serde_json::json!({"id": 1})).into_response();
/// assert_eq!(response.status(), StatusCode::OK);
/// assert_eq!(response.headers()["content-type"], "application/json");
/// ```
///
/// In a handler:
///
/// ```no_run
/// use ocre::{ApiResult, Json};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Deserialize, Serialize)]
/// struct Echo {
///     message: String,
/// }
///
/// async fn echo(Json(body): Json<Echo>) -> ApiResult<Json<Echo>> {
///     Ok(Json(body))
/// }
/// # let _ = echo;
/// ```
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

/// `201 Created` response with a JSON body, for create endpoints.
///
/// # Examples
///
/// ```
/// use axum::{http::StatusCode, response::IntoResponse};
/// use ocre::Created;
///
/// let response = Created(serde_json::json!({"id": 7})).into_response();
/// assert_eq!(response.status(), StatusCode::CREATED);
/// assert_eq!(response.headers()["content-type"], "application/json");
/// ```
pub struct Created<T>(pub T);

impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, axum::Json(self.0)).into_response()
    }
}

/// `?limit=&offset=` pagination for list endpoints, as an extractor.
///
/// `limit` defaults to [`DEFAULT_LIMIT`](Self::DEFAULT_LIMIT) (50) and is at
/// most [`MAX_LIMIT`](Self::MAX_LIMIT) (100), to keep each request within the
/// free plan's D1 rows-read budget; `offset` defaults to 0. Out-of-range or
/// non-numeric values are rejected with a JSON 400 ([`ApiError`]). Bind both
/// fields as `LIMIT ?1 OFFSET ?2`.
///
/// # Examples
///
/// ```no_run
/// use axum::extract::State;
/// use ocre::{ApiResult, Ctx, Json, Page, params};
///
/// async fn index(State(ctx): State<Ctx>, page: Page) -> ApiResult<Json<Vec<serde_json::Value>>> {
///     let sql = "SELECT * FROM posts ORDER BY id DESC LIMIT ?1 OFFSET ?2";
///     Ok(Json(ctx.db()?.all(sql, params![page.limit, page.offset]).await?))
/// }
/// # let _ = index;
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Page {
    /// Rows to return, `1..=100`.
    pub limit: i64,
    /// Rows to skip, `0` or more.
    pub offset: i64,
}

impl Page {
    /// `limit` when the query string has none: 50.
    pub const DEFAULT_LIMIT: i64 = 50;
    /// Largest accepted `limit`: 100.
    pub const MAX_LIMIT: i64 = 100;

    /// Builds a page after checking the bounds: `1..=100` for `limit`, `0..` for `offset`.
    ///
    /// The extractor calls it; call it yourself where there is no query
    /// string, e.g. GraphQL arguments.
    ///
    /// # Errors
    ///
    /// [`Error::BadRequest`] (400) when `limit` is outside `1..=100`
    /// ("limit must be between 1 and 100") or `offset` is negative ("offset
    /// must be 0 or more").
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Page;
    ///
    /// assert_eq!(Page::new(20, 40).unwrap(), Page { limit: 20, offset: 40 });
    /// assert_eq!(Page::new(101, 0).unwrap_err().to_string(), "bad request: limit must be between 1 and 100");
    /// assert!(Page::new(10, -1).is_err());
    /// ```
    pub fn new(limit: i64, offset: i64) -> Result<Self, Error> {
        if !(1..=Self::MAX_LIMIT).contains(&limit) {
            return Err(Error::bad_request(format!("limit must be between 1 and {}", Self::MAX_LIMIT)));
        }
        if offset < 0 {
            return Err(Error::bad_request("offset must be 0 or more"));
        }
        Ok(Self { limit, offset })
    }

    /// The page after this one, or `None` when `returned` (the rows this page got) is less than `limit`.
    ///
    /// Ocre paginates without `COUNT(*)` (that query reads every row, and D1
    /// bills rows read), so "is there more?" is guessed from a full page: when
    /// the last page is exactly full, its "Next" link leads to an empty page.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Page;
    ///
    /// let page = Page::new(50, 0).unwrap();
    /// assert_eq!(page.next(50), Some(Page { limit: 50, offset: 50 }));
    /// assert_eq!(page.next(12), None);
    /// ```
    pub fn next(&self, returned: usize) -> Option<Self> {
        (i64::try_from(returned).is_ok_and(|returned| returned >= self.limit))
            .then_some(Self { limit: self.limit, offset: self.offset.saturating_add(self.limit) })
    }

    /// The page before this one, or `None` on the first page.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Page;
    ///
    /// assert_eq!(Page::new(50, 70).unwrap().previous(), Some(Page { limit: 50, offset: 20 }));
    /// assert_eq!(Page::new(50, 0).unwrap().previous(), None);
    /// ```
    pub fn previous(&self) -> Option<Self> {
        (self.offset > 0).then(|| Self { limit: self.limit, offset: (self.offset - self.limit).max(0) })
    }

    /// The query string of this page, without `?`: `offset=50`, with `limit=` first when it is not the default.
    ///
    /// Templates link to other pages with it:
    /// `{% if let Some(next) = page.next(posts.len()) %}<a href="?{{ next.query() }}">Next</a>{% endif %}`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::Page;
    ///
    /// assert_eq!(Page::new(50, 100).unwrap().query(), "offset=100");
    /// assert_eq!(Page::new(20, 40).unwrap().query(), "limit=20&offset=40");
    /// ```
    pub fn query(&self) -> String {
        if self.limit == Self::DEFAULT_LIMIT {
            format!("offset={}", self.offset)
        } else {
            format!("limit={}&offset={}", self.limit, self.offset)
        }
    }

    /// `Link` header (RFC 8288) pointing JSON clients at the `next`, `prev` and `first` pages of `path`.
    ///
    /// GitHub's API convention. Returns a response part: put it before the
    /// body in a tuple. `returned` is the number of rows this page got; see
    /// [`next`](Self::next). No header on a single page.
    ///
    /// # Examples
    ///
    /// ```
    /// use axum::response::IntoResponse;
    /// use ocre::{Json, Page};
    ///
    /// let page = Page::new(2, 2).unwrap();
    /// let posts = vec!["c", "d"];
    /// let response = (page.links("/api/posts", posts.len()), Json(posts)).into_response();
    /// assert_eq!(
    ///     response.headers()["link"],
    ///     r#"</api/posts?limit=2&offset=4>; rel="next", </api/posts?limit=2&offset=0>; rel="prev", </api/posts?limit=2&offset=0>; rel="first""#
    /// );
    /// ```
    pub fn links(&self, path: &str, returned: usize) -> PageLinks {
        let mut links = Vec::new();
        if let Some(next) = self.next(returned) {
            links.push(format!("<{path}?{}>; rel=\"next\"", next.query()));
        }
        if let Some(previous) = self.previous() {
            links.push(format!("<{path}?{}>; rel=\"prev\"", previous.query()));
            let first = Self { limit: self.limit, offset: 0 };
            links.push(format!("<{path}?{}>; rel=\"first\"", first.query()));
        }
        PageLinks((!links.is_empty()).then(|| links.join(", ")).and_then(|value| HeaderValue::from_str(&value).ok()))
    }
}

/// The `Link` header built by [`Page::links`], as a response part (nothing when there is no other page).
///
/// # Examples
///
/// ```
/// use axum::response::IntoResponse;
/// use ocre::Page;
///
/// let only_page = Page::new(50, 0).unwrap().links("/api/posts", 3);
/// let response = (only_page, "[]").into_response();
/// assert!(response.headers().get("link").is_none());
/// ```
#[derive(Debug, Clone)]
pub struct PageLinks(Option<HeaderValue>);

impl IntoResponseParts for PageLinks {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        if let Some(value) = self.0 {
            res.headers_mut().insert(header::LINK, value);
        }
        Ok(res)
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
#[path = "../tests/api.rs"]
mod tests;
