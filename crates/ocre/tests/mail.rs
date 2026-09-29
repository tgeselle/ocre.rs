use super::*;

fn internal(err: Error) -> String {
    match err {
        Error::Internal(message) => message,
        other => panic!("expected an internal error, got {other:?}"),
    }
}

#[test]
fn adapter_is_explicit() {
    assert_eq!(adapter(Some("log")).unwrap(), Adapter::Log);
    assert_eq!(adapter(Some(" resend ")).unwrap(), Adapter::Resend);
    assert_eq!(adapter(Some("cloudflare")).unwrap(), Adapter::Cloudflare);
    let unset = internal(adapter(None).unwrap_err());
    assert!(unset.starts_with("cannot send email: MAIL_ADAPTER is not set. Fix: set MAIL_ADAPTER"), "{unset}");
    assert!(unset.contains("MAIL_ADAPTER=log in .dev.vars"), "{unset}");
    let unknown = internal(adapter(Some("smtp")).unwrap_err());
    assert!(unknown.contains("unknown MAIL_ADAPTER \"smtp\" (expected log, resend or cloudflare)"), "{unknown}");
}

#[test]
fn resend_needs_its_key() {
    assert_eq!(resend_key(Some("re_123".into())).unwrap(), "re_123");
    for missing in [None, Some(" ".to_owned())] {
        let message = internal(resend_key(missing).unwrap_err());
        assert!(message.contains("`ocre secrets push RESEND_API_KEY --file .prod.vars`"), "{message}");
    }
}

#[test]
fn mailboxes_parse_with_or_without_a_name() {
    let plain = Mailbox::parse(" noreply@example.com ").unwrap();
    assert_eq!(Mailbox::parse("noreply@example.com").unwrap().to_string(), "noreply@example.com");
    assert_eq!((plain.name, plain.address.as_str()), (None, "noreply@example.com"));
    let named = Mailbox::parse("Ocre App <noreply@example.com>").unwrap();
    assert_eq!(
        (named.name.as_deref(), named.to_string().as_str()),
        (Some("Ocre App"), "Ocre App <noreply@example.com>")
    );
    let quoted = Mailbox::parse("\"Acme, Inc.\" <x@acme.test>").unwrap();
    assert_eq!(quoted.to_string(), "\"Acme, Inc.\" <x@acme.test>", "names with specials stay quoted");
    assert_eq!(Mailbox::parse("<x@acme.test>").unwrap().name, None, "empty name");
    for bad in ["", "Acme", "Acme <nope>", "A\"b <x@acme.test>", "A\nBcc: y@z.co <x@acme.test>"] {
        assert_eq!(Mailbox::parse(bad), None, "{bad:?}");
    }
}

fn outgoing(email: Email) -> Result<Outgoing> {
    Outgoing::new(Some("Ocre <noreply@example.com>".into()), email)
}

#[test]
fn outgoing_checks_sender_recipient_and_subject() {
    let email = Email::new("ada@example.com", "Hi", "Hello");
    let missing = internal(Outgoing::new(None, email.clone()).unwrap_err());
    assert!(missing.contains("MAIL_FROM is not set. Fix: add MAIL_FROM: bindings.text("), "{missing}");
    let bad_from = internal(Outgoing::new(Some("nobody".into()), email.clone()).unwrap_err());
    assert!(bad_from.contains("MAIL_FROM \"nobody\" is not an address"), "{bad_from}");

    let to = outgoing(Email::new("ada at example", "Hi", "Hello")).unwrap_err();
    assert!(matches!(&to, Error::BadRequest(m) if m == "invalid email address: ada at example"), "{to:?}");
    let reply = outgoing(email.clone().reply_to("x@y.co\r\nBcc: z@z.co")).unwrap_err();
    assert!(matches!(reply, Error::BadRequest(_)), "header injection through Reply-To");
    for subject in ["", "  ", "Hi\r\nBcc: z@z.co"] {
        let message = internal(outgoing(Email::new("ada@example.com", subject, "x")).unwrap_err());
        assert!(message.contains("the subject must be one non-empty line"), "{message}");
    }
    assert_eq!(outgoing(email.clone()).unwrap().email, email);
}

