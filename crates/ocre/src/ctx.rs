use worker::{Env, send::SendWrapper};

use crate::{Db, Error, Result};

/// Name of the D1 binding every Ocre app uses for its main database.
pub(crate) const DB_BINDING: &str = "DB";

/// Per-request application context, available in handlers as
/// `State(ctx): State<Ctx>`.
#[derive(Clone)]
pub struct Ctx {
    env: SendWrapper<Env>,
}

impl Ctx {
    pub(crate) fn new(env: Env) -> Self {
        Self { env: SendWrapper::new(env) }
    }

    /// The raw Workers environment, for bindings Ocre does not wrap yet.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The application database (D1 binding `DB`).
    pub fn db(&self) -> Result<Db> {
        self.env.d1(DB_BINDING).map(Db::new).map_err(|err| {
            Error::internal(format!(
                "D1 binding `{DB_BINDING}` is missing ({err}). Fix: add a [[d1_databases]] entry with binding = \"{DB_BINDING}\" to wrangler.toml"
            ))
        })
    }
}
