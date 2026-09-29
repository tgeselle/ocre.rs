use super::*;

#[test]
fn simple_message_with_folded_and_encoded_headers() {
    let raw = b"From: \"Ada\" <ada@example.com>\r\nTo: support@app.test\r\nSubject: =?UTF-8?B?Q2Fmw6k=?=\r\n \
                =?utf-8?Q?_cr=C3=A8me?= and more\r\nX-Long: one\r\n\ttwo\r\n \r\n\r\nHello\r\nthere\r\n";
    let message = Message::parse(raw);
    assert_eq!(message.header("subject"), Some("Café crème and more"), "adjacent encoded words join");
    assert_eq!(message.header("X-LONG"), Some("one two"));
    assert_eq!(message.header("from"), Some("\"Ada\" <ada@example.com>"));
    assert_eq!(message.header("Missing"), None);
    assert_eq!(message.headers.len(), 4, "the whitespace-only continuation is folded, not a header");
    assert_eq!(message.text.as_deref(), Some("Hello\r\nthere\r\n"));
    assert_eq!(message.html, None);
}

#[test]
fn headers_only_and_junk_lines() {
    let message = Message::parse(b" orphan continuation\nno colon here\nSubject: Hi");
    assert_eq!(message.headers, vec![("Subject".to_owned(), "Hi".to_owned())]);
    assert_eq!(message.text.as_deref(), Some(""));
}

#[test]
fn encoded_words_that_do_not_decode_stay_as_is() {
    for kept in ["=?", "=?utf-8", "=?utf-8?B", "=?utf-8?B?abc", "=??B?abc?=", "=?utf-8?X?abc?=", "=?utf-8?Q?a b?="] {
        assert_eq!(decode_words(kept), kept);
    }
    assert_eq!(decode_words("a =?iso-8859-1?q?caf=E9?= b"), "a café b");
    assert_eq!(decode_words("=?utf-8*en?Q?x?=y =?utf-8?Q?z?="), "xy z", "text between words is kept");
    assert_eq!(decode_words("=?x =?utf-8?B?eg==?="), "=?x z");
}

#[test]
fn multipart_alternative_in_mixed_with_attachment() {
    let raw = "Subject: Report\n\
        Content-Type: multipart/mixed; boundary=\"outer; b\"\n\
        \n\
        preamble\n\
        --outer; b\n\
        Content-Type: multipart/alternative; boundary=inner\n\
        \n\
        --inner\n\
        Content-Type: text/plain; charset=utf-8\n\
        Content-Transfer-Encoding: quoted-printable\n\
        \n\
        Caf=C3=A9 =3D soft=\n\
        break=\r\nx =ZZ end=\n\
        --inner\n\
        Content-Type: text/html; charset=\"ISO-8859-1\"\n\
        Content-Transfer-Encoding: base64\n\
        \n\
        PHA+Y2Fm6TwvcD4=\n\
        --inner--\n\
        --outer; b\n\
        Content-Type: text/plain\n\
        Content-Disposition: attachment; filename=notes.txt\n\
        \n\
        attached\n\
        --outer; b\n\
        Content-Type: image/png; name=\"=?UTF-8?Q?logo=C3=A9.png?=\"\n\
        Content-ID: <logo@x>\n\
        Content-Transfer-Encoding: base64\n\
        \n\
        AQID\n\
        --outer; b--\n\
        epilogue\n";
    let message = Message::parse(raw.as_bytes());
    assert_eq!(message.header("Subject"), Some("Report"));
    assert_eq!(message.text.as_deref(), Some("Café = softbreakx =ZZ end="), "malformed escapes are kept");
    assert_eq!(message.html.as_deref(), Some("<p>café</p>"));
    let files: Vec<_> = message
        .attachments
        .iter()
        .map(|a| (a.filename.as_str(), a.content_type.as_str(), a.content.as_slice(), a.content_id.as_deref()))
        .collect();
    assert_eq!(
        files,
        [
            ("notes.txt", "text/plain", b"attached".as_slice(), None),
            ("logoé.png", "image/png", [1, 2, 3].as_slice(), Some("logo@x")),
        ]
    );
}

#[test]
fn first_text_part_wins_and_crlf_boundaries() {
    let raw = "Content-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\n\r\nfirst\r\n--b\r\n\r\nsecond\r\n--b\r\n\
               Content-Type: text/html\r\n\r\n<b>unterminated</b>\r\n";
    let message = Message::parse(raw.as_bytes());
    assert_eq!(message.text.as_deref(), Some("first"));
    assert_eq!(message.attachments[0].filename, "attachment", "a later text part is a nameless attachment");
    assert_eq!(message.attachments[0].content, b"second");
    assert_eq!(message.html.as_deref(), Some("<b>unterminated</b>\r\n"), "no closing delimiter keeps the last part");
}

#[test]
fn empty_parts_and_multiparts_without_boundary() {
    let message = Message::parse(b"Content-Type: multipart/alternative; boundary=b\n\n--b\n\nbody\n--b\n--b--\n");
    assert_eq!(message.text.as_deref(), Some("body"), "the empty last part is ignored");
    let message = Message::parse(b"Content-Type: multipart/mixed\n\n--b\n\ntext\n--b--\n");
    assert_eq!(message, Message { headers: message.headers.clone(), ..Message::default() });
}

#[test]
fn nesting_is_bounded() {
    let mut raw = String::new();
    for depth in 0..=MAX_DEPTH {
        raw.push_str(&format!("Content-Type: multipart/mixed; boundary=b{depth}\n\n--b{depth}\n"));
    }
    raw.push_str("\ndeep\n");
    assert_eq!(Message::parse(raw.as_bytes()).text, None);
}

#[test]
fn base64_skips_line_breaks_and_junk() {
    assert_eq!(base64(b"SGVs\r\nbG8h"), b"Hello!");
    assert_eq!(base64(b"SGk=ignored"), b"Hi");
    assert_eq!(base64(b"+/+/*"), [0xfb, 0xff, 0xbf]);
    assert_eq!(base64(b"YWJj ZGVm"), b"abcdef");
}

#[test]
fn quoted_printable_edges() {
    assert_eq!(quoted_printable(b"a=\nb=\r\nc=41=4a=4"), b"abcAJ=4");
    assert_eq!(quoted_printable(b"x=+1="), b"x=+1=");
}

#[test]
fn charsets() {
    assert_eq!(decode_charset(&[0x63, 0xe9], "windows-1252"), "cé");
    assert_eq!(decode_charset("é".as_bytes(), " UTF-8 "), "é");
    assert_eq!(decode_charset(&[0xff], "koi8-r"), "\u{fffd}", "unknown charsets decode as lossy UTF-8");
}
