// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`HttpResponse`] builders for routes.

use lessdugin_core::host::HttpResponse;

/// A response with a body and content type.
pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![("content-type".to_owned(), content_type.to_owned())],
        body,
    }
}

/// A JSON response.
pub fn json(status: u16, value: &serde_json::Value) -> HttpResponse {
    bytes(
        status,
        "application/json",
        serde_json::to_vec(value).expect("JSON value"),
    )
}

/// A 200 JSON response.
pub fn ok_json(value: &serde_json::Value) -> HttpResponse {
    json(200, value)
}

/// A 200 CAR response.
pub fn car(body: Vec<u8>) -> HttpResponse {
    bytes(200, "application/vnd.ipld.car", body)
}

/// A plain-text response.
pub fn text(status: u16, body: &str) -> HttpResponse {
    bytes(status, "text/plain", body.as_bytes().to_vec())
}

/// An XRPC error response `{"error", "message"}`.
pub fn xrpc_error(status: u16, error: &str, message: &str) -> HttpResponse {
    json(
        status,
        &serde_json::json!({ "error": error, "message": message }),
    )
}

/// A response with no body.
pub fn status(status: u16) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

/// Adds a header (lowercased name).
pub fn with_header(mut resp: HttpResponse, name: &str, value: &str) -> HttpResponse {
    resp.headers
        .push((name.to_ascii_lowercase(), value.to_owned()));
    resp
}
