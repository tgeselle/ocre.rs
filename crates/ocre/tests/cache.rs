use std::collections::BTreeMap;

use axum::http::Request;

use super::*;
use crate::support::{block_on, body_text};

#[test]
fn keys_are_1_to_512_bytes() {
    assert!(check_key("posts:v1").is_ok());
    assert!(check_key(&"k".repeat(512)).is_ok());
    for key in [String::new(), "k".repeat(513)] {
        let err = check_key(&key).unwrap_err().to_string();
        assert!(err.contains("KV keys are 1 to 512 bytes"), "{err}");
    }
}

#[test]
fn ttls_start_at_kv_minimum() {
    assert_eq!(ttl_seconds(Duration::from_secs(60)).unwrap(), 60);
    assert_eq!(ttl_seconds(Duration::from_millis(3_600_900)).unwrap(), 3600);
    let err = ttl_seconds(Duration::from_secs(59)).unwrap_err().to_string();
    assert!(err.contains("minimum of 60 seconds") && err.contains("1,000 a day"), "{err}");
}

#[test]
fn values_round_trip_as_json_and_stale_shapes_are_misses() {
    let json = encode("k", &vec![1, 2]).unwrap();
    assert_eq!(json, "[1,2]");
    assert_eq!(decode::<Vec<i32>>("k", &json), Some(vec![1, 2]));
    assert_eq!(decode::<String>("k", &json), None, "a value from an older deploy is recomputed");
    let unserializable: BTreeMap<(i32, i32), i32> = BTreeMap::from([((1, 2), 3)]);
    let err = encode("k", &unserializable).unwrap_err().to_string();
    assert!(err.contains("cannot cache `k`"), "{err}");
}

#[test]
fn missing_binding_names_the_generator() {
    let err = binding_error(&"no such binding").to_string();
    assert!(err.contains("`CACHE`") && err.contains("ocre g cache") && err.contains("no such binding"), "{err}");
}

#[test]
fn cache_control_values() {
    let hour = Duration::from_secs(3600);
    assert_eq!(CacheControl::no_store().to_string(), "no-store");
    assert_eq!(CacheControl::no_cache().stale_while_revalidate(hour).to_string(), "private, no-cache");
    assert_eq!(CacheControl::private(hour).to_string(), "private, max-age=3600");
    assert_eq!(
        CacheControl::public(hour).stale_while_revalidate(Duration::from_secs(30)).to_string(),
        "public, max-age=3600, stale-while-revalidate=30"
    );
    let response = (CacheControl::public(hour), ETag::new("v1"), "body").into_response();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=3600");
    assert_eq!(response.headers()[header::ETAG], ETag::new("v1").as_str());
}

#[test]
fn etags_are_weak_hashes_of_the_version() {
    let tag = ETag::new("v1");
    assert_eq!(tag.as_str(), "W/\"3bfc269594ef649228e9a74bab00f042\"");
    assert_eq!(ETag::of(&"v1").unwrap(), ETag::new("\"v1\""), "`of` hashes the JSON");
    let unserializable: BTreeMap<(i32, i32), i32> = BTreeMap::from([((1, 2), 3)]);
    assert!(ETag::of(&unserializable).unwrap_err().to_string().contains("cannot compute an ETag"));
    let opaque = &tag.as_str()[2..];
    assert!(tag.matches(opaque) && tag.matches(&format!(" {} ", tag.as_str())), "weak comparison");
    assert!(!tag.matches("\"other\""));
}

fn conditional(method: &str, if_none_match: Option<&str>) -> Conditional {
    let mut builder = Request::builder().method(method).uri("/");
    if let Some(value) = if_none_match {
        builder = builder.header("If-None-Match", value);
    }
    let (mut parts, ()) = builder.body(()).unwrap().into_parts();
    let Ok(conditional) = block_on(Conditional::from_request_parts(&mut parts, &()));
    conditional
}

#[test]
fn fresh_only_for_get_and_head_with_a_matching_tag() {
    let tag = ETag::new("v1");
    let list = format!("\"a\", {}", tag.as_str());
    assert!(conditional("GET", Some(tag.as_str())).is_fresh(&tag));
    assert!(conditional("HEAD", Some(&list)).is_fresh(&tag));
    assert!(conditional("GET", Some("*")).is_fresh(&tag));
    assert!(!conditional("GET", Some("\"a\"")).is_fresh(&tag));
    assert!(!conditional("GET", None).is_fresh(&tag));
    assert!(!conditional("POST", Some(tag.as_str())).is_fresh(&tag), "unsafe methods always run");
}

#[test]
fn fresh_when_skips_rendering_for_current_copies() {
    let tag = ETag::new("v1");
    let renders = std::cell::Cell::new(0);
    let render = || {
        renders.set(renders.get() + 1);
        Ok("page")
    };
    let response =
        conditional("GET", Some(tag.as_str())).fresh_when(tag.clone(), CacheControl::no_cache(), render).unwrap();
    assert_eq!(renders.get(), 0, "not rendered");
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()[header::ETAG], tag.as_str());
    assert_eq!(response.headers()[header::CACHE_CONTROL], "private, no-cache");
    assert_eq!(body_text(response), "");

    let stale = conditional("GET", Some("\"old\""));
    let response = stale.fresh_when(tag.clone(), CacheControl::no_cache(), render).unwrap();
    assert_eq!(renders.get(), 1);
    assert_eq!(
        (response.status(), response.headers()[header::ETAG].clone()),
        (StatusCode::OK, tag.as_str().parse().unwrap())
    );
    assert_eq!(body_text(response), "page");

    let failed = stale.fresh_when(tag, CacheControl::no_cache(), || -> Result<&str> { Err(Error::NotFound) });
    assert!(matches!(failed, Err(Error::NotFound)));
}
