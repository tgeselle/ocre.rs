use super::*;

#[test]
fn keeps_safe_markup_and_rebuilds_attributes() {
    assert_eq!(
        sanitize(r#"<P CLASS=intro title='a "quote"'>Hi<br/><img src="/a.png" alt=x onerror=alert(1)></P>"#),
        r#"<p class="intro" title="a &quot;quote&quot;">Hi<br><img src="/a.png" alt="x"></p>"#
    );
    assert_eq!(sanitize("<a href=\"mailto:a@b.c\">m</a>"), "<a href=\"mailto:a@b.c\">m</a>");
    assert_eq!(
        sanitize("<a href=\"#top\">t</a><a href=\"?q=1\">q</a>"),
        "<a href=\"#top\">t</a><a href=\"?q=1\">q</a>"
    );
    assert_eq!(sanitize("<a href=\"a&amp;b\">x</a>"), "<a href=\"a&amp;b\">x</a>", "entities decoded then re-escaped");
    assert_eq!(sanitize("<span disabled>x</span>"), "<span>x</span>");
    assert_eq!(sanitize("<span title>x</span>"), "<span title=\"\">x</span>", "valueless attributes");
}

#[test]
fn removes_scripts_handlers_and_dangerous_urls() {
    for (dirty, clean) in [
        ("<script>alert(1)</script>ok", "ok"),
        ("<SCRIPT type=x>alert(1)</SCRIPT >ok", "ok"),
        ("<style>body{}</style><p>x</p>", "<p>x</p>"),
        ("<script>never closed", ""),
        ("<svg><script>a</script></svg>after", "after"),
        ("<scripty>x</scripty>", "x"),
        ("<a href=\"java\tscript:alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"jav&#x61;script:alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"jav&#97script:alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"javascript&colon;alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\" JAVASCRIPT:alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"data:text/html,x\">x</a>", "<a>x</a>"),
        ("<img src=x onerror=alert(1)>", "<img src=\"x\">"),
        ("<p style=\"background:url(x)\">s</p>", "<p>s</p>"),
        ("<iframe src=\"https://evil\"></iframe>t", "t"),
    ] {
        assert_eq!(sanitize(dirty), clean, "{dirty}");
    }
}

#[test]
fn text_comments_and_broken_markup_are_escaped_or_removed() {
    assert_eq!(sanitize("a<!-- secret -->b<!-- open"), "ab");
    assert_eq!(sanitize("<!doctype html><?php x ?>t<!unclosed"), "t");
    assert_eq!(
        sanitize("1 < 2 > 0 &amp; &#39; &#x27; &copy; & &; &#; &#xZ; &x;"),
        "1 &lt; 2 &gt; 0 &amp; &#39; &#x27; &copy; &amp; &amp;; &amp;#; &amp;#xZ; &amp;x;"
    );
    assert_eq!(sanitize("<p title=\"never closed>x"), "&lt;p title=\"never closed&gt;x");
    assert_eq!(sanitize("<b>bold <i>both</b> after"), "<b>bold <i>both</i></b> after", "misnested tags are closed");
    assert_eq!(sanitize("</p>stray</div>"), "stray");
    assert_eq!(sanitize("<b =x ==y>z</b>"), "<b>z</b>");
    assert_eq!(sanitize("<b é=1>z</b>"), "<b>z</b>");
    assert_eq!(sanitize("<b title=\"&#99999999;&#0;&lt\">z</b>"), "<b title=\"&amp;#99999999;\u{0}&amp;lt\">z</b>");
    assert_eq!(sanitize("<b title=a&amp;b\ttitle2>z</b>"), "<b title=\"a&amp;b\">z</b>");
}

#[test]
fn custom_lists_and_strip_tags() {
    assert_eq!(sanitize_with("<p><em>x</em></p>", &["em"], &[]), "<em>x</em>");
    assert_eq!(sanitize_with("<a onclick=\"x\" href=\"/\">x</a>", &["a"], &["onclick", "href"]), "<a href=\"/\">x</a>");
    assert_eq!(strip_tags("<p>Hi <a href=\"/\">you</a></p><script>x</script>"), "Hi you");
    assert_eq!(strip_tags("<textarea><b>raw</b></textarea>ok"), "ok");
}

#[test]
fn script_escaping() {
    assert_eq!(json_escape("\"\u{2028}\u{2029}&\""), "\"\\u2028\\u2029\\u0026\"");
    assert_eq!(json_escape(r#""</script>""#), r#""\u003c/script\u003e""#);
    assert_eq!(
        escape_javascript("a\\b`${x}`\r\nc\rd\u{2028}\u{2029}<b>"),
        "a\\\\b\\`\\${x}\\`\\nc\\nd\\u2028\\u2029<b>"
    );
    assert_eq!(escape_javascript("'a'\n\"b\"</x>"), "\\'a\\'\\n\\\"b\\\"<\\/x>");
}

#[test]
fn unclosed_markup_at_the_end_of_the_input() {
    assert_eq!(sanitize("<p><b>open"), "<p><b>open</b></p>", "kept elements are closed at the end");
    assert_eq!(sanitize("x<b"), "x&lt;b", "a tag that never ends is text");
    assert_eq!(sanitize("x<b class=y"), "x&lt;b class=y");
    assert_eq!(sanitize("<script>a</scripted>b</script >c"), "c", "only the real closing tag ends the script");
    assert_eq!(sanitize("<style>a</stylex"), "");
}
