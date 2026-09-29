//! Wall-clock time that works on Workers and in native tests.
//!
//! `std::time::SystemTime::now()` panics on `wasm32-unknown-unknown`; on
//! Workers the time comes from JavaScript's `Date.now()`. Workers advance it
//! only between I/O operations, which is precise enough for expirations.

/// Current Unix time in seconds, on Workers and in native tests.
///
/// On Workers it reads JavaScript's `Date.now()`, which Workers advance only
/// between I/O operations: precise enough for expirations (sessions, tokens,
/// JWT `exp`, cache TTLs), not for measuring CPU time. Natively it reads
/// [`std::time::SystemTime`]. Use it instead of `SystemTime::now()` in app
/// code: that one panics on `wasm32-unknown-unknown`. Costs no binding call.
///
/// # Panics
///
/// Natively only, if the system clock is set before 1970.
///
/// # Examples
///
/// ```
/// let issued_at = ocre::now();
/// let expires_at = issued_at + 3600;
/// assert!(expires_at > issued_at);
/// ```
pub fn now() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        crate::runtime::unix_millis() / 1000
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let elapsed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        i64::try_from(elapsed.expect("the clock is after 1970").as_secs()).expect("seconds fit in i64")
    }
}

#[cfg(test)]
#[path = "../tests/clock.rs"]
mod tests;
