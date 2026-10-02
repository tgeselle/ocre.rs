//! `ocre secret` and the random values behind `SECRET_KEY_BASE`.

use std::fmt::Write;

use crate::{CliResult, output::Report};

/// Worker secret Ocre derives its session and cookie keys from.
pub const SECRET_KEY_BASE: &str = "SECRET_KEY_BASE";

/// 64 bytes from the OS random number generator, as 128 lowercase hex characters.
pub fn generate() -> String {
    let mut bytes = [0u8; 64];
    getrandom::fill(&mut bytes).expect("the OS random number generator is available");
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

pub fn run() -> CliResult {
    Ok(Report { secret: Some(generate()), ..Report::new("secret") })
}

/// `ocre push-keys`: a new VAPID key pair for web push.
pub fn push_keys() -> CliResult {
    let keys = ocre::push::VapidKeys::generate();
    let vapid = serde_json::json!({ "public_key": keys.public_key, "private_key": keys.private_key });
    Ok(Report { vapid: Some(vapid), ..Report::new("push-keys") })
}
