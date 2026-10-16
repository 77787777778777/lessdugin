// SPDX-License-Identifier: AGPL-3.0-or-later
//! The OAuth redirect: a one-shot loopback HTTP listener on `127.0.0.1:<port>/callback`, and the
//! paste-back parser for `--no-browser` (`data-model.md` §10.5, `threat-model.md` AUTH-5).
//!
//! Both accept parameters in the `?query` or the `#fragment`, and both check `state` (CSRF) and
//! `iss` (RFC 9207 mix-up protection) before the code is used; PKCE is checked by the
//! authorization server when the code is exchanged with the verifier.

use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use lessdugin_core::engine::{EngineError, ErrorCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use crate::auth::redact::Secret;

/// The longest request head the listener reads.
const MAX_REQUEST_HEAD: usize = 16 * 1024;
/// How long one connection may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections served at once (more wait in the listen backlog), so that a local process that
/// opens connections and sends nothing cannot stall the login.
const MAX_CONNECTIONS: usize = 32;

/// An authorization code that passed the `state` and `iss` checks.
#[derive(Debug)]
pub struct AuthorizationCode(pub(crate) Secret);

/// What the callback must carry.
#[derive(Clone, Debug)]
pub(crate) struct Expected<'a> {
    pub state: &'a str,
    pub issuer: &'a str,
    /// The server announced `authorization_response_iss_parameter_supported`.
    pub iss_required: bool,
}

/// Callback parameters from a query string or fragment.
fn params_of(s: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(s.as_bytes())
        .into_owned()
        .collect()
}

fn get<'p>(params: &'p [(String, String)], key: &str) -> Option<&'p str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Outcome of checking one callback.
#[derive(Debug)]
pub(crate) enum Checked {
    /// A usable code.
    Code(AuthorizationCode),
    /// The authorization server reported an error for our request.
    Denied(EngineError),
    /// Not for our request (wrong or missing `state`): ignored by the listener.
    Foreign,
}

/// Checks callback parameters against the pending request.
pub(crate) fn check(params: &[(String, String)], expected: &Expected<'_>) -> Checked {
    let Some(state) = get(params, "state") else {
        return Checked::Foreign;
    };
    if state != expected.state {
        return Checked::Foreign;
    }
    match get(params, "iss") {
        Some(iss) if iss != expected.issuer => {
            return Checked::Denied(
                EngineError::blocked(format!(
                    "the callback comes from issuer {iss:?}, not {}",
                    expected.issuer
                ))
                .with_detail("kind", "issuerMismatch"),
            );
        }
        None if expected.iss_required => {
            return Checked::Denied(EngineError::blocked(
                "the callback has no iss parameter although the server promised one",
            ));
        }
        _ => {}
    }
    if let Some(err) = get(params, "error") {
        let desc = get(params, "error_description").unwrap_or("");
        let code = if err == "access_denied" {
            ErrorCode::AuthRequired
        } else {
            ErrorCode::InvalidInput
        };
        return Checked::Denied(
            EngineError::new(
                code,
                format!("authorization was not granted: {err} {desc}")
                    .trim_end()
                    .to_owned(),
            )
            .with_detail("error", err),
        );
    }
    match get(params, "code") {
        Some(c) if !c.is_empty() => Checked::Code(AuthorizationCode(Secret::new(c))),
        _ => Checked::Denied(EngineError::invalid_input("the callback has no code")),
    }
}

