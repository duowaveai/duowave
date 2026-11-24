//! A Station that is an ear of the home's centre (docs/plans/home-and-devices.md, stage 4): its socket opened on to the
//! centre's uniproxy, frames both ways (was src/home/ear.rs).

use std::time::Duration;

use axum::extract::ws::{Message as StationMessage, WebSocket};
use axum::http::HeaderMap;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as CentreMessage;

use crate::ear::centre::{centre, EAR_KEY_HEADER};
use crate::secrets::SecretsService;
use crate::settings::SettingsService;

/// How long the centre is given to take a station's socket before the ear serves it itself.
const DIAL: Duration = Duration::from_secs(2);

/// The station's handshake headers that go on to the centre: all but the connection's own — and an ear key the
/// station itself might have sent.
fn forwarded(headers: &HeaderMap) -> Vec<(axum::http::HeaderName, axum::http::HeaderValue)> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            !matches!(name, "host" | "connection" | "upgrade" | "content-length" | EAR_KEY_HEADER)
                && !name.starts_with("sec-websocket-")
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

pub type CentreSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The station's socket opened on to the centre, within `DIAL`; none — the centre is away.
pub async fn dial(
    settings: &SettingsService,
    secrets: &SecretsService,
    uniproxy_path: &str,
    headers: &HeaderMap,
) -> Option<CentreSocket> {
    let (ear, key) = centre(settings, secrets)?;
    let url = format!("ws://{}:{}{}", ear.address, ear.port, uniproxy_path);
    let mut request = url.into_client_request().ok()?;
    for (name, value) in forwarded(headers) {
        request.headers_mut().insert(name, value);
    }
    if let Ok(key) = axum::http::HeaderValue::from_str(&key) {
        request.headers_mut().insert(EAR_KEY_HEADER, key);
    }
    // From this ear's own address when the centre is another pretend station (src/utils/net.rs); the system's else.
    let opened = async {
        let stream = crate::utils::net::dial(&ear.address, ear.port).await?;
        tokio_tungstenite::client_async(request, tokio_tungstenite::MaybeTlsStream::Plain(stream))
            .await
            .map_err(std::io::Error::other)
    };
    match tokio::time::timeout(DIAL, opened).await {
        Ok(Ok((socket, _))) => Some(socket),
        Ok(Err(error)) => {
            tracing::info!(target: "home", "the centre did not take the station's socket: {error}; answering here");
            None
        }
        Err(_) => {
            tracing::info!(target: "home", "the centre did not answer in time; answering here");
            None
        }
    }
}

/// Frames both ways between the station and the centre, until either side closes.
pub async fn bridge(station: WebSocket, centre: CentreSocket) {
    let (mut to_station, mut from_station) = station.split();
    let (mut to_centre, mut from_centre) = centre.split();
    let up = async {
        while let Some(Ok(message)) = from_station.next().await {
            let message = match message {
                StationMessage::Text(text) => CentreMessage::Text(text.as_str().into()),
                StationMessage::Binary(bytes) => CentreMessage::Binary(bytes),
                StationMessage::Ping(bytes) => CentreMessage::Ping(bytes),
                StationMessage::Pong(bytes) => CentreMessage::Pong(bytes),
                StationMessage::Close(_) => {
                    let _ = to_centre.send(CentreMessage::Close(None)).await;
                    break;
                }
            };
            if to_centre.send(message).await.is_err() {
                break;
            }
        }
    };
    let down = async {
        while let Some(Ok(message)) = from_centre.next().await {
            let message = match message {
                CentreMessage::Text(text) => StationMessage::Text(text.as_str().into()),
                CentreMessage::Binary(bytes) => StationMessage::Binary(bytes),
                CentreMessage::Ping(bytes) => StationMessage::Ping(bytes),
                CentreMessage::Pong(bytes) => StationMessage::Pong(bytes),
                CentreMessage::Close(_) => {
                    let _ = to_station.send(StationMessage::Close(None)).await;
                    break;
                }
                CentreMessage::Frame(_) => continue,
            };
            if to_station.send(message).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stations_own_handshake_goes_on_but_the_connections_own_headers_do_not() {
        let mut headers = HeaderMap::new();
        headers.insert("x-uprx-device-id", "BEDROOM01".parse().unwrap());
        headers.insert("authorization", "OAuth station-token".parse().unwrap());
        headers.insert("host", "192.0.2.5:7780".parse().unwrap());
        headers.insert("sec-websocket-key", "abc".parse().unwrap());
        headers.insert("upgrade", "websocket".parse().unwrap());
        let names: Vec<String> = forwarded(&headers).into_iter().map(|(name, _)| name.to_string()).collect();
        assert!(names.contains(&"x-uprx-device-id".to_string()) && names.contains(&"authorization".to_string()));
        assert!(!names.iter().any(|name| name == "host" || name == "upgrade" || name.starts_with("sec-websocket")));
    }
}
