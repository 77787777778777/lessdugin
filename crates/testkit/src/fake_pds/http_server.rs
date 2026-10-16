// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real HTTP serving (feature `http`): one axum listener on `127.0.0.1:0` per service, on a
//! Tokio runtime owned by the [`super::FakeNet`]. Latency faults sleep; truncation announces the
//! full `Content-Length` and drops the connection after the allowed bytes; disconnects drop it
//! before any body byte. Every response carries `access-control-allow-origin: *`.

use std::sync::Weak;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Request};
use axum::response::Response;

use super::Inner;
use super::server::{Abort, Incoming};

pub(crate) struct Server {
    rt: Option<tokio::runtime::Runtime>,
}

impl Server {
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fake-net-http")
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self { rt: Some(rt) }
    }

    /// Binds a listener and serves the network on it; returns its base URL.
    pub fn bind(&self, inner: Weak<Inner>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        listener.set_nonblocking(true).expect("non-blocking");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        let b = base.clone();
        let rt = self.rt.as_ref().expect("runtime");
        rt.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            let app = Router::new()
                .fallback(move |req: Request| handle(inner.clone(), b.clone(), req))
                .layer(DefaultBodyLimit::disable());
            let _ = axum::serve(listener, app).await;
        });
        base
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

fn plain(status: u16, msg: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(Body::from(msg.to_owned()))
        .expect("response")
}

async fn handle(inner: Weak<Inner>, base: String, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return plain(400, "unreadable body");
    };
    let url = format!(
        "{base}{}",
        parts.uri.path_and_query().map_or("/", |p| p.as_str())
    );
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_ascii_lowercase(), v.to_str().ok()?.to_owned())))
        .collect();
    let caller = headers
        .iter()
        .find(|(k, _)| k == "x-fake-caller")
        .map_or_else(|| "http".to_owned(), |(_, v)| v.clone());
    let body = (parts.method == http::Method::POST || !bytes.is_empty()).then(|| bytes.to_vec());
    let out = {
        let Some(inner) = inner.upgrade() else {
            return plain(503, "the fake network was dropped");
        };
        inner.serve(&Incoming {
            method: &parts.method,
            url: &url,
            headers: &headers,
            body: body.as_deref(),
            caller: &caller,
        })
    };
    if out.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(out.delay_ms)).await;
    }
    let mut b = Response::builder().status(if out.resp.status == 0 {
        200
    } else {
        out.resp.status
    });
    for (k, v) in &out.resp.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    b = b.header("access-control-allow-origin", "*");
    let fail = || std::io::Error::other("fake network fault");
    let body = match out.abort {
        None => Body::from(out.resp.body),
        Some(Abort::Truncate(n)) => {
            let full = out.resp.body.len();
            b = b.header("content-length", full.to_string());
            let mut body = out.resp.body;
            body.truncate(n);
            Body::from_stream(futures::stream::iter([Ok(Bytes::from(body)), Err(fail())]))
        }
        Some(Abort::Disconnect) => {
            Body::from_stream(futures::stream::iter([Err::<Bytes, _>(fail())]))
        }
        Some(Abort::Host(e)) => return plain(502, &e.to_string()),
    };
    b.body(body)
        .unwrap_or_else(|e| plain(500, &format!("bad fake response: {e}")))
}
