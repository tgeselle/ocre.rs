//! Owner alerts: an email to the app's owner when something needs attention, without flooding the inbox.
//!
//! [`record`] stores an [`Alert`] (a dotted reason such as
//! `payment.failed`, an optional key such as the user's id, a detail) in
//! the `alerts` table (`ocre g alerts` creates it) and emails it to
//! [`ALERT_EMAIL`] at once, unless:
//!
//! - the same reason with the same key was emailed in the last
//!   [`DEDUP_MINUTES`] (30): the alert is kept as `deduped`, no email;
//! - [`HOURLY_CAP`] (20) alerts were emailed in the last hour: the alert
//!   waits as `overflow` for the digest, one email listing what was held
//!   back by reason ([`send_digest`], from a schedule).
//!
//! The decision is part of the `INSERT` (one statement), so concurrent
//! requests cannot both email the same alert. Alerts are owner-facing:
//! plain text, never translated, only ever sent to `ALERT_EMAIL`.
//! [`send_digest`] also forgets alerts older than [`KEEP_DAYS`].
//!
//! # Free plan
//!
//! [`record`]: one D1 write, plus one email when it is sent (the mail
//! adapter's own limits apply: Resend's free plan sends 100 a day, which
//! the hourly cap keeps alerts well under). [`send_digest`]: one D1 write,
//! one read, one delete, and an email only when alerts were held back.
//!
//! # Examples
//!
//! ```no_run
//! use ocre::{Ctx, alerts::{self, Alert}};
//!
//! async fn payment_failed(ctx: &Ctx, user_id: i64, provider_message: &str) {
//!     let alert = Alert::new("payment.failed").key(user_id).detail(format!("Provider said: {provider_message}"));
//!     alerts::record(ctx, alert).await; // never fails the request: a problem is logged
//! }
//!
//! // src/schedules/alert_digest.rs (`ocre g schedule alert_digest "every 15 minutes"`):
//! pub async fn run(ctx: &Ctx) -> ocre::Result<()> {
//!     alerts::send_digest(ctx).await.map(|_| ())
//! }
//! ```

use std::fmt::Display;

use serde::Deserialize;

use crate::{helpers::civil_from_days, mail::Email};

pub use crate::runtime::alerts::{record, send_digest};

/// Worker variable holding the owner's address, where alerts go: `ALERT_EMAIL`.
///
/// Unset, alerts are still recorded (status `unsent`) and nothing is emailed.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::alerts::ALERT_EMAIL, "ALERT_EMAIL");
/// ```
pub const ALERT_EMAIL: &str = "ALERT_EMAIL";

/// Minutes during which the same reason and key are emailed once.
pub const DEDUP_MINUTES: i64 = 30;

/// Alert emails per hour at most; later alerts wait for the digest.
pub const HOURLY_CAP: i64 = 20;

/// Days alerts are kept; [`send_digest`] deletes older ones.
pub const KEEP_DAYS: i64 = 30;

/// The `alerts` table, created by the migration of `ocre g alerts`.
///
/// `status` is the alert's fate: `sent`, `deduped`, `overflow` (waiting for
/// the digest), `digested`, `unsent` (no `ALERT_EMAIL`) or `failed` (the
/// email could not be sent). Times are Unix seconds.
pub const TABLE_SQL: &str = "CREATE TABLE alerts (
  id INTEGER PRIMARY KEY,
  reason TEXT NOT NULL,
  dedup_key TEXT,
  detail TEXT,
  status TEXT NOT NULL CHECK (status IN ('sent', 'deduped', 'overflow', 'digested', 'unsent', 'failed')),
  created_at INTEGER NOT NULL
);
CREATE INDEX index_alerts_on_reason_and_dedup_key ON alerts (reason, dedup_key, created_at);
CREATE INDEX index_alerts_on_status_and_created_at ON alerts (status, created_at);
";

/// Something the owner should know about: a reason, what it concerns, and details.
///
/// # Examples
///
/// ```
/// let alert = ocre::alerts::Alert::new("upload.failed").key("user:7").detail("R2 refused part 3");
/// assert_eq!((alert.reason.as_str(), alert.key.as_deref()), ("upload.failed", Some("user:7")));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// A dotted code naming what happened (`payment.failed`); alerts group by it.
    pub reason: String,
    /// What it concerns (a user or record id): the same reason for another key is another alert.
    pub key: Option<String>,
    /// Free text shown in the email.
    pub detail: Option<String>,
}