#[test]
fn log_text_shows_the_whole_email() {
    let text = outgoing(Email::new("ada@example.com", "Hi", "Open https://example.com/x")).unwrap().log_text();
    assert_eq!(
        text,
        "[ocre mail] not sent (MAIL_ADAPTER = \"log\")\nFrom: Ocre <noreply@example.com>\nTo: ada@example.com\n\
         Subject: Hi\n\nOpen https://example.com/x\n[ocre mail] end"
    );
    let full = Email::new("ada@example.com", "Hi", "Hello").html("<p>Hello</p>").reply_to("team@example.com");
    let text = outgoing(full).unwrap().log_text();
    assert!(text.contains("To: ada@example.com\nReply-To: team@example.com\nSubject: Hi\n"), "{text}");
    assert!(text.ends_with("Hello\n[ocre mail] HTML version:\n<p>Hello</p>\n[ocre mail] end"), "{text}");
}

#[test]
fn resend_json_matches_the_api() {
    let plain = outgoing(Email::new("ada@example.com", "Hi", "Hello")).unwrap().resend_json();
    assert_eq!(
        plain,
        json!({"from": "Ocre <noreply@example.com>", "to": ["ada@example.com"], "subject": "Hi", "text": "Hello"})
    );
    let full = Email::new("ada@example.com", "Hi", "Hello").html("<p>Hello</p>").reply_to("team@example.com");
    let body = outgoing(full).unwrap().resend_json();
    assert_eq!((body["html"].as_str(), body["reply_to"].as_str()), (Some("<p>Hello</p>"), Some("team@example.com")));
}

fn full_email() -> Email {
    Email::new("Ada <ada@example.com>", "Report", "See attached.")
        .also_to("grace@example.com")
        .cc("team@example.com")
        .bcc("archive@example.com")
        .header("In-Reply-To", "<1@example.com>")
        .html("<img src=\"cid:logo\">")
        .attach("report.csv", "text/csv", b"a,b\n".to_vec())
        .inline("logo", "logo.png", "image/png", vec![1, 2, 3])
}

#[test]
fn recipients_headers_and_attachments_are_checked() {
    let checked = outgoing(full_email()).unwrap();
    assert_eq!(Outgoing::addresses(&checked.email.to), ["ada@example.com", "grace@example.com"]);
    for bad in [full_email().cc("nope"), full_email().bcc("x@y.co\r\nTo: z@z.co"), full_email().also_to("a b")] {
        assert!(matches!(outgoing(bad), Err(Error::BadRequest(_))));
    }
    let mut none = full_email();
    none.to.clear();
    assert!(internal(outgoing(none).unwrap_err()).contains("it has no `to` recipient"));
    let crowd = (0..50).fold(Email::new("a@example.com", "Hi", "x"), |email, i| email.bcc(format!("u{i}@example.com")));
    let message = internal(outgoing(crowd).unwrap_err());
    assert!(message.starts_with("cannot send email to 51 recipients: 50 at most"), "{message}");
    for (name, value) in [("Subject", "x"), ("BCC", "x"), ("X Bad", "x"), ("", "x"), ("X-Ok", "a\r\nBcc: b@c.co")] {
        let message = internal(outgoing(full_email().header(name, value)).unwrap_err());
        assert!(message.contains("is not allowed. Fix:"), "{name}: {message}");
    }
    for bad in [
        Email::new("a@example.com", "Hi", "x").attach(" ", "text/plain", vec![]),
        Email::new("a@example.com", "Hi", "x").attach("a.txt", "text", vec![]),
        Email::new("a@example.com", "Hi", "x").attach("a.txt", "text/ plain", vec![]),
        Email::new("a@example.com", "Hi", "x").inline("<logo>", "a.png", "image/png", vec![]),
        Email::new("a@example.com", "Hi", "x").inline("", "a.png", "image/png", vec![]),
    ] {
        let message = internal(outgoing(bad).unwrap_err());
        assert!(message.contains("is invalid. Fix: give a file name"), "{message}");
    }
}

