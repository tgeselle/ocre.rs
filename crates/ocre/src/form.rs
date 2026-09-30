//! Forms whose field names use brackets, Rails-style: `post[title]`, `tag_ids[]`, `items[0][name]`.

use axum::{
    body::Bytes,
    extract::{FromRequest, Request},
    http::{HeaderMap, Method, header},
    response::{IntoResponse, Response},
};
use serde::{
    Deserializer,
    de::{self, DeserializeOwned, IntoDeserializer, Visitor, value::MapDeserializer, value::SeqDeserializer},
};

use crate::{ApiError, Error, Result};

/// Form extractor for bracketed field names, like Rails' `params` (`post[title]`, `tag_ids[]`, `lines[0][qty]`).
///
/// axum's `Form` only knows flat names (`title=...`); use it for ordinary
/// forms. `NestedForm` is for forms that send a list or nested records:
///
/// | Field name | Becomes |
/// |---|---|
/// | `title` | the field `title` |
/// | `post[title]` | the field `title` of the struct in field `post` |
/// | `tag_ids[]` (repeated) | a `Vec` with one item per value, in order |
/// | `lines[0][qty]`, `lines[1][qty]` | a `Vec` of structs, sorted by index (Rails' `fields_for` / nested attributes) |
/// | `lines[][qty]`, `lines[][price]` | a `Vec` of structs; a name already set in the last item starts a new one |
///
/// A name sent twice keeps the last value (so a hidden `done=0` before the
/// checkbox `done=1` works as in Rails). Values are text: numbers and
/// booleans are parsed from it (`1`/`true`/`on`/`yes` are `true`, `0`,
/// `false`, `off`, `no` and an empty value are `false`), an empty value is
/// `None` for an `Option`, and a single value fills a one-item `Vec`. Enums
/// with unit variants read their variant name.
///
/// On `GET` and `HEAD` it reads the query string; otherwise the body, which
/// must be `application/x-www-form-urlencoded` (2 MB at most, axum's
/// default limit). A body that does not decode into `T` (a missing field, a
/// number that is not one, `a=1&a[b]=2`) is a 400, as an HTML page for
/// browsers (feature `html`) or JSON for API clients. Pure CPU, no binding
/// call.
///
/// # Examples
///
/// ```
/// use ocre::NestedForm;
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize, PartialEq)]
/// struct Order {
///     customer: Customer,
///     #[serde(default)]
///     tag_ids: Vec<i64>,
///     lines: Vec<Line>,
/// }
///
/// #[derive(Debug, Deserialize, PartialEq)]
/// struct Customer {
///     name: String,
/// }
///
/// #[derive(Debug, Deserialize, PartialEq)]
/// struct Line {
///     product: String,
///     qty: u32,
///     #[serde(default)]
///     remove: bool,
/// }
///
/// let body = "customer[name]=Ada&tag_ids[]=3&tag_ids[]=7\
///             &lines[0][product]=Tea&lines[0][qty]=2\
///             &lines[1][product]=Cake&lines[1][qty]=1&lines[1][remove]=0&lines[1][remove]=1";
/// let order: Order = NestedForm::parse(body).unwrap();
/// assert_eq!(order.customer.name, "Ada");
/// assert_eq!(order.tag_ids, [3, 7]);
/// assert_eq!(order.lines[1], Line { product: "Cake".into(), qty: 1, remove: true });
/// ```
///
/// In a handler, like axum's `Form`:
///
/// ```no_run
/// use axum::response::Redirect;
/// use ocre::{NestedForm, Result};
/// # #[derive(serde::Deserialize)] struct Order { lines: Vec<String> }
///
/// async fn create(NestedForm(order): NestedForm<Order>) -> Result<Redirect> {
///     # let _ = order.lines;
///     Ok(Redirect::to("/orders"))
/// }
/// # let _ = create;
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NestedForm<T>(pub T);

