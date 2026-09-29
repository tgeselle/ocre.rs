use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;
use worker::{
    D1Database, D1PreparedStatement,
    send::{SendFuture, SendWrapper},
};

use crate::{Error, Result};

/// Handle to the application's D1 database.
///
/// Every method returns a `Send` future, so plain axum handlers can await it.
/// Always pass values through `params![...]` and `?1, ?2` placeholders, never
/// by formatting them into the SQL string.
pub struct Db {
    inner: SendWrapper<D1Database>,
}

impl Db {
    pub(crate) fn new(db: D1Database) -> Self {
        Self { inner: SendWrapper::new(db) }
    }

    /// Returns every row, deserialized into `T`.
    pub fn all<'q, T: DeserializeOwned>(
        &self,
        sql: &'q str,
        params: Vec<Param>,
    ) -> impl Future<Output = Result<Vec<T>>> + Send + use<'q, T> {
        let stmt = self.prepare(sql, params);
        SendFuture::new(async move {
            let rows = stmt?.all().await.map_err(|err| query_error(sql, err))?;
            rows.results::<T>().map_err(|err| query_error(sql, err))
        })
    }

    /// Returns the first row, if any. Use with `INSERT ... RETURNING *` to get
    /// the created row back.
    pub fn first<'q, T: DeserializeOwned>(
        &self,
        sql: &'q str,
        params: Vec<Param>,
    ) -> impl Future<Output = Result<Option<T>>> + Send + use<'q, T> {
        let stmt = self.prepare(sql, params);
        SendFuture::new(async move { stmt?.first::<T>(None).await.map_err(|err| query_error(sql, err)) })
    }

    /// Runs a statement that returns no rows and gives the number of rows changed.
    pub fn execute<'q>(
        &self,
        sql: &'q str,
        params: Vec<Param>,
    ) -> impl Future<Output = Result<usize>> + Send + use<'q> {
        let stmt = self.prepare(sql, params);
        SendFuture::new(async move {
            let result = stmt?.run().await.map_err(|err| query_error(sql, err))?;
            let meta = result.meta().map_err(|err| query_error(sql, err))?;
            Ok(meta.and_then(|m| m.changes).unwrap_or(0))
        })
    }

    fn prepare(&self, sql: &str, params: Vec<Param>) -> Result<D1PreparedStatement> {
        let values: Vec<JsValue> = params.into_iter().map(|p| p.0).collect();
        self.inner.prepare(sql).bind(&values).map_err(|err| query_error(sql, err))
    }
}

fn query_error(sql: &str, err: worker::Error) -> Error {
    Error::internal(format!("D1 query failed: {err}. SQL: {sql}"))
}

/// A value bound to a `?N` placeholder. Build with [`params!`](crate::params).
pub struct Param(JsValue);

/// Types that can be bound as query parameters.
pub trait IntoParam {
    fn into_param(self) -> Param;
}

impl IntoParam for Param {
    fn into_param(self) -> Param {
        self
    }
}

impl IntoParam for &str {
    fn into_param(self) -> Param {
        Param(JsValue::from_str(self))
    }
}

impl IntoParam for String {
    fn into_param(self) -> Param {
        Param(JsValue::from(self))
    }
}

impl IntoParam for &String {
    fn into_param(self) -> Param {
        Param(JsValue::from_str(self))
    }
}

/// SQLite has no boolean type: `true`/`false` are stored as INTEGER 1/0.
/// Read them back with `#[serde(deserialize_with = "ocre::bool_from_sql")]`.
impl IntoParam for bool {
    fn into_param(self) -> Param {
        Param(JsValue::from_f64(if self { 1.0 } else { 0.0 }))
    }
}

impl IntoParam for f64 {
    fn into_param(self) -> Param {
        Param(JsValue::from_f64(self))
    }
}

macro_rules! lossless_number_param {
    ($($ty:ty),*) => {$(
        impl IntoParam for $ty {
            fn into_param(self) -> Param {
                Param(JsValue::from_f64(f64::from(self)))
            }
        }
    )*};
}

lossless_number_param!(i8, i16, i32, u8, u16, u32);

/// D1 numbers are JavaScript numbers, exact up to 2^53. Larger values are bound
/// as decimal text, which SQLite converts back for INTEGER columns.
impl IntoParam for i64 {
    fn into_param(self) -> Param {
        const MAX_SAFE: i64 = (1 << 53) - 1;
        if (-MAX_SAFE..=MAX_SAFE).contains(&self) {
            Param(JsValue::from_f64(self as f64))
        } else {
            Param(JsValue::from(self.to_string()))
        }
    }
}

impl<T: IntoParam> IntoParam for Option<T> {
    fn into_param(self) -> Param {
        self.map_or(Param(JsValue::NULL), IntoParam::into_param)
    }
}

/// Deserializes a SQLite boolean column (INTEGER 0/1) into `bool`:
/// `#[serde(deserialize_with = "ocre::bool_from_sql")]`.
pub fn bool_from_sql<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    struct Visitor;

    impl serde::de::Visitor<'_> for Visitor {
        type Value = bool;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a boolean or the integer 0 or 1")
        }

        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<bool, E> {
            Ok(v != 0)
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<bool, E> {
            Ok(v != 0)
        }

        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<bool, E> {
            Ok(v != 0.0)
        }
    }

    deserializer.deserialize_any(Visitor)
}