#[test]
fn the_email_may_set_its_own_sender() {
    let email = Email::new("ada@example.com", "Hi", "x").from("Billing <billing@example.com>");
    assert_eq!(Outgoing::new(None, email).unwrap().from.to_string(), "Billing <billing@example.com>");
    let bad = internal(Outgoing::new(None, Email::new("ada@example.com", "Hi", "x").from("billing")).unwrap_err());
    assert!(bad.contains("the email's `from` \"billing\" is not an address"), "{bad}");
}

#[test]
fn log_and_resend_carry_every_part() {
    let checked = outgoing(full_email()).unwrap();
    let text = checked.log_text();
    assert!(
        text.contains(
            "To: Ada <ada@example.com>, grace@example.com\nCc: team@example.com\nBcc: archive@example.com\n\
             In-Reply-To: <1@example.com>\nSubject: Report\n"
        ),
        "{text}"
    );
    assert!(
        text.ends_with(
            "[ocre mail] attachment: report.csv (text/csv, 4 bytes)\n\
             [ocre mail] inline cid:logo: logo.png (image/png, 3 bytes)\n[ocre mail] end"
        ),
        "{text}"
    );
    let body = checked.resend_json();
    assert_eq!(body["to"], json!(["Ada <ada@example.com>", "grace@example.com"]));
    assert_eq!(
        (body["cc"].clone(), body["bcc"].clone()),
        (json!(["team@example.com"]), json!(["archive@example.com"]))
    );
    assert_eq!(body["headers"], json!({"In-Reply-To": "<1@example.com>"}));
    assert_eq!(
        body["attachments"],
        json!([
            {"filename": "report.csv", "content": "YSxiCg==", "content_type": "text/csv"},
            {"filename": "logo.png", "content": "AQID", "content_type": "image/png", "content_id": "logo"},
        ])
    );
}

#[test]
fn attachments_travel_as_base64() {
    let email = full_email();
    let json = serde_json::to_value(&email).unwrap();
    assert_eq!(json["attachments"][0]["content"], "YSxiCg==");
    assert_eq!(serde_json::from_value::<Email>(json).unwrap(), email);
    let broken = json!({"to": ["a@b.co"], "subject": "x", "text": "x", "html": null, "reply_to": null,
        "attachments": [{"filename": "a", "content_type": "a/b", "content": "not base64!"}]});
    assert!(serde_json::from_value::<Email>(broken).is_err());
}

#[test]
fn names_are_quoted_and_cleaned() {
    assert_eq!(address_with_name("Ada \"The\"\nCountess", "ada@example.com"), "Ada The Countess <ada@example.com>");
    assert_eq!(address_with_name("", " ada@example.com "), "ada@example.com");
    let quoted = address_with_name("Acme, Inc.", "x@acme.test");
    assert_eq!(Mailbox::parse(&quoted).unwrap().name.as_deref(), Some("Acme, Inc."), "round-trips");
}

#[test]
fn provider_errors_name_the_fix() {
    let auth =
        internal(resend_error(403, r#"{"statusCode":403,"name":"validation_error","message":"Domain not verified"}"#));
    assert!(
        auth.starts_with("Resend did not send the email (403: Domain not verified). Fix: check the RESEND_API_KEY")
    );
    let quota = internal(resend_error(429, "slow down"));
    assert!(quota.contains("(429: slow down)") && quota.contains("100 emails a day"), "{quota}");
    let other = internal(resend_error(500, "{}"));
    assert!(other.contains("(500: {})") && other.contains("api-reference/errors"), "{other}");
    let cloudflare = internal(cloudflare_error("E_SENDER_NOT_VERIFIED: no"));
    assert!(cloudflare.contains("(E_SENDER_NOT_VERIFIED: no)") && cloudflare.contains("Workers Paid plan"));
}
