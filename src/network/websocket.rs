use std::time::Duration;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

type InnerStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
pub enum Error {
    Request(String),
    Connect(tokio_tungstenite::tungstenite::Error),
    Read(tokio_tungstenite::tungstenite::Error),
    Json(serde_json::Error),
    NoData,
    Timeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(message) => write!(formatter, "构造请求失败: {message}"),
            Self::Connect(error) => write!(formatter, "连接失败: {error}"),
            Self::Read(error) => write!(formatter, "读取失败: {error}"),
            Self::Json(error) => write!(formatter, "JSON 解析失败: {error}"),
            Self::NoData => write!(formatter, "未返回数据"),
            Self::Timeout => write!(formatter, "读取首帧超时"),
        }
    }
}

impl std::error::Error for Error {}

fn build_request(url: &str, authorization: Option<&str>) -> Result<http::Request<()>, Error> {
    let mut request = url
        .into_client_request()
        .map_err(|error| Error::Request(error.to_string()))?;
    if let Some(value) = authorization {
        let value = value
            .parse()
            .map_err(|error: http::header::InvalidHeaderValue| Error::Request(error.to_string()))?;
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, value);
    }
    Ok(request)
}

pub struct JsonStream {
    inner: InnerStream,
}

impl JsonStream {
    pub async fn next<T: DeserializeOwned>(&mut self) -> Result<Option<T>, Error> {
        while let Some(message) = self.inner.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text).map(Some).map_err(Error::Json)
                }
                Ok(Message::Close(_)) => return Ok(None),
                Ok(_) => {}
                Err(error) => return Err(Error::Read(error)),
            }
        }
        Ok(None)
    }
}

pub async fn connect_json(url: &str, authorization: Option<&str>) -> Result<JsonStream, Error> {
    let request = build_request(url, authorization)?;
    let (inner, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(Error::Connect)?;
    Ok(JsonStream { inner })
}

pub async fn read_first_json<T: DeserializeOwned>(
    url: &str,
    authorization: Option<&str>,
    timeout: Duration,
) -> Result<T, Error> {
    tokio::time::timeout(timeout, async {
        let mut stream = connect_json(url, authorization).await?;
        stream.next().await?.ok_or(Error::NoData)
    })
    .await
    .map_err(|_| Error::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::build_request;

    #[test]
    fn request_contains_handshake_and_authorization_headers() {
        let request = build_request("ws://127.0.0.1:20000/traffic", Some("Bearer secret")).unwrap();

        assert!(request
            .headers()
            .contains_key(http::header::SEC_WEBSOCKET_KEY));
        assert_eq!(
            request.headers().get(http::header::AUTHORIZATION).unwrap(),
            "Bearer secret"
        );
    }

    #[test]
    fn request_preserves_path_and_query() {
        let request = build_request(
            "ws://127.0.0.1:20000/logs?level=debug&format=structured",
            Some("Bearer secret"),
        )
        .unwrap();

        assert_eq!(
            request.uri().path_and_query().unwrap().as_str(),
            "/logs?level=debug&format=structured"
        );
    }
}
