use serde_json::{Value, json};

use super::*;

#[test]
fn json_ld_parses_back_to_the_same_data_whatever_it_holds() {
    let data = json!({
        "@type": "FAQPage",
        "name": "</script><script>alert(1)</script> & \u{2028}\u{2029} 'quotes' \"double\"",
        "mainEntity": [{"@type": "Question", "name": "Q?"}],
    });
    let html = json_ld(&data);
    let inner = html.strip_prefix("<script type=\"application/ld+json\">").unwrap().strip_suffix("</script>").unwrap();
    assert!(!inner.contains(['<', '>', '&', '\u{2028}', '\u{2029}']));
    assert_eq!(serde_json::from_str::<Value>(inner).unwrap(), data);
}

#[test]
fn sitemaps_escape_their_values_and_answer_xml() {
    let mut sitemap = Sitemap::new();
    assert!(sitemap.is_empty());
    sitemap.add(SitemapUrl { loc: "https://ex.com/?a=1&b=<2>".into(), ..SitemapUrl::default() });
    sitemap.add_localized(&[], None);
    assert_eq!(sitemap.len(), 1);
    assert!(sitemap.to_xml().contains("<url><loc>https://ex.com/?a=1&amp;b=&lt;2&gt;</loc></url>"));
    let response = sitemap.into_response();
    assert_eq!(response.headers()["content-type"], "application/xml; charset=utf-8");
    let llms = LlmsTxt::new("T", "S").into_response();
    assert_eq!(llms.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(escape_xml("'\""), "&apos;&quot;");
}

#[test]
fn data_that_is_not_json_gives_null() {
    let tuple_keys = std::collections::BTreeMap::from([((1, 2), 3)]);
    assert_eq!(json_ld(&tuple_keys), "<script type=\"application/ld+json\">null</script>");
}

#[test]
fn each_language_version_lists_all_the_others() {
    let mut sitemap = Sitemap::new();
    sitemap.add_localized(
        &[("en", "https://ex.com/en/faq"), ("zh-Hans", "https://ex.com/zh-Hans/faq")],
        Some("2026-10-01"),
    );
    let xml = sitemap.to_xml();
    let alternates = "<xhtml:link rel=\"alternate\" hreflang=\"en\" href=\"https://ex.com/en/faq\"/>\
                      <xhtml:link rel=\"alternate\" hreflang=\"zh-Hans\" href=\"https://ex.com/zh-Hans/faq\"/>\
                      <xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"https://ex.com/en/faq\"/>";
    for loc in ["https://ex.com/en/faq", "https://ex.com/zh-Hans/faq"] {
        assert!(
            xml.contains(&format!("<url><loc>{loc}</loc><lastmod>2026-10-01</lastmod>{alternates}</url>")),
            "{xml}"
        );
    }
}

#[test]
fn llms_txt_groups_links_by_section_in_first_seen_order() {
    let txt = LlmsTxt::new("Shop", "Sells things.")
        .details("More about it.")
        .link("Docs", "Guide", "https://ex.com/guide", "")
        .link("Pages", "Pricing", "https://ex.com/pricing", "per minute")
        .link("Docs", "API", "https://ex.com/api", "JSON")
        .to_string();
    assert_eq!(
        txt,
        "# Shop\n\n> Sells things.\n\nMore about it.\n\n## Docs\n\n- [Guide](https://ex.com/guide)\n- [API](https://ex.com/api): JSON\n\n\
         ## Pages\n\n- [Pricing](https://ex.com/pricing): per minute\n"
    );
}