impl<T: DeserializeOwned> NestedForm<T> {
    /// Decodes an `application/x-www-form-urlencoded` string (a body or a query string) with bracketed names.
    ///
    /// The rules are those of [`NestedForm`]; errors are
    /// [`Error::BadRequest`] explaining which field failed.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::NestedForm;
    /// use std::collections::BTreeMap;
    ///
    /// // `?filter[status]=open&filter[author]=ada` on an index page.
    /// let filter: BTreeMap<String, BTreeMap<String, String>> =
    ///     NestedForm::parse("filter[status]=open&filter[author]=ada").unwrap();
    /// assert_eq!(filter["filter"]["author"], "ada");
    /// assert!(NestedForm::<BTreeMap<String, u32>>::parse("n=many").is_err());
    /// ```
    pub fn parse(input: &str) -> Result<T> {
        // Decoding pairs cannot fail: invalid percent-escapes and UTF-8 are kept as text.
        let pairs: Vec<(String, String)> = serde_urlencoded::from_str(input).unwrap_or_default();
        let mut root = Vec::new();
        for (name, value) in pairs {
            insert(&mut root, &segments(&name), value)?;
        }
        T::deserialize(Node::Map(root)).map_err(|err| Error::bad_request(format!("Invalid form data: {err}")))
    }
}

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for NestedForm<T> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> std::result::Result<Self, Self::Rejection> {
        let html = wants_html(req.headers());
        read(req, state).await.map(Self).map_err(|err| rejection(err, html))
    }
}

async fn read<T: DeserializeOwned, S: Send + Sync>(req: Request, state: &S) -> Result<T> {
    if req.method() == Method::GET || req.method() == Method::HEAD {
        return NestedForm::parse(req.uri().query().unwrap_or(""));
    }
    let content_type = req.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or("");
    if !content_type.starts_with("application/x-www-form-urlencoded") {
        return Err(Error::bad_request("Expected an application/x-www-form-urlencoded body"));
    }
    let body = Bytes::from_request(req, state).await.map_err(|err| Error::bad_request(err.body_text()))?;
    NestedForm::parse(&String::from_utf8_lossy(&body))
}

/// Browsers ask for HTML; API clients (`curl`, `fetch`) do not.
fn wants_html(headers: &HeaderMap) -> bool {
    headers.get(header::ACCEPT).and_then(|value| value.to_str().ok()).is_some_and(|accept| accept.contains("text/html"))
}

fn rejection(err: Error, html: bool) -> Response {
    #[cfg(feature = "html")]
    if html {
        return err.into_response();
    }
    let _ = html;
    ApiError(err).into_response()
}

/// A decoded form: text values in maps and lists.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Leaf(String),
    Map(Vec<(String, Node)>),
    List(Vec<Node>),
}

/// `a[b][]` is `["a", "b", ""]`; a name that is not well bracketed is one segment.
fn segments(name: &str) -> Vec<&str> {
    let Some(open) = name.find('[').filter(|&open| open > 0 && name.ends_with(']')) else {
        return vec![name];
    };
    let inner = &name[open + 1..name.len() - 1];
    let parts: Vec<&str> = inner.split("][").collect();
    if parts.iter().any(|part| part.contains(['[', ']'])) {
        return vec![name];
    }
    std::iter::once(&name[..open]).chain(parts).collect()
}

fn conflict(key: &str) -> Error {
    Error::bad_request(format!("Invalid form data: `{key}` is both a value and a group of fields"))
}

/// Puts `value` at `path` in `map`, creating groups on the way.
fn insert(map: &mut Vec<(String, Node)>, path: &[&str], value: String) -> Result<()> {
    let (key, rest) = (path[0], &path[1..]);
    let position = map.iter().position(|(name, _)| name == key);
    let Some(next) = rest.first() else {
        match position {
            Some(index) => map[index].1 = Node::Leaf(value),
            None => map.push((key.to_owned(), Node::Leaf(value))),
        }
        return Ok(());
    };
    let index = position.unwrap_or_else(|| {
        let empty = if next.is_empty() { Node::List(Vec::new()) } else { Node::Map(Vec::new()) };
        map.push((key.to_owned(), empty));
        map.len() - 1
    });
    match (&mut map[index].1, next.is_empty()) {
        (Node::Map(child), false) => insert(child, rest, value),
        (Node::List(items), true) => push(items, &rest[1..], value),
        _ => Err(conflict(key)),
    }
}

