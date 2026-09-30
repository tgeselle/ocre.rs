use super::*;

#[test]
fn options_come_in_a_fixed_order_with_format_auto_last() {
    let all = Variant::new().quality(75).fit(Fit::ScaleDown).height(200).width(300);
    assert_eq!(
        all.path("/photos/1/image"),
        "/cdn-cgi/image/width=300,height=200,fit=scale-down,quality=75,format=auto/photos/1/image"
    );
    assert_eq!(Variant::default(), Variant::new());
    assert_eq!(Variant::new().path("a.png"), "/cdn-cgi/image/format=auto/a.png");
}

#[test]
fn every_fit_has_its_cloudflare_name() {
    let names: Vec<&str> =
        [Fit::ScaleDown, Fit::Contain, Fit::Cover, Fit::Crop, Fit::Pad].into_iter().map(Fit::as_str).collect();
    assert_eq!(names, ["scale-down", "contain", "cover", "crop", "pad"]);
}

#[test]
fn quality_is_clamped_to_1_through_100() {
    assert_eq!(Variant::new().quality(0), Variant::new().quality(1));
    assert_eq!(Variant::new().quality(101), Variant::new().quality(100));
    assert_eq!(Variant::new().quality(50).path("/a"), "/cdn-cgi/image/quality=50,format=auto/a");
}

#[test]
fn absolute_sources_are_kept_whole() {
    let thumb = Variant::new().width(10);
    assert_eq!(thumb.path("http://example.com/a.png"), "/cdn-cgi/image/width=10,format=auto/http://example.com/a.png");
    assert_eq!(
        thumb.path("https://example.com/a.png"),
        "/cdn-cgi/image/width=10,format=auto/https://example.com/a.png"
    );
    assert_eq!(thumb.path("///a.png"), "/cdn-cgi/image/width=10,format=auto/a.png");
}
