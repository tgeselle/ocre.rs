use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;
use worker::{
    D1Database, D1PreparedStatement,
    send::{SendFuture, SendWrapper},
};

use crate::{Error, Param, Result, sql::Value};

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
        let values: Vec<JsValue> = params.into_iter().map(to_js).collect();
        self.inner.prepare(sql).bind(&values).map_err(|err| query_error(sql, err))
    }
}

fn to_js(param: Param) -> JsValue {
    match param.0 {
        Value::Null => JsValue::NULL,
        Value::Number(n) => JsValue::from_f64(n),
        Value::Text(s) => JsValue::from(s),
    }
}

fn query_error(sql: &str, err: worker::Error) -> Error {
    Error::internal(format!("D1 query failed: {err}. SQL: {sql}"))
}
