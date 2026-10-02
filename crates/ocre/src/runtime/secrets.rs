//! Secrets wherever they live: a Worker secret (or `.dev.vars` value), or a
//! Secrets Store secret bound to the Worker.

use worker::{Env, SecretStore, send::SendFuture, wasm_bindgen::JsValue};

use crate::{Error, Result};

/// The value of the secret bound as `name`, `None` when the Worker has no
/// such binding. A Worker secret, variable or `.dev.vars` value is a string;
/// a Secrets Store binding is read with one call to the account's store.
pub(crate) async fn lookup(env: &Env, name: &str) -> Result<Option<String>> {
    let binding = worker::js_sys::Reflect::get(env, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED);
    if binding.is_undefined() || binding.is_null() {
        return Ok(None);
    }
    if let Some(text) = binding.as_string() {
        return Ok(Some(text));
    }
    match SendFuture::new(SecretStore::from(binding).get()).await {
        Ok(value) => Ok(value),
        // The binding exists but the store has no such secret (workerd throws "Secret ... not found").
        Err(err) if err.to_string().contains("not found") => Ok(None),
        Err(err) => Err(Error::internal(format!(
            "the Secrets Store secret bound as {name} could not be read: {err}. Fix: in `ocre dev`, put \
             {name}=<value> in .dev.vars (Ocre copies it into the local store); in production, check the \
             secret exists in the store named by its binding in cloudflare.config.ts (`ocre secrets list`)"
        ))),
    }
}

/// [`lookup`], with an error naming the fix when the secret is missing.
pub(crate) async fn require(env: &Env, name: &str) -> Result<String> {
    lookup(env, name).await?.filter(|value| !value.is_empty()).ok_or_else(|| {
        Error::internal(format!(
            "the {name} secret is not set. Fix: put it in .dev.vars for `ocre dev`, and for production run \
             `ocre secrets push {name} --file .prod.vars` (this Worker only) or `ocre secrets push {name} --store \
             --file .prod.vars` (the account's Secrets Store, shared by every Worker that binds it)"
        ))
    })
}
