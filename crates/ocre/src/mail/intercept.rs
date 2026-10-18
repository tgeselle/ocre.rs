//! [`intercept`]: change or stop outgoing email before any adapter sends it (Rails' mail interceptors).

use std::sync::{Arc, Mutex, PoisonError};

use super::Email;

/// An interceptor: returns the email to send (changed or not), or `None` to drop it.
type Interceptor = dyn Fn(Email, &dyn Fn(&str) -> Option<String>) -> Option<Email> + Send + Sync;

static INTERCEPTORS: Mutex<Vec<Arc<Interceptor>>> = Mutex::new(Vec::new());

/// Registers an interceptor every outgoing email goes through, in registration order, before the adapter sends it.
///
/// Rails' `register_interceptor`: `interceptor` gets the [`Email`] and a
/// lookup of Worker variables and secrets, and returns the email to send
/// (rewritten, e.g. every recipient replaced on a staging copy) or `None`
/// to drop it, logged as `[ocre mail] dropped by an interceptor: ...`. It
/// applies to [`send`](crate::mail::send) and to emails sent from the
/// queue ([`deliver_later`](crate::mail::deliver_later)), whatever the
/// adapter, `log` included. Register interceptors once per Worker instance,
/// in the `start` event of `src/lib.rs`.
///
/// # Examples
///
/// A safety switch for automated emails on a copy of production's users:
/// marketing emails carry `X-Category: marketing`, and go out only when the
/// `MARKETING_EMAILS` variable is `on`.
///
/// ```no_run
/// #[worker::event(start)]
/// fn start() {
///     ocre::mail::intercept(|email, vars| {
///         let marketing = email.headers.iter().any(|(name, value)| name == "X-Category" && value == "marketing");
///         let allowed = vars("MARKETING_EMAILS").is_some_and(|value| value.trim() == "on");
///         (!marketing || allowed).then_some(email)
///     });
/// }
/// ```
pub fn intercept(
    interceptor: impl Fn(Email, &dyn Fn(&str) -> Option<String>) -> Option<Email> + Send + Sync + 'static,
) {
    INTERCEPTORS.lock().unwrap_or_else(PoisonError::into_inner).push(Arc::new(interceptor));
}

/// The email after every registered interceptor, or `None` when one dropped it.
pub(crate) fn intercepted(email: Email, vars: &dyn Fn(&str) -> Option<String>) -> Option<Email> {
    let interceptors = INTERCEPTORS.lock().unwrap_or_else(PoisonError::into_inner).clone();
    run(&interceptors, email, vars)
}

fn run(interceptors: &[Arc<Interceptor>], email: Email, vars: &dyn Fn(&str) -> Option<String>) -> Option<Email> {
    interceptors.iter().try_fold(email, |email, interceptor| interceptor(email, vars))
}

/// The log line of a dropped email: subject and recipients.
pub(crate) fn dropped_line(email: &Email) -> String {
    format!("{} dropped by an interceptor: {:?} to {}", super::LOG_PREFIX, email.subject, email.to.join(", "))
}

#[cfg(test)]
#[path = "../../tests/mail/intercept.rs"]
mod tests;