/// Parses a pasted redirect URL (`--no-browser`). It must be the redirect URI of this request
/// (scheme, host, port and path), with the parameters in the query or the fragment.
pub(crate) fn parse_pasted(
    pasted: &str,
    redirect_uri: &Url,
    expected: &Expected<'_>,
) -> Result<AuthorizationCode, EngineError> {
    let text = pasted.trim();
    let url = Url::parse(text).map_err(|_| {
        EngineError::invalid_input(
            "that is not a URL: paste the whole address of the page that failed to load \
             (it starts with http://127.0.0.1)",
        )
    })?;
    let same_target = url.scheme() == redirect_uri.scheme()
        && url.host() == redirect_uri.host()
        && url.port_or_known_default() == redirect_uri.port_or_known_default()
        && url.path() == redirect_uri.path();
    if !same_target {
        return Err(EngineError::invalid_input(format!(
            "that URL does not go to {}",
            redirect_uri.as_str()
        )));
    }
    let mut params = params_of(url.query().unwrap_or(""));
    if get(&params, "state").is_none() {
        params = params_of(url.fragment().unwrap_or(""));
    }
    match check(&params, expected) {
        Checked::Code(c) => Ok(c),
        Checked::Denied(e) => Err(e),
        Checked::Foreign => Err(EngineError::invalid_input(
            "that redirect does not belong to this login (state mismatch); start again",
        )
        .with_detail("kind", "stateMismatch")),
    }
}

fn page(status: &str, title: &str, text: &str) -> Vec<u8> {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
         <body style=\"font-family:sans-serif;max-width:32em;margin:4em auto\">\
         <h1>{title}</h1><p>{text}</p></body>"
    );
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\n\
         content-length: {}\r\ncache-control: no-store\r\nreferrer-policy: no-referrer\r\n\
         x-content-type-options: nosniff\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Reads one request head; `None` if the connection sent no complete head.
async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    while buf.len() < MAX_REQUEST_HEAD {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n") {
            return String::from_utf8(buf).ok();
        }
    }
    None
}

/// Serves one connection: `None` unless it was a `GET /callback`.
async fn serve(mut stream: TcpStream, expected: &Expected<'_>) -> Option<Checked> {
    let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut stream))
        .await
        .ok()
        .flatten()?;
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if method != "GET" || path != "/callback" {
        let _ = stream
            .write_all(&page("404 Not Found", "Not found", "Nothing here."))
            .await;
        return None;
    }
    let outcome = check(&params_of(query), expected);
    let response = match &outcome {
        Checked::Code(_) => page(
            "200 OK",
            "lessdugin: signed in",
            "Authorization complete. You can close this tab and return to the terminal.",
        ),
        Checked::Denied(_) => page(
            "400 Bad Request",
            "lessdugin: sign-in failed",
            "The authorization did not complete. See the terminal for details.",
        ),
        Checked::Foreign => page(
            "400 Bad Request",
            "lessdugin: unknown request",
            "This callback does not belong to the pending login.",
        ),
    };
    let _ = stream.write_all(&response).await;
    let _ = stream.shutdown().await;
    Some(outcome)
}

