//! Helpers for native unit tests.

use axum::response::Response;

pub use pollster::block_on;

pub fn body_text(response: Response) -> String {
    let bytes = block_on(axum::body::to_bytes(response.into_body(), usize::MAX)).expect("in-memory body");
    String::from_utf8(bytes.to_vec()).expect("UTF-8 body")
}
