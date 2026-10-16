// SPDX-License-Identifier: AGPL-3.0-or-later
//! WebSockets (Jetstream) over `tokio-tungstenite` with rustls. The TCP connection goes to an
//! address checked by the SSRF guard (no second resolution), and message and frame sizes are
//! capped by the library before a message is buffered whole (`threat-model.md` INFRA-5).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use lessdugin_core::host::{HostError, WebSocket, WsMessage};
use lessdugin_core::limits;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::error::CapacityError;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::{Host, Url};

use super::net::{Net, USER_AGENT, redact_url};

/// An open WebSocket.
pub struct NativeWs {
    stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl std::fmt::Debug for NativeWs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeWs")
            .field("open", &self.stream.is_some())
            .finish()
    }
}

fn map_ws_error(e: WsError) -> HostError {
    match e {
        WsError::Capacity(CapacityError::MessageTooLong { .. }) => HostError::TooLarge,
        WsError::Capacity(c) => HostError::Network(c.to_string()),
        WsError::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => HostError::Timeout,
        other => HostError::Network(other.to_string()),
    }
}

/// Connects to the first reachable address, each attempt bounded by the connect timeout.
async fn connect_any(addrs: &[SocketAddr]) -> Result<TcpStream, HostError> {
    let timeout = Duration::from_millis(u64::from(limits::CONNECT_TIMEOUT_MS));
    let mut last = HostError::Network("no address to connect to".into());
    for addr in addrs {
        match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => last = HostError::Network(format!("connect: {e}")),
            Err(_) => last = HostError::Timeout,
        }
    }
    Err(last)
}

/// Opens `url` (`wss`, or `ws` for a configured devnet origin) with messages and frames capped
/// at `max_message_bytes`.
pub async fn connect(net: &Net, url: Url, max_message_bytes: u32) -> Result<NativeWs, HostError> {
    if !matches!(url.scheme(), "wss" | "ws") {
        return Err(HostError::Blocked(format!(
            "not a WebSocket URL: {}",
            url.scheme()
        )));
    }
    net.policy().check_url(&url)?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| HostError::Blocked("the URL has no usable port".into()))?;
    let ips: Vec<IpAddr> = match url.host() {
        Some(Host::Domain(d)) => net.resolver().resolve_checked(d).await?,
        Some(Host::Ipv4(ip)) => vec![IpAddr::V4(ip)],
        Some(Host::Ipv6(ip)) => vec![IpAddr::V6(ip)],
        None => return Err(HostError::Blocked("the URL has no host".into())),
    };
    let addrs: Vec<SocketAddr> = ips
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    tracing::debug!(url = %redact_url(&url), "websocket connect");

    let handshake = async {
        let tcp = connect_any(&addrs).await?;
        let mut request = url.as_str().into_client_request().map_err(map_ws_error)?;
        request.headers_mut().insert(
            http::header::USER_AGENT,
            http::HeaderValue::from_static(USER_AGENT),
        );
        let max = max_message_bytes as usize;
        let config = WebSocketConfig::default()
            .max_message_size(Some(max))
            .max_frame_size(Some(max));
        // TLS (for wss) uses the URL's host for SNI and certificate checks; the TCP stream is
        // already connected to the pinned address.
        let (stream, _resp) =
            tokio_tungstenite::client_async_tls_with_config(request, tcp, Some(config), None)
                .await
                .map_err(map_ws_error)?;
        Ok::<_, HostError>(stream)
    };
    let total = Duration::from_millis(u64::from(limits::REQUEST_TIMEOUT_MS));
    let stream = tokio::time::timeout(total, handshake)
        .await
        .map_err(|_| HostError::Timeout)??;
    Ok(NativeWs {
        stream: Some(stream),
    })
}

impl WebSocket for NativeWs {
    async fn next(&mut self) -> Option<Result<WsMessage, HostError>> {
        loop {
            let stream = self.stream.as_mut()?;
            match stream.next().await {
                None => {
                    self.stream = None;
                    return None;
                }
                Some(Ok(Message::Text(t))) => return Some(Ok(WsMessage::Text(t.to_string()))),
                Some(Ok(Message::Binary(b))) => return Some(Ok(WsMessage::Binary(b.to_vec()))),
                // Pings are answered by the library; frames are never yielded when reading.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                Some(Ok(Message::Close(_))) => {
                    self.stream = None;
                    return None;
                }
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => {
                    self.stream = None;
                    return None;
                }
                Some(Err(e)) => {
                    // An oversized message (or any protocol error) ends the stream.
                    if let Some(mut s) = self.stream.take() {
                        let _ = s.close(None).await;
                    }
                    return Some(Err(map_ws_error(e)));
                }
            }
        }
    }

    async fn send_text(&mut self, text: String) -> Result<(), HostError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| HostError::Network("the WebSocket is closed".into()))?;
        stream
            .send(Message::Text(text.into()))
            .await
            .map_err(map_ws_error)
    }

    async fn close(&mut self) {
        if let Some(mut s) = self.stream.take() {
            let _ = s.close(None).await;
        }
    }
}
