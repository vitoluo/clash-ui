use std::sync::OnceLock;
use std::time::Duration;

use serde::de::DeserializeOwned;

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .build()
            .expect("创建 HTTP 客户端失败")
    })
}

#[derive(Debug)]
pub enum Error {
    Request(reqwest::Error),
    Status(u16, String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(error) => write!(formatter, "HTTP 请求失败: {error}"),
            Self::Status(code, message) => write!(formatter, "HTTP 状态码 {code}: {message}"),
        }
    }
}

impl std::error::Error for Error {}

fn authorize(
    builder: reqwest::RequestBuilder,
    authorization: Option<&str>,
) -> reqwest::RequestBuilder {
    match authorization {
        Some(value) => builder.header(reqwest::header::AUTHORIZATION, value),
        None => builder,
    }
}

pub async fn get_json<T: DeserializeOwned>(
    url: &str,
    authorization: Option<&str>,
    query: Option<&[(&str, &str)]>,
    timeout: Duration,
) -> Result<T, Error> {
    let mut builder = authorize(client().get(url).timeout(timeout), authorization);
    if let Some(query) = query {
        builder = builder.query(query);
    }
    let response = builder.send().await.map_err(Error::Request)?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Status(status.as_u16(), status.to_string()));
    }
    response.json::<T>().await.map_err(Error::Request)
}

pub async fn request_status(
    method: reqwest::Method,
    url: &str,
    authorization: Option<&str>,
    query: Option<&[(&str, &str)]>,
    body: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<(), Error> {
    let mut builder = authorize(
        client().request(method, url).timeout(timeout),
        authorization,
    );
    if let Some(query) = query {
        builder = builder.query(query);
    }
    if let Some(body) = body {
        builder = builder.json(body);
    }
    let response = builder.send().await.map_err(Error::Request)?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Status(status.as_u16(), status.to_string()));
    }
    Ok(())
}

/// 下载文本，并由调用方指定总超时。
pub async fn download_text(url: &str, timeout: Duration) -> Result<String, Error> {
    let response = client()
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .map_err(Error::Request)?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Status(status.as_u16(), status.to_string()));
    }
    response.text().await.map_err(Error::Request)
}
