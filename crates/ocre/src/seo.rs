//! Search engines and language models: JSON-LD, sitemaps and `llms.txt`.
//!
//! - [`json_ld`]: structured data (`Organization`, `FAQPage`,
//!   `BreadcrumbList`...) as a `<script type="application/ld+json">`,
//!   escaped so the JSON can never close the script element.
//! - [`Sitemap`]: `/sitemap.xml`, with the other languages of each page
//!   (`xhtml:link rel="alternate" hreflang`) and `lastmod`.
//! - [`LlmsTxt`]: `/llms.txt`, the Markdown index of a site for language
//!   models (<https://llmstxt.org>).
//!
//! The canonical and `hreflang` links of a page come from the visitor's
//! locale: [`I18n::alternate_links`](crate::i18n::I18n::alternate_links).
//! `ocre g seo` writes `src/seo.rs` with the routes and a list of pages.

use std::fmt::Write as _;

use axum::{
    http::header,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// `data` as a JSON-LD `<script>` for a page's `<head>`.
///
/// The JSON is escaped for HTML: `<`, `>` and `&` become `\u003c`,
/// `\u003e` and `\u0026` (as do U+2028 and U+2029), so a value containing
/// `</script>` cannot end the element, and the JSON parses to the same
/// data. In askama: `{{ ocre::seo::json_ld(&data)|safe }}`.
///
/// # Examples
///
/// ```
/// use serde_json::json;
///
/// let html = ocre::seo::json_ld(&json!({
///     "@context": "https://schema.org",
///     "@type": "Organization",
///     "name": "PureFrame </script><b>",
/// }));
/// assert!(html.starts_with("<script type=\"application/ld+json\">{"));
/// assert!(html.contains("PureFrame \\u003c/script\\u003e\\u003cb\\u003e"));
/// assert_eq!(html.matches("</script>").count(), 1);
/// ```
pub fn json_ld(data: &impl Serialize) -> String {
    let json = serde_json::to_string(data).unwrap_or_else(|_| "null".to_owned());
    let mut out = String::with_capacity(json.len() + 48);
    out.push_str("<script type=\"application/ld+json\">");
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.push_str("</script>");
    out
}

/// A page of a [`Sitemap`]: its absolute URL, last change and other languages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SitemapUrl {
    /// Absolute URL (`https://example.com/fr/pricing`).
    pub loc: String,
    /// Last change, `YYYY-MM-DD` or a full W3C date-time; omitted when `None`.
    pub lastmod: Option<String>,
    /// The page in each language, the page itself included: `(hreflang, absolute URL)`,
    /// plus `("x-default", url)` for the language-picking version.
    pub alternates: Vec<(String, String)>,
}

/// `/sitemap.xml`: the pages search engines should crawl.
///
/// Answer it from a route (it is a response: `application/xml`). One
/// sitemap holds at most 50,000 URLs (and 50 MB); split larger sites.
///
/// # Examples
///
/// ```
/// use ocre::seo::{Sitemap, SitemapUrl};
///
/// let mut sitemap = Sitemap::new();
/// sitemap.add(SitemapUrl {
///     loc: "https://example.com/en/pricing".into(),
///     lastmod: Some("2026-10-01".into()),
///     alternates: vec![
///         ("en".into(), "https://example.com/en/pricing".into()),
///         ("fr".into(), "https://example.com/fr/pricing".into()),
///     ],
/// });
/// let xml = sitemap.to_xml();
/// assert!(xml.contains("<url><loc>https://example.com/en/pricing</loc><lastmod>2026-10-01</lastmod>"));
/// assert!(xml.contains("<xhtml:link rel=\"alternate\" hreflang=\"fr\" href=\"https://example.com/fr/pricing\"/>"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sitemap {
    urls: Vec<SitemapUrl>,
}

impl Sitemap {
    /// Most URLs in one sitemap file.
    pub const MAX_URLS: usize = 50_000;

    /// An empty sitemap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a page.
    pub fn add(&mut self, url: SitemapUrl) -> &mut Self {
        self.urls.push(url);
        self
    }

