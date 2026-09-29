//! Just enough MIME (RFC 5322, 2045-2047) to read incoming mail: headers,
//! nested multiparts, the first text/plain and text/html parts,
//! quoted-printable and base64 bodies, encoded-word headers. Attachments are
//! skipped; the raw bytes stay available for anything else.

/// A parsed message: decoded headers and the text bodies.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Message {
    /// `(name, value)` in order, unfolded, encoded words decoded.
    pub headers: Vec<(String, String)>,
    pub text: Option<String>,
    pub html: Option<String>,
}

/// Multiparts nested deeper than this are ignored (malicious input).
const MAX_DEPTH: u8 = 8;

impl Message {
    pub fn parse(raw: &[u8]) -> Self {
        let (head, body) = split_head(raw);
        let mut message = Self { headers: parse_headers(head), ..Self::default() };
        let headers = std::mem::take(&mut message.headers);
        message.collect(&headers, body, 0);
        message.headers = headers;
        message
    }

    /// First header with this name, case-insensitive.
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    fn collect(&mut self, headers: &[(String, String)], body: &[u8], depth: u8) {
        let (mime, params) = content_type(header(headers, "content-type").unwrap_or("text/plain"));
        if mime.starts_with("multipart/") {
            let Some(boundary) = param(&params, "boundary") else { return };
            if depth >= MAX_DEPTH {
                return;
            }
            for part in split_multipart(body, boundary) {
                let (head, body) = split_head(part);
                self.collect(&parse_headers(head), body, depth + 1);
            }
            return;
        }
        let attachment = header(headers, "content-disposition")
            .is_some_and(|value| value.trim_start().to_ascii_lowercase().starts_with("attachment"));
        let slot = match mime.as_str() {
            "text/plain" => &mut self.text,
            "text/html" => &mut self.html,
            _ => return,
        };
        if attachment || slot.is_some() {
            return;
        }
        let encoding = header(headers, "content-transfer-encoding").unwrap_or("7bit").trim().to_ascii_lowercase();
        let bytes = match encoding.as_str() {
            "base64" => base64(body),
            "quoted-printable" => quoted_printable(body),
            _ => body.to_vec(),
        };
        *slot = Some(decode_charset(&bytes, param(&params, "charset").unwrap_or("us-ascii")));
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// Splits at the first empty line. No empty line: all headers, no body.
fn split_head(raw: &[u8]) -> (&[u8], &[u8]) {
    let mut start = 0;
    for line in raw.split_inclusive(|&b| b == b'\n') {
        if line == b"\n" || line == b"\r\n" {
            return (&raw[..start], &raw[start + line.len()..]);
        }
        start += line.len();
    }
    (raw, &[])
}

/// Unfolds continuation lines and decodes encoded words.
fn parse_headers(head: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = headers.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    headers.into_iter().map(|(name, value)| (name, decode_words(value.trim()))).collect()
}

/// `text/plain; charset="utf-8"` -> (`text/plain`, [(`charset`, `utf-8`)]).
fn content_type(value: &str) -> (String, Vec<(String, String)>) {
    let mut parts = split_params(value).into_iter();
    let mime = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let params = parts
        .filter_map(|part| {
            let (name, value) = part.split_once('=')?;
            let value = value.trim();
            let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
            Some((name.trim().to_ascii_lowercase(), value.to_owned()))
        })
        .collect();
    (mime, params)
}

/// Splits on `;` outside double quotes.
fn split_params(value: &str) -> Vec<&str> {
    let (mut parts, mut start, mut quoted) = (Vec::new(), 0, false);
    for (i, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// The parts between `--boundary` lines, up to `--boundary--`.
fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delimiter = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut part_start: Option<usize> = None;
    let mut offset = 0;
    for line in body.split_inclusive(|&b| b == b'\n') {
        let content = line.trim_ascii_end();
        if let Some(rest) = content.strip_prefix(delimiter.as_bytes())
            && (rest.is_empty() || rest == b"--")
        {
            if let Some(start) = part_start {
                // The line break before the delimiter belongs to it; the
                // previous line always ends with one.
                let end = offset - if body[..offset].ends_with(b"\r\n") { 2 } else { 1 };
                parts.push(&body[start..end.max(start)]);
            }
            if rest == b"--" {
                return parts;
            }
            part_start = Some(offset + line.len());
        }
        offset += line.len();
    }
    // No closing delimiter: keep the last part.
    if let Some(start) = part_start {
        parts.push(&body[start..]);
    }
    parts
}

/// `=?charset?B|Q?text?=` words decoded; whitespace between two encoded
/// words dropped, as RFC 2047 requires.
fn decode_words(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    let mut after_word = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        match encoded_word(&rest[start..]) {
            Some((decoded, len)) => {
                if !(after_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&decoded);
                rest = &rest[start + len..];
                after_word = true;
            }
            None => {
                out.push_str(&rest[..start + 2]);
                rest = &rest[start + 2..];
                after_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Decodes the encoded word at the start of `text`; returns it and its length.
fn encoded_word(text: &str) -> Option<(String, usize)> {
    let inner = &text[2..];
    let (charset, rest) = inner.split_once('?')?;
    let (encoding, rest) = rest.split_once('?')?;
    let end = rest.find("?=")?;
    let word = &rest[..end];
    if charset.is_empty() || word.contains(' ') {
        return None;
    }
    let bytes = match encoding {
        "B" | "b" => base64(word.as_bytes()),
        "Q" | "q" => quoted_printable(word.replace('_', " ").as_bytes()),
        _ => return None,
    };
    let charset = charset.split('*').next().unwrap_or(charset);
    Some((decode_charset(&bytes, charset), 2 + inner.len() - rest.len() + end + 2))
}

/// UTF-8 (lossy) for UTF-8, ASCII and unknown charsets; Latin-1 byte to
/// char for ISO-8859-1 and windows-1252 (close enough for text bodies).
fn decode_charset(bytes: &[u8], charset: &str) -> String {
    match charset.trim().to_ascii_lowercase().as_str() {
        "iso-8859-1" | "latin1" | "iso-8859-15" | "windows-1252" | "cp1252" => {
            bytes.iter().map(|&b| char::from(b)).collect()
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Base64, skipping line breaks and anything outside the alphabet; stops at `=`.
fn base64(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in input {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => continue,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    out
}

/// Quoted-printable: `=XX` escapes and `=` soft line breaks. A malformed
/// escape is kept as is.
fn quoted_printable(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != b'=' {
            out.push(input[i]);
            i += 1;
        } else if input[i + 1..].starts_with(b"\r\n") {
            i += 3;
        } else if input[i + 1..].starts_with(b"\n") {
            i += 2;
        } else if let Some(byte) = input.get(i + 1..i + 3).and_then(hex_byte) {
            out.push(byte);
            i += 3;
        } else {
            out.push(b'=');
            i += 1;
        }
    }
    out
}

fn hex_byte(pair: &[u8]) -> Option<u8> {
    let digit = |byte: u8| char::from(byte).to_digit(16);
    Some((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
}

#[cfg(test)]
mod tests;
