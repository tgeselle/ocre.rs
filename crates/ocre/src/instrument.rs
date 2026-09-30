//! Per-request instrumentation (Rails' `ActiveSupport::Notifications` for
//! `sql.active_record`): the D1 statements a request ran and how long each
//! took, for the debug request line, the `Server-Timing` header of debug
//! builds and the development error page.
//!
//! Workers' clock only advances during I/O, so durations measure time spent
//! waiting for D1 (and other bindings), not CPU time; Workers Logs shows
//! each request's CPU time.

use std::sync::{Arc, Mutex, PoisonError};

/// Statements kept per request: enough for the error page, bounded memory.
const MAX_KEPT: usize = 100;

/// One timed D1 call.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Timing {
    pub sql: String,
    pub ms: f64,
}

#[derive(Debug, Default)]
struct State {
    count: usize,
    total_ms: f64,
    kept: Vec<Timing>,
}

/// Shared by a request's [`Ctx`](crate::Ctx) and every [`Db`](crate::Db) it hands out.
#[derive(Debug, Clone, Default)]
pub(crate) struct Timings(Arc<Mutex<State>>);

impl Timings {
    pub fn record(&self, sql: &str, ms: f64) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.count += 1;
        state.total_ms += ms;
        // Release builds only count: the text is for the development error page.
        if cfg!(debug_assertions) && state.kept.len() < MAX_KEPT {
            state.kept.push(Timing { sql: sql.to_owned(), ms });
        }
    }

    /// Statements run so far and their total duration.
    pub fn totals(&self) -> (usize, f64) {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (state.count, state.total_ms)
    }

    /// The first [`MAX_KEPT`] statements, in order.
    pub fn statements(&self) -> Vec<Timing> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).kept.clone()
    }

    /// `db;dur=12;desc="3 queries", total;dur=40` (W3C Server Timing), shown by
    /// the browser's developer tools under Network > Timing.
    pub fn server_timing(&self, total_ms: f64) -> String {
        let (queries, db_ms) = self.queries();
        format!("db;dur={db_ms};desc=\"{queries}\", total;dur={total_ms}")
    }

    /// `GET /posts 200 in 40 ms (db: 3 queries, 12 ms)`: the debug request line.
    pub fn summary(&self, method: &str, path: &str, status: u16, total_ms: f64) -> String {
        let (queries, db_ms) = self.queries();
        format!("{method} {path} {status} in {total_ms} ms (db: {queries}, {db_ms} ms)")
    }

    /// `1 query` or `3 queries`, and their total duration.
    fn queries(&self) -> (String, f64) {
        let (count, db_ms) = self.totals();
        (format!("{count} {}", if count == 1 { "query" } else { "queries" }), db_ms)
    }
}

#[cfg(test)]
#[path = "../tests/instrument.rs"]
mod tests;
