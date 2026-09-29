//! WebCrypto and the JavaScript clock.

use js_sys::{Array, ArrayBuffer, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;
use worker::send::SendFuture;

use crate::{Error, Result};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["crypto", "subtle"], js_name = importKey, catch)]
    fn import_key(
        format: &str,
        key_data: &Uint8Array,
        algorithm: &str,
        extractable: bool,
        usages: &Array,
    ) -> std::result::Result<Promise, JsValue>;

    #[wasm_bindgen(js_namespace = ["crypto", "subtle"], js_name = deriveBits, catch)]
    fn derive_bits(algorithm: &Object, key: &JsValue, length: u32) -> std::result::Result<Promise, JsValue>;
}

/// Milliseconds since the Unix epoch (`Date.now()`).
pub(crate) fn unix_millis() -> i64 {
    js_sys::Date::now() as i64
}

/// PBKDF2-HMAC-SHA256 through `crypto.subtle.deriveBits`, which runs natively
/// in workerd. Errors never include the password.
pub(crate) fn pbkdf2_sha256(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    bytes: usize,
) -> impl Future<Output = Result<Vec<u8>>> + Send + use<> {
    let (password, salt) = (password.to_vec(), salt.to_vec());
    SendFuture::new(async move {
        let failed = |step: &str, err: JsValue| {
            Error::internal(format!("PBKDF2 {step} failed in WebCrypto: {}", describe(&err)))
        };
        let usages = Array::of1(&JsValue::from_str("deriveBits"));
        let password = Uint8Array::from(password.as_slice());
        let key = import_key("raw", &password, "PBKDF2", false, &usages).map_err(|err| failed("importKey", err))?;
        let key = JsFuture::from(key).await.map_err(|err| failed("importKey", err))?;

        let algorithm = Object::new();
        for (name, value) in [
            ("name", JsValue::from_str("PBKDF2")),
            ("hash", JsValue::from_str("SHA-256")),
            ("salt", Uint8Array::from(salt.as_slice()).into()),
            ("iterations", JsValue::from_f64(f64::from(iterations))),
        ] {
            Reflect::set(&algorithm, &JsValue::from_str(name), &value).map_err(|err| failed("setup", err))?;
        }
        let bits = u32::try_from(bytes * 8).expect("hash lengths fit in u32");
        let derived = derive_bits(&algorithm, &key, bits).map_err(|err| failed("deriveBits", err))?;
        let derived = JsFuture::from(derived).await.map_err(|err| failed("deriveBits", err))?;
        let buffer: ArrayBuffer = derived.dyn_into().map_err(|err| failed("deriveBits", err))?;
        Ok(Uint8Array::new(&buffer).to_vec())
    })
}

fn describe(err: &JsValue) -> String {
    err.dyn_ref::<js_sys::Error>().map_or_else(|| format!("{err:?}"), |err| String::from(err.message()))
}