/// `key[]=v` appends `v`; `key[][name]=v` sets `name` in the last item, or in a new one when it has it.
fn push(items: &mut Vec<Node>, path: &[&str], value: String) -> Result<()> {
    let Some(name) = path.first() else {
        items.push(Node::Leaf(value));
        return Ok(());
    };
    if name.is_empty() {
        // `key[][]=v`: each value is a new one-item list.
        let mut inner = Vec::new();
        let result = push(&mut inner, &path[1..], value);
        items.push(Node::List(inner));
        return result;
    }
    let mut fields = match items.pop() {
        Some(Node::Map(fields)) if !fields.iter().any(|(field, _)| field == name) => fields,
        Some(last) => {
            items.push(last);
            Vec::new()
        }
        None => Vec::new(),
    };
    let result = insert(&mut fields, path, value);
    items.push(Node::Map(fields));
    result
}

type DeError = de::value::Error;

impl Node {
    fn invalid(&self, expected: &str) -> DeError {
        de::Error::custom(match self {
            Node::Leaf(value) => format!("expected {expected}, found `{value}`"),
            _ => format!("expected {expected}, found a group of fields"),
        })
    }

    fn parse<T: std::str::FromStr>(&self, expected: &str) -> std::result::Result<T, DeError> {
        match self {
            Node::Leaf(value) => value.trim().parse().map_err(|_| self.invalid(expected)),
            _ => Err(self.invalid(expected)),
        }
    }
}

impl<'de> IntoDeserializer<'de, DeError> for Node {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

macro_rules! parse_number {
    ($($method:ident => $visit:ident, $expected:literal;)*) => {
        $(fn $method<V: Visitor<'de>>(self, visitor: V) -> std::result::Result<V::Value, DeError> {
            visitor.$visit(self.parse($expected)?)
        })*
    };
}

impl<'de> Deserializer<'de> for Node {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> std::result::Result<V::Value, DeError> {
        match self {
            Node::Leaf(value) => visitor.visit_string(value),
            Node::Map(fields) => visitor.visit_map(MapDeserializer::new(fields.into_iter())),
            Node::List(items) => visitor.visit_seq(SeqDeserializer::new(items.into_iter())),
        }
    }

    parse_number! {
        deserialize_i8 => visit_i8, "an integer";
        deserialize_i16 => visit_i16, "an integer";
        deserialize_i32 => visit_i32, "an integer";
        deserialize_i64 => visit_i64, "an integer";
        deserialize_u8 => visit_u8, "a positive integer";
        deserialize_u16 => visit_u16, "a positive integer";
        deserialize_u32 => visit_u32, "a positive integer";
        deserialize_u64 => visit_u64, "a positive integer";
        deserialize_f32 => visit_f32, "a number";
        deserialize_f64 => visit_f64, "a number";
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> std::result::Result<V::Value, DeError> {
        let value = match &self {
            Node::Leaf(value) => match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" => Some(true),
                "" | "0" | "false" | "off" | "no" => Some(false),
                _ => None,
            },
            _ => None,
        };
        visitor.visit_bool(value.ok_or_else(|| self.invalid("true or false"))?)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> std::result::Result<V::Value, DeError> {
        match &self {
            Node::Leaf(value) if value.is_empty() => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> std::result::Result<V::Value, DeError> {
        match self {
            Node::List(items) => visitor.visit_seq(SeqDeserializer::new(items.into_iter())),
            Node::Map(fields) => {
                // `lines[0][qty]`: a group whose names are all indexes is a list, in index order.
                let mut indexed = Vec::with_capacity(fields.len());
                for (name, node) in fields {
                    let index: u64 = name.parse().map_err(|_| Node::Map(Vec::new()).invalid("a list"))?;
                    indexed.push((index, node));
                }
                indexed.sort_by_key(|(index, _)| *index);
                visitor.visit_seq(SeqDeserializer::new(indexed.into_iter().map(|(_, node)| node)))
            }
            leaf => visitor.visit_seq(SeqDeserializer::new(std::iter::once(leaf))),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> std::result::Result<V::Value, DeError> {
        match self {
            Node::Leaf(value) => visitor.visit_enum(value.into_deserializer()),
            other => Err(other.invalid("one of the choices")),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> std::result::Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    serde::forward_to_deserialize_any! {
        i128 u128 char str string bytes byte_buf unit unit_struct tuple
        tuple_struct map struct identifier ignored_any
    }
}

#[cfg(test)]
#[path = "../tests/form.rs"]
mod tests;
