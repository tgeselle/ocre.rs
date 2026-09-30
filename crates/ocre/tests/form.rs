use std::collections::BTreeMap;

use axum::{
    body::Body,
    http::{Request as HttpRequest, StatusCode},
};
use serde::Deserialize;

use super::*;
use crate::support::{block_on, body_text};

#[derive(Debug, Deserialize, PartialEq)]
struct Post {
    title: String,
    #[serde(default)]
    tag_ids: Vec<i64>,
    published: bool,
    rating: Option<f64>,
    status: Status,
    id: Id,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Draft,
    Live,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Id(u32);

#[derive(Debug, Deserialize, PartialEq)]
struct Envelope {
    post: Post,
}

#[test]
fn nested_names_fill_nested_structs_with_parsed_values() {
    let body = "post[title]=Hi+there&post[tag_ids][]=3&post[tag_ids][]=7&post[published]=0&post[published]=on\
                &post[rating]=&post[status]=live&post[id]=%2042";
    let Envelope { post } = NestedForm::parse(body).unwrap();
    assert_eq!(
        post,
        Post {
            title: "Hi there".into(),
            tag_ids: vec![3, 7],
            published: true,
            rating: None,
            status: Status::Live,
            id: Id(42)
        }
    );
    let post: Post = NestedForm::parse("title=a&tag_ids=5&published=&rating=2.5&status=draft&id=1").unwrap();
    assert_eq!((post.tag_ids, post.published, post.rating, post.status), (vec![5], false, Some(2.5), Status::Draft));
}

#[derive(Debug, Deserialize, PartialEq)]
struct Line {
    qty: u8,
    #[serde(default)]
    note: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Order {
    lines: Vec<Line>,
}

#[test]
fn indexed_and_empty_bracket_groups_become_lists() {
    let nested: BTreeMap<String, Vec<Vec<String>>> = NestedForm::parse("m[][]=a&m[][]=b").unwrap();
    assert_eq!(nested["m"], [["a"], ["b"]], "`m[][]` starts a new item when the last one has a value");
    let order: Order = NestedForm::parse("lines[][qty]=1&lines[][note]=a&lines[][qty]=2").unwrap();
    assert_eq!(order.lines, [Line { qty: 1, note: "a".into() }, Line { qty: 2, note: String::new() }]);
}

#[test]
fn malformed_names_are_plain_names() {
    let map: BTreeMap<String, String> = NestedForm::parse("[a]=1&b[c=2&d[e]f]=3&g[h][i]j]=4&k]=5").unwrap();
    let keys: Vec<&str> = map.keys().map(String::as_str).collect();
    assert_eq!(keys, ["[a]", "b[c", "d[e]f]", "g[h][i]j]", "k]"]);
    assert_eq!(segments("a[b][]"), ["a", "b", ""]);
}

#[test]
fn errors_name_what_is_wrong() {
    let error = |input: &str| NestedForm::<Order>::parse(input).unwrap_err().to_string();
    assert!(error("lines[0][qty]=many").contains("expected a positive integer, found `many`"));
    assert!(error("lines[a][qty]=1").contains("expected a list"));
    assert!(error("lines=1&lines[0][qty]=1").contains("`lines` is both a value and a group"));
    assert!(error("lines[0]=1&lines[0][qty]=1").contains("`0` is both"));
    assert!(error("lines[]=1&lines[][qty]=1").contains("invalid type: string \"1\""));
    assert!(error("lines[0][qty][x]=1").contains("expected a positive integer, found a group"));
    assert!(error("lines=1&lines[]=1").contains("`lines` is both"));
    let post = |input: &str| NestedForm::<Post>::parse(input).unwrap_err().to_string();
    assert!(post("title=a&published=maybe").contains("expected true or false, found `maybe`"));
    assert!(post("title=a&published[x]=1").contains("expected true or false, found a group"));
    assert!(post("title=a&published=1&status[x]=1").contains("expected one of the choices"));
    assert!(post("title=a&published=1&status=gone").contains("unknown variant `gone`"));
    assert!(post("title[x]=a").contains("invalid type: map"));
    assert!(post("title=a&published=1&status=live&rating[x]=1").contains("expected a number, found a group"));
    assert!(NestedForm::<Vec<String>>::parse("a=1").unwrap_err().to_string().contains("expected a list"));
    assert!(NestedForm::<BTreeMap<String, (u8, u8)>>::parse("t[]=1&t[]=2").is_ok());
}

#[derive(Debug, Deserialize, PartialEq)]
struct Small {
    i8: i8,
    i16: i16,
    i32: i32,
    u16: u16,
    u64: u64,
    f32: f32,
}

#[test]
fn every_number_type_parses() {
    let small: Small = NestedForm::parse("i8=-1&i16=-2&i32=-3&u16=4&u64=5&f32=0.5").unwrap();
    assert_eq!(small, Small { i8: -1, i16: -2, i32: -3, u16: 4, u64: 5, f32: 0.5 });
}

fn request(method: &str, uri: &str, content_type: Option<&str>, accept: &str, body: &str) -> Request {
    let mut builder = HttpRequest::builder().method(method).uri(uri).header("accept", accept);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

fn extract(req: Request) -> std::result::Result<Order, (StatusCode, String)> {
    match block_on(NestedForm::<Order>::from_request(req, &())) {
        Ok(NestedForm(order)) => Ok(order),
        Err(response) => Err((response.status(), body_text(response))),
    }
}

#[test]
fn the_extractor_reads_the_query_on_get_and_the_body_otherwise() {
    let form = Some("application/x-www-form-urlencoded; charset=utf-8");
    let order = extract(request("GET", "/o?lines[][qty]=4", None, "", "")).unwrap();
    assert_eq!(order.lines[0].qty, 4);
    assert!(extract(request("HEAD", "/o", None, "", "")).is_err());
    let order = extract(request("POST", "/o", form, "", "lines[0][qty]=2")).unwrap();
    assert_eq!(order.lines[0].qty, 2);
    let (status, body) = extract(request("POST", "/o", Some("application/json"), "", "{}")).unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Expected an application/x-www-form-urlencoded body"), "{body}");
    let (status, body) = extract(request("POST", "/o", form, "text/html", "lines[0][qty]=x")).unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("<"), "browsers get an HTML page: {body}");
}

#[test]
fn unreadable_bodies_are_400() {
    let stream = futures_util::stream::iter([Err::<axum::body::Bytes, _>(std::io::Error::other("reset"))]);
    let req = HttpRequest::builder()
        .method("POST")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from_stream(stream))
        .unwrap();
    let (status, _) = extract(req).unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[derive(Debug, Deserialize, PartialEq)]
struct Tags {
    tags: Vec<String>,
}

#[test]
fn a_list_field_takes_brackets_indexes_or_one_value() {
    let tags = |body: &str| NestedForm::<Tags>::parse(body).unwrap().tags;
    assert_eq!(tags("tags[]=a&tags[]=b"), ["a", "b"]);
    assert_eq!(tags("tags[1]=b&tags[0]=a"), ["a", "b"], "indexes give the order");
    assert_eq!(tags("tags=a"), ["a"]);
}

#[test]
fn untyped_values_keep_the_shape_of_the_names() {
    let value = NestedForm::<serde_json::Value>::parse("a=1&b[c]=2&d[]=3&d[]=4").unwrap();
    assert_eq!(value, serde_json::json!({"a": "1", "b": {"c": "2"}, "d": ["3", "4"]}));
}