/// Waits for the redirect to `GET /callback` on `listener` and returns its code. Requests for
/// other paths, other methods, and callbacks whose `state` is not ours are answered and
/// ignored. Connections are served concurrently (each has [`HEAD_TIMEOUT`] to send its request
/// head), so a silent connection does not hold up the browser's. Fails after `timeout`.
pub(crate) async fn accept(
    listener: &TcpListener,
    expected: &Expected<'_>,
    timeout: Duration,
) -> Result<AuthorizationCode, EngineError> {
    let work = async {
        let mut connections = FuturesUnordered::new();
        loop {
            tokio::select! {
                accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                    if let Ok((stream, _)) = accepted {
                        connections.push(serve(stream, expected));
                    }
                }
                Some(outcome) = connections.next(), if !connections.is_empty() => {
                    match outcome {
                        Some(Checked::Code(c)) => return Ok(c),
                        Some(Checked::Denied(e)) => return Err(e),
                        Some(Checked::Foreign) => {
                            tracing::warn!("ignored a callback that does not belong to this login");
                        }
                        None => {}
                    }
                }
            }
        }
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| {
        EngineError::new(
            ErrorCode::AuthRequired,
            "timed out waiting for the browser to complete the login",
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXP: Expected<'static> = Expected {
        state: "st4te",
        issuer: "https://as.example.com",
        iss_required: true,
    };

    fn redirect() -> Url {
        Url::parse("http://127.0.0.1:53124/callback").unwrap()
    }

    #[test]
    fn pasted_query_and_fragment() {
        let q = "http://127.0.0.1:53124/callback?code=c0de&state=st4te&iss=https%3A%2F%2Fas.example.com";
        assert_eq!(
            parse_pasted(q, &redirect(), &EXP).unwrap().0.expose(),
            "c0de"
        );
        let f = "  http://127.0.0.1:53124/callback#state=st4te&iss=https%3A%2F%2Fas.example.com&code=xyz\n";
        assert_eq!(
            parse_pasted(f, &redirect(), &EXP).unwrap().0.expose(),
            "xyz"
        );
    }

    #[test]
    fn pasted_rejections() {
        let bad = [
            "not a url",
            "http://127.0.0.1:9999/callback?code=c&state=st4te&iss=https%3A%2F%2Fas.example.com",
            "http://localhost:53124/callback?code=c&state=st4te&iss=https%3A%2F%2Fas.example.com",
            "http://127.0.0.1:53124/other?code=c&state=st4te&iss=https%3A%2F%2Fas.example.com",
            "https://127.0.0.1:53124/callback?code=c&state=st4te&iss=https%3A%2F%2Fas.example.com",
        ];
        for b in bad {
            assert_eq!(
                parse_pasted(b, &redirect(), &EXP).unwrap_err().code,
                ErrorCode::InvalidInput,
                "{b}"
            );
        }
        let wrong_state =
            "http://127.0.0.1:53124/callback?code=c&state=other&iss=https%3A%2F%2Fas.example.com";
        let e = parse_pasted(wrong_state, &redirect(), &EXP).unwrap_err();
        assert_eq!(e.detail.unwrap()["kind"], "stateMismatch");
        let wrong_iss = "http://127.0.0.1:53124/callback?code=c&state=st4te&iss=https%3A%2F%2Fevil";
        let e = parse_pasted(wrong_iss, &redirect(), &EXP).unwrap_err();
        assert_eq!(e.code, ErrorCode::Blocked);
        let no_iss = "http://127.0.0.1:53124/callback?code=c&state=st4te";
        assert_eq!(
            parse_pasted(no_iss, &redirect(), &EXP).unwrap_err().code,
            ErrorCode::Blocked
        );
        let denied = "http://127.0.0.1:53124/callback?error=access_denied&state=st4te&iss=https%3A%2F%2Fas.example.com";
        let e = parse_pasted(denied, &redirect(), &EXP).unwrap_err();
        assert_eq!(e.code, ErrorCode::AuthRequired);
    }

    #[tokio::test]
    async fn listener_ignores_foreign_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            let send = |target: &'static str| async move {
                let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap();
                s.write_all(format!("GET {target} HTTP/1.1\r\nhost: x\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).await.unwrap();
                out
            };
            let a = send("/favicon.ico").await;
            let b = send("/callback?code=x&state=forged&iss=https%3A%2F%2Fas.example.com").await;
            let c = send("/callback?code=good&state=st4te&iss=https%3A%2F%2Fas.example.com").await;
            (a, b, c)
        });
        let code = accept(&listener, &EXP, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(code.0.expose(), "good");
        let (a, b, c) = client.await.unwrap();
        assert!(a.starts_with("HTTP/1.1 404"));
        assert!(b.starts_with("HTTP/1.1 400"));
        assert!(c.starts_with("HTTP/1.1 200") && c.contains("no-store"));
    }

    #[tokio::test]
    async fn silent_connections_do_not_stall_the_callback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            // A local process opens connections and sends nothing.
            let mut idle = Vec::new();
            for _ in 0..3 {
                idle.push(
                    tokio::net::TcpStream::connect(("127.0.0.1", port))
                        .await
                        .unwrap(),
                );
            }
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            s.write_all(
                b"GET /callback?code=good&state=st4te&iss=https%3A%2F%2Fas.example.com HTTP/1.1\r\n\r\n",
            )
            .await
            .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            drop(idle);
            out
        });
        // Well within the 10 s each idle connection may take.
        let code = accept(&listener, &EXP, Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(code.0.expose(), "good");
        assert!(client.await.unwrap().starts_with("HTTP/1.1 200"));
    }

    #[tokio::test]
    async fn listener_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let e = accept(&listener, &EXP, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::AuthRequired);
    }
}
