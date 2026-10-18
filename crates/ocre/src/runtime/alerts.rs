//! [`record`] and [`send_digest`]: owner alerts in D1, emailed through `ocre::mail`.

use serde::Deserialize;

use super::Ctx;
use crate::{
    Result,
    alerts::{
        ALERT_EMAIL, Alert, CLAIM_SQL, CLEANUP_SQL, DEDUP_MINUTES, HOURLY_CAP, Held, INSERT_SQL, KEEP_DAYS,
        UNCLAIM_SQL, alert_email, digest_email,
    },
    params,
};

/// The owner's address, when set.
fn recipient(ctx: &Ctx) -> Option<String> {
    ctx.env().var(ALERT_EMAIL).ok().map(|value| value.to_string().trim().to_owned()).filter(|value| !value.is_empty())
}

/// Records `alert` and emails it to [`ALERT_EMAIL`](crate::alerts::ALERT_EMAIL) unless it is a repeat or over the hourly cap.
///
/// Never fails: an alert must not break the request that raised it, so a
/// D1 or mail problem is logged (`[ocre alerts] ...`) instead. See the
/// [module documentation](crate::alerts) for the rules.
pub fn record(ctx: &Ctx, alert: Alert) -> impl Future<Output = ()> + Send + use<> {
    let ctx = ctx.clone();
    worker::send::SendFuture::new(async move {
        if let Err(err) = deliver(&ctx, &alert).await {
            ctx.log().error(format!("[ocre alerts] {} not recorded or sent: {err}", alert.reason));
        }
    })
}

async fn deliver(ctx: &Ctx, alert: &Alert) -> Result<()> {
    #[derive(Deserialize)]
    struct Row {
        id: i64,
        status: String,
    }
    let to = recipient(ctx);
    let now = crate::now();
    let row: Option<Row> = ctx
        .db()?
        .first(
            INSERT_SQL,
            params![
                alert.reason.as_str(),
                alert.key.as_deref(),
                alert.detail.as_deref(),
                now,
                now - DEDUP_MINUTES * 60,
                now - 3600,
                HOURLY_CAP,
                to.is_some()
            ],
        )
        .await?;
    let (Some(row), Some(to)) = (row, to) else { return Ok(()) };
    if row.status != "sent" {
        return Ok(());
    }
    if let Err(err) = crate::mail::send(ctx, alert_email(&to, alert, now)).await {
        ctx.db()?.execute("UPDATE alerts SET status = 'failed' WHERE id = ?1", params![row.id]).await?;
        return Err(err);
    }
    Ok(())
}

/// Emails one digest of the alerts held back by the hourly cap, forgets alerts older than [`KEEP_DAYS`](crate::alerts::KEEP_DAYS), and returns how many alerts the digest listed.
///
/// Run it from a schedule (every 15 minutes is plenty): nothing is sent
/// when no alert was held back. Safe to run twice at once: claiming the
/// alerts is one `UPDATE`.
///
/// # Errors
///
/// A D1 or mail error; the held alerts then wait for the next run.
pub fn send_digest(ctx: &Ctx) -> impl Future<Output = Result<usize>> + Send + use<> {
    let ctx = ctx.clone();
    worker::send::SendFuture::new(async move {
        let db = ctx.db()?;
        db.execute(CLEANUP_SQL, params![crate::now() - KEEP_DAYS * 86_400]).await?;
        let Some(to) = recipient(&ctx) else { return Ok(0) };
        let held: Vec<Held> = db.all(CLAIM_SQL, params![]).await?;
        if held.is_empty() {
            return Ok(0);
        }
        if let Err(err) = crate::mail::send(&ctx, digest_email(&to, &held)).await {
            let ids = serde_json::json!(held.iter().map(|row| row.id).collect::<Vec<_>>());
            db.execute(UNCLAIM_SQL, params![ids]).await?;
            return Err(err);
        }
        Ok(held.len())
    })
}