impl Alert {
    /// An alert for `reason`.
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), key: None, detail: None }
    }

    /// Sets what the alert concerns (deduplicated per reason and key).
    pub fn key(mut self, key: impl Display) -> Self {
        self.key = Some(key.to_string());
        self
    }

    /// Sets the detail shown in the email.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The `INSERT` deciding an alert's fate: `?1` reason, `?2` key, `?3`
/// detail, `?4` now, `?5` dedup window start, `?6` hour start, `?7` cap,
/// `?8` whether there is a recipient.
pub(crate) const INSERT_SQL: &str = "INSERT INTO alerts (reason, dedup_key, detail, status, created_at) \
     VALUES (?1, ?2, ?3, CASE \
       WHEN ?8 = 0 THEN 'unsent' \
       WHEN EXISTS (SELECT 1 FROM alerts d WHERE d.reason = ?1 AND d.dedup_key IS ?2 AND d.status = 'sent' \
         AND d.created_at > ?5) THEN 'deduped' \
       WHEN (SELECT COUNT(*) FROM alerts c WHERE c.status = 'sent' AND c.created_at > ?6) >= ?7 THEN 'overflow' \
       ELSE 'sent' END, ?4) \
     RETURNING id, status";

/// Claims the alerts waiting for the digest.
pub(crate) const CLAIM_SQL: &str =
    "UPDATE alerts SET status = 'digested' WHERE status = 'overflow' RETURNING id, reason, dedup_key, created_at";

/// Puts claimed alerts back when the digest email failed: `?1` their ids, a JSON array.
pub(crate) const UNCLAIM_SQL: &str =
    "UPDATE alerts SET status = 'overflow' WHERE status = 'digested' AND id IN (SELECT value FROM json_each(?1))";

/// Forgets alerts older than `?1`.
pub(crate) const CLEANUP_SQL: &str = "DELETE FROM alerts WHERE created_at < ?1";

/// A row claimed by the digest.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Held {
    pub(crate) id: i64,
    pub(crate) reason: String,
    pub(crate) dedup_key: Option<String>,
    pub(crate) created_at: i64,
}

/// `2026-10-18 14:05 UTC`.
fn utc(unix: i64) -> String {
    let (year, month, day) = civil_from_days(unix.div_euclid(86_400));
    let seconds = unix.rem_euclid(86_400);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", seconds / 3600, seconds % 3600 / 60)
}

/// The email of one alert.
pub(crate) fn alert_email(to: &str, alert: &Alert, now: i64) -> Email {
    let mut text = format!("{}\n\nWhen: {}\n", alert.reason, utc(now));
    if let Some(key) = &alert.key {
        text.push_str(&format!("About: {key}\n"));
    }
    if let Some(detail) = &alert.detail {
        text.push_str(&format!("\n{detail}\n"));
    }
    text.push_str(&format!(
        "\nThe same alert is emailed once per {DEDUP_MINUTES} minutes; past {HOURLY_CAP} alerts an hour, the others \
         wait for the digest.\n"
    ));
    Email::new(to, format!("Alert: {}", alert.reason), text)
}

/// The digest of the alerts held back, by reason, most frequent first.
pub(crate) fn digest_email(to: &str, held: &[Held]) -> Email {
    let mut reasons: Vec<(&str, usize, usize, i64, i64)> = Vec::new();
    for row in held {
        match reasons.iter_mut().find(|(reason, ..)| *reason == row.reason) {
            Some(entry) => {
                entry.1 += 1;
                entry.3 = entry.3.min(row.created_at);
                entry.4 = entry.4.max(row.created_at);
            }
            None => reasons.push((&row.reason, 1, 0, row.created_at, row.created_at)),
        }
    }
    for entry in &mut reasons {
        let mut keys: Vec<&str> =
            held.iter().filter(|row| row.reason == entry.0).filter_map(|row| row.dedup_key.as_deref()).collect();
        keys.sort_unstable();
        keys.dedup();
        entry.2 = keys.len();
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut text = format!("{} alerts were held back by the hourly limit of {HOURLY_CAP} emails:\n\n", held.len());
    for (reason, count, keys, first, last) in &reasons {
        text.push_str(&format!("- {reason}: {count} ({keys} distinct), {} to {}\n", utc(*first), utc(*last)));
    }
    Email::new(to, format!("Alert digest: {} alerts held back", held.len()), text)
}

#[cfg(test)]
#[path = "../tests/alerts.rs"]
mod tests;