    /// Adds a page in every language: `urls` is `(hreflang, absolute URL)`
    /// for each locale, the first being the default (also `x-default`).
    /// Each version is listed, with all the others as alternates, as
    /// search engines expect.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut sitemap = ocre::seo::Sitemap::new();
    /// sitemap.add_localized(&[("en", "https://ex.com/en"), ("fr", "https://ex.com/fr")], None);
    /// let xml = sitemap.to_xml();
    /// assert_eq!(xml.matches("<url>").count(), 2);
    /// assert_eq!(xml.matches("hreflang=\"x-default\" href=\"https://ex.com/en\"").count(), 2);
    /// ```
    pub fn add_localized(&mut self, urls: &[(&str, &str)], lastmod: Option<&str>) -> &mut Self {
        let mut alternates: Vec<(String, String)> =
            urls.iter().map(|(lang, url)| ((*lang).to_owned(), (*url).to_owned())).collect();
        if let Some((_, default)) = urls.first() {
            alternates.push(("x-default".to_owned(), (*default).to_owned()));
        }
        for (_, url) in urls {
            self.urls.push(SitemapUrl {
                loc: (*url).to_owned(),
                lastmod: lastmod.map(str::to_owned),
                alternates: alternates.clone(),
            });
        }
        self
    }

    /// The URLs added so far.
    pub fn len(&self) -> usize {
        self.urls.len()
    }

    /// Whether no URL was added.
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty()
    }

    /// The sitemap document (UTF-8 XML, values escaped).
    pub fn to_xml(&self) -> String {
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" \
             xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">\n",
        );
        for url in &self.urls {
            write!(xml, "<url><loc>{}</loc>", escape_xml(&url.loc)).expect("writing to a String");
            if let Some(lastmod) = &url.lastmod {
                write!(xml, "<lastmod>{}</lastmod>", escape_xml(lastmod)).expect("writing to a String");
            }
            for (lang, href) in &url.alternates {
                write!(
                    xml,
                    "<xhtml:link rel=\"alternate\" hreflang=\"{}\" href=\"{}\"/>",
                    escape_xml(lang),
                    escape_xml(href)
                )
                .expect("writing to a String");
            }
            xml.push_str("</url>\n");
        }
        xml.push_str("</urlset>\n");
        xml
    }
}

impl IntoResponse for Sitemap {
    fn into_response(self) -> Response {
        ([(header::CONTENT_TYPE, "application/xml; charset=utf-8")], self.to_xml()).into_response()
    }
}

/// `/llms.txt`: a site's summary and links in Markdown, for language models (<https://llmstxt.org>).
///
/// # Examples
///
/// ```
/// use ocre::seo::LlmsTxt;
///
/// let txt = LlmsTxt::new("PureFrame", "AI video upscaling, paid per minute.")
///     .details("Upload a video, get a free preview, then pay to download the full MP4.")
///     .link("Pages", "Pricing", "https://pureframe.example/pricing", "per-minute prices")
///     .link("Pages", "FAQ", "https://pureframe.example/faq", "")
///     .to_string();
/// assert!(txt.starts_with("# PureFrame\n\n> AI video upscaling, paid per minute.\n\nUpload a video,"));
/// let pages = "\n## Pages\n\n- [Pricing](https://pureframe.example/pricing): per-minute prices\n- [FAQ](https://pureframe.example/faq)\n";
/// assert!(txt.ends_with(pages));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LlmsTxt {
    title: String,
    summary: String,
    details: String,
    sections: Vec<(String, Vec<String>)>,
}

impl LlmsTxt {
    /// The site's name (`# title`) and one-line summary (`> summary`).
    pub fn new(title: impl Into<String>, summary: impl Into<String>) -> Self {
        Self { title: title.into(), summary: summary.into(), ..Self::default() }
    }

    /// Free text after the summary.
    #[must_use]
    pub fn details(mut self, text: impl Into<String>) -> Self {
        self.details = text.into();
        self
    }

    /// A link in the `section` list (sections keep the order they first appear in).
    #[must_use]
    pub fn link(mut self, section: &str, title: &str, url: &str, description: &str) -> Self {
        let line = if description.is_empty() {
            format!("- [{title}]({url})")
        } else {
            format!("- [{title}]({url}): {description}")
        };
        match self.sections.iter_mut().find(|(name, _)| name == section) {
            Some((_, lines)) => lines.push(line),
            None => self.sections.push((section.to_owned(), vec![line])),
        }
        self
    }
}

impl std::fmt::Display for LlmsTxt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "# {}\n\n> {}\n", self.title, self.summary)?;
        if !self.details.is_empty() {
            write!(f, "\n{}\n", self.details)?;
        }
        for (section, lines) in &self.sections {
            write!(f, "\n## {section}\n\n{}\n", lines.join("\n"))?;
        }
        Ok(())
    }
}

impl IntoResponse for LlmsTxt {
    fn into_response(self) -> Response {
        ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], self.to_string()).into_response()
    }
}

fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
#[path = "../tests/seo.rs"]
mod tests;
