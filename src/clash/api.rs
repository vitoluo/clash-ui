// clash 外部控制 API 客户端：封装 clash 核心的 REST 与 WebSocket 通信。
//
// 设计要点（见 .llm/task/004-clash-api-client）：
// - reqwest / tokio-tungstenite 为异步栈；用全局 OnceLock 持有多线程 tokio Runtime。
// - reqwest::Client 全局共享，每次请求现读 port/secret，天然适配核心重启。
// - 后台 WS 流（connections/logs/traffic/memory）经 broadcast 多播；start_streams 幂等，
//   核心停止后通过会话代次使旧任务失效，restart 由新会话重新连接。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

// ===== 模型：仅建模页面/统计确需字段，其余不解析 =====
mod model {
    use serde::{Deserialize, Deserializer};

    fn deserialize_null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de> + Default,
    {
        Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    pub struct Version {
        pub meta: bool,
        pub version: String,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    pub struct ProxyHistory {
        pub time: String,
        pub delay: u16,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    pub struct ProxyEntry {
        pub name: String,
        #[serde(rename = "type")]
        pub type_: String,
        #[serde(default)]
        pub udp: bool,
        #[serde(default)]
        pub uot: bool,
        #[serde(default)]
        pub xudp: bool,
        #[serde(default)]
        pub tfo: bool,
        #[serde(default)]
        pub mptcp: bool,
        #[serde(default)]
        pub smux: bool,
        #[serde(default)]
        pub alive: bool,
        #[serde(default)]
        pub history: Vec<ProxyHistory>,
        #[serde(rename = "provider-name", default)]
        pub provider_name: Option<String>,
        #[serde(rename = "dialer-proxy", default)]
        pub dialer_proxy: Option<String>,
        // 策略组额外字段
        #[serde(default)]
        pub now: Option<String>,
        #[serde(default)]
        pub all: Vec<String>,
        #[serde(rename = "testUrl", default)]
        pub test_url: Option<String>,
        #[serde(default)]
        pub hidden: Option<bool>,
        #[serde(default)]
        pub icon: Option<String>,
        #[serde(rename = "emptyFallback", default)]
        pub empty_fallback: Option<String>,
        #[serde(rename = "expectedStatus", default)]
        pub expected_status: Option<String>,
        #[serde(default)]
        pub fixed: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct ProxiesResponse {
        pub proxies: std::collections::HashMap<String, ProxyEntry>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    pub struct RuleExtra {
        #[serde(default)]
        pub disabled: bool,
        #[serde(rename = "hitCount", default)]
        pub hit_count: u64,
        #[serde(rename = "hitAt", default)]
        pub hit_at: Option<String>,
        #[serde(rename = "missCount", default)]
        pub miss_count: u64,
        #[serde(rename = "missAt", default)]
        pub miss_at: Option<String>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    pub struct RuleEntry {
        pub index: usize,
        #[serde(rename = "type")]
        pub type_: String,
        pub payload: String,
        pub proxy: String,
        pub size: i64,
        #[serde(default)]
        pub extra: Option<RuleExtra>,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct RulesResponse {
        pub rules: Vec<RuleEntry>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize)]
    pub struct Configs {
        #[serde(default)]
        pub mode: String,
        #[serde(rename = "log-level", default)]
        pub log_level: String,
        #[serde(rename = "allow-lan", default)]
        pub allow_lan: bool,
        #[serde(default)]
        pub ipv6: bool,
        #[serde(default)]
        pub port: u16,
        #[serde(rename = "socks-port", default)]
        pub socks_port: u16,
        #[serde(rename = "mixed-port", default)]
        pub mixed_port: u16,
        #[serde(rename = "bind-address", default)]
        pub bind_address: String,
        #[serde(default)]
        pub tun: Option<serde_json::Value>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, serde::Serialize, Default)]
    pub struct PatchConfigs {
        #[serde(rename = "mode", skip_serializing_if = "Option::is_none")]
        pub mode: Option<String>,
        #[serde(rename = "log-level", skip_serializing_if = "Option::is_none")]
        pub log_level: Option<String>,
        #[serde(rename = "allow-lan", skip_serializing_if = "Option::is_none")]
        pub allow_lan: Option<bool>,
        #[serde(rename = "ipv6", skip_serializing_if = "Option::is_none")]
        pub ipv6: Option<bool>,
        #[serde(rename = "port", skip_serializing_if = "Option::is_none")]
        pub port: Option<u16>,
        #[serde(rename = "socks-port", skip_serializing_if = "Option::is_none")]
        pub socks_port: Option<u16>,
        #[serde(rename = "mixed-port", skip_serializing_if = "Option::is_none")]
        pub mixed_port: Option<u16>,
        #[serde(rename = "bind-address", skip_serializing_if = "Option::is_none")]
        pub bind_address: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub tun: Option<serde_json::Value>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct ConnMeta {
        #[serde(default)]
        pub network: String,
        #[serde(rename = "type", default)]
        pub type_: String,
        #[serde(rename = "sourceIP", default)]
        pub source_ip: String,
        #[serde(rename = "destinationIP", default)]
        pub destination_ip: String,
        #[serde(rename = "sourceGeoIP", default)]
        pub source_geo_ip: Option<Vec<String>>,
        #[serde(rename = "destinationGeoIP", default)]
        pub destination_geo_ip: Option<Vec<String>>,
        #[serde(rename = "sourceIPASN", default)]
        pub source_ip_asn: String,
        #[serde(rename = "destinationIPASN", default)]
        pub destination_ip_asn: String,
        #[serde(rename = "sourcePort", default)]
        pub source_port: String,
        #[serde(rename = "destinationPort", default)]
        pub destination_port: String,
        #[serde(rename = "inboundIP", default)]
        pub inbound_ip: String,
        #[serde(rename = "inboundPort", default)]
        pub inbound_port: String,
        #[serde(rename = "inboundName", default)]
        pub inbound_name: String,
        #[serde(rename = "inboundUser", default)]
        pub inbound_user: String,
        #[serde(rename = "rematchName", default)]
        pub rematch_name: String,
        #[serde(default)]
        pub host: String,
        #[serde(rename = "dnsMode", default)]
        pub dns_mode: String,
        #[serde(default)]
        pub uid: u64,
        #[serde(default)]
        pub process: String,
        #[serde(rename = "processPath", default)]
        pub process_path: String,
        #[serde(rename = "specialProxy", default)]
        pub special_proxy: String,
        #[serde(rename = "specialRules", default)]
        pub special_rules: String,
        #[serde(rename = "remoteDestination", default)]
        pub remote_destination: String,
        #[serde(default)]
        pub dscp: u8,
        #[serde(rename = "sniffHost", default)]
        pub sniff_host: String,
        #[serde(flatten)]
        pub extra: serde_json::Map<String, serde_json::Value>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct ConnEntry {
        pub id: String,
        pub metadata: ConnMeta,
        pub upload: u64,
        pub download: u64,
        pub start: String,
        #[serde(default)]
        pub chains: Vec<String>,
        #[serde(rename = "providerChains", default)]
        pub provider_chains: Vec<String>,
        #[serde(default)]
        pub rule: String,
        #[serde(rename = "rulePayload", default)]
        pub rule_payload: String,
        #[serde(flatten)]
        pub extra: serde_json::Map<String, serde_json::Value>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct ConnectionSnapshot {
        #[serde(rename = "downloadTotal", default)]
        pub download_total: u64,
        #[serde(rename = "uploadTotal", default)]
        pub upload_total: u64,
        #[serde(default)]
        pub memory: u64,
        #[serde(default, deserialize_with = "deserialize_null_as_default")]
        pub connections: Vec<ConnEntry>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Deserialize, Default)]
    pub struct MemorySnapshot {
        #[serde(default)]
        pub inuse: u64,
        #[serde(default)]
        pub oslimit: u64,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct LogLine {
        pub time: String,
        pub level: String,
        pub message: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct Traffic {
        pub up: u64,
        pub down: u64,
        #[serde(rename = "upTotal", default)]
        pub up_total: u64,
        #[serde(rename = "downTotal", default)]
        pub down_total: u64,
    }
}

pub use model::*;

// ===== 错误模型 =====
#[allow(dead_code)]
#[derive(Debug)]
pub enum ApiError {
    Http(reqwest::Error),
    Ws(String),
    Json(serde_json::Error),
    NoSession,
    InvalidUrl(String),
    HttpStatus(u16, String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Http(e) => write!(f, "HTTP 请求失败: {e}"),
            ApiError::Ws(e) => write!(f, "WebSocket 错误: {e}"),
            ApiError::Json(e) => write!(f, "JSON 解析失败: {e}"),
            ApiError::NoSession => write!(f, "核心会话未建立（未启动或无端口）"),
            ApiError::InvalidUrl(e) => write!(f, "非法 URL: {e}"),
            ApiError::HttpStatus(code, msg) => write!(f, "HTTP 状态码 {code}: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

// ===== 客户端助手：runtime / client / url / auth =====
static RT: OnceLock<Runtime> = OnceLock::new();
fn rt() -> &'static Runtime {
    RT.get_or_init(|| Runtime::new().expect("创建 tokio runtime 失败"))
}

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(reqwest::Client::new)
}

fn base_url() -> Result<String, ApiError> {
    match crate::clash::core::get_port() {
        Some(port) => Ok(format!("http://127.0.0.1:{port}")),
        None => Err(ApiError::NoSession),
    }
}

fn auth_header() -> String {
    format!(
        "Bearer {}",
        crate::clash::core::get_secret().unwrap_or_default()
    )
}

/// 将字符串编码为 RFC3986 URL 路径段。
fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
            encoded.push(char::from(b"0123456789ABCDEF"[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

async fn get_json<T: DeserializeOwned>(
    path: &str,
    query: Option<&[(&str, &str)]>,
) -> Result<T, ApiError> {
    let url = base_url()?;
    let mut b = client()
        .get(format!("{url}{path}"))
        .header(reqwest::header::AUTHORIZATION, auth_header());
    if let Some(q) = query {
        b = b.query(q);
    }
    let resp = b.send().await.map_err(ApiError::Http)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ApiError::HttpStatus(status.as_u16(), status.to_string()));
    }
    resp.json::<T>().await.map_err(ApiError::Http)
}

async fn req_status(
    method: reqwest::Method,
    path: &str,
    query: Option<&[(&str, &str)]>,
    body: Option<&serde_json::Value>,
) -> Result<(), ApiError> {
    let snapshot = crate::clash::core::get_controller_snapshot().ok_or(ApiError::NoSession)?;
    let url = format!("http://127.0.0.1:{}", snapshot.port);
    let mut b = client().request(method, format!("{url}{path}")).header(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", snapshot.secret),
    );
    if let Some(q) = query {
        b = b.query(q);
    }
    if let Some(j) = body {
        b = b.json(j);
    }
    let resp = b.send().await.map_err(ApiError::Http)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ApiError::HttpStatus(status.as_u16(), status.to_string()));
    }
    Ok(())
}

fn build_ws_request(port: u16, secret: &str, path: &str) -> Result<http::Request<()>, ApiError> {
    let ws_url = format!("ws://127.0.0.1:{port}{path}");
    let mut request = ws_url
        .into_client_request()
        .map_err(|_| ApiError::Ws(format!("构造 {path} 请求失败")))?;
    let authorization = format!("Bearer {secret}")
        .parse()
        .map_err(|_| ApiError::Ws(format!("构造 {path} 请求失败")))?;
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, authorization);
    Ok(request)
}

#[allow(dead_code)]
async fn read_ws_first<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let port = crate::clash::core::get_port().ok_or(ApiError::NoSession)?;
    let secret = crate::clash::core::get_secret().unwrap_or_default();
    let request = build_ws_request(port, &secret, path)?;
    let (mut stream, _) = connect_async(request)
        .await
        .map_err(|_| ApiError::Ws(format!("连接 {path} 失败")))?;

    while let Some(message) = stream.next().await {
        match message {
            Ok(Message::Text(text)) => return serde_json::from_str(&text).map_err(ApiError::Json),
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => return Err(ApiError::Ws(format!("读取 {path} 失败"))),
        }
    }

    Err(ApiError::Ws(format!("{path} 未返回数据")))
}

// ===== 公共 REST 接口（同步阻塞） =====
/// 在全局 tokio runtime 上阻塞执行异步任务，供同步上下文（Slint 回调、core）调用。
pub fn block<F: std::future::Future>(fut: F) -> F::Output {
    rt().block_on(fut)
}

pub fn get_version() -> Result<Version, ApiError> {
    block(async { get_json("/version", None).await })
}

pub fn get_proxies() -> Result<std::collections::HashMap<String, ProxyEntry>, ApiError> {
    block(async {
        let r: ProxiesResponse = get_json("/proxies", None).await?;
        Ok(r.proxies)
    })
}

pub fn get_proxy_delay(name: &str, url: &str, timeout: u32) -> Result<u16, ApiError> {
    block(async {
        let path = format!("/proxies/{}/delay", encode_path_segment(name));
        let timeout_str = timeout.to_string();
        let q = [("url", url), ("timeout", timeout_str.as_str())];
        #[derive(Deserialize)]
        struct DelayResp {
            delay: u16,
        }
        let r: DelayResp = get_json(&path, Some(&q[..])).await?;
        Ok(r.delay)
    })
}

pub fn get_group_delay(
    group: &str,
    url: &str,
    timeout: u32,
) -> Result<std::collections::HashMap<String, u16>, ApiError> {
    block(async {
        let path = format!("/group/{}/delay", encode_path_segment(group));
        let timeout_str = timeout.to_string();
        let q = [("url", url), ("timeout", timeout_str.as_str())];
        get_json(&path, Some(&q[..])).await
    })
}

pub fn select_proxy(group: &str, node: &str) -> Result<(), ApiError> {
    block(async {
        let path = format!("/proxies/{}", encode_path_segment(group));
        let body = serde_json::json!({ "name": node });
        req_status(reqwest::Method::PUT, &path, None, Some(&body)).await
    })
}

pub fn get_rules() -> Result<Vec<RuleEntry>, ApiError> {
    block(async {
        let r: RulesResponse = get_json("/rules", None).await?;
        Ok(r.rules)
    })
}

pub fn get_configs() -> Result<Configs, ApiError> {
    block(async { get_json("/configs", None).await })
}

pub fn put_mode(mode: &str) -> Result<(), ApiError> {
    block(async {
        let body = serde_json::json!({ "mode": mode });
        req_status(reqwest::Method::PATCH, "/configs", None, Some(&body)).await
    })
}

#[allow(dead_code)]
pub fn patch_configs(patch: &PatchConfigs) -> Result<(), ApiError> {
    block(async {
        let body = serde_json::to_value(patch).map_err(ApiError::Json)?;
        req_status(reqwest::Method::PATCH, "/configs", None, Some(&body)).await
    })
}

/// 请求核心下载并解压在线面板。
pub fn upgrade_ui() -> Result<(), ApiError> {
    block(async { req_status(reqwest::Method::POST, "/upgrade/ui", None, None).await })
}

#[allow(dead_code)]
pub fn get_connections() -> Result<ConnectionSnapshot, ApiError> {
    block(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            read_ws_first::<ConnectionSnapshot>("/connections"),
        )
        .await
        .map_err(|_| ApiError::Ws("读取 /connections 首帧超时".to_string()))?
    })
}

pub fn latest_memory() -> Option<MemorySnapshot> {
    memory_latest()
        .read()
        .ok()
        .and_then(|snapshot| snapshot.clone())
}

pub fn close_all_connections() -> Result<(), ApiError> {
    block(async { req_status(reqwest::Method::DELETE, "/connections", None, None).await })
}

pub fn close_connection(id: &str) -> Result<(), ApiError> {
    block(async {
        let path = format!("/connections/{}", encode_path_segment(id));
        req_status(reqwest::Method::DELETE, &path, None, None).await
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        build_ws_request, encode_path_segment, ConnectionSnapshot, LogLine, MemorySnapshot,
        ProxyEntry, RuleExtra, LOGS_PATH,
    };

    #[test]
    fn ws_request_contains_tungstenite_handshake_headers() {
        let request = build_ws_request(20000, "secret", "/traffic").unwrap();

        assert!(request
            .headers()
            .contains_key(http::header::SEC_WEBSOCKET_KEY));
        assert_eq!(
            request.headers().get(http::header::AUTHORIZATION).unwrap(),
            "Bearer secret"
        );
    }

    #[test]
    fn structured_log_request_uses_debug_and_structured_queries() {
        let request = build_ws_request(20000, "secret", LOGS_PATH).unwrap();
        assert_eq!(
            request.uri().path_and_query().unwrap().as_str(),
            "/logs?level=debug&format=structured"
        );
    }

    #[test]
    fn structured_log_response_reads_required_fields_and_ignores_fields() {
        let line: LogLine = serde_json::from_value(serde_json::json!({
            "time": "08:00:01",
            "level": "warning",
            "message": "连接失败",
            "fields": [{"key": "host", "value": "example.com"}]
        }))
        .unwrap();

        assert_eq!(line.time, "08:00:01");
        assert_eq!(line.level, "warning");
        assert_eq!(line.message, "连接失败");
    }

    #[test]
    fn structured_log_response_preserves_warn_level_for_domain_normalization() {
        let line: LogLine = serde_json::from_value(serde_json::json!({
            "time": "08:00:02",
            "level": "warn",
            "message": "核心警告"
        }))
        .unwrap();

        assert_eq!(line.level, "warn");
    }

    #[test]
    fn encodes_path_segment_reserved_characters() {
        assert_eq!(encode_path_segment("A proxy"), "A%20proxy");
        assert_eq!(encode_path_segment("节点"), "%E8%8A%82%E7%82%B9");
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(encode_path_segment("a?b#c%"), "a%3Fb%23c%25");
        assert_eq!(encode_path_segment("-._~"), "-._~");
    }

    #[test]
    fn connection_response_preserves_official_and_unknown_fields() {
        let response: serde_json::Value = serde_json::from_str(
            r#"
            {
                "downloadTotal": 100,
                "uploadTotal": 200,
                "memory": 300,
                "connections": [{
                    "id": "connection-1",
                    "upload": 10,
                    "download": 20,
                    "start": "2026-08-11T08:00:00Z",
                    "chains": ["Proxy"],
                    "providerChains": ["Provider"],
                    "rule": "MATCH",
                    "rulePayload": "DIRECT",
                    "metadata": {
                        "network": "tcp",
                        "type": "HTTP",
                        "sourceIP": "127.0.0.1",
                        "destinationIP": "192.0.2.1",
                        "sourceGeoIP": ["CN"],
                        "destinationGeoIP": [],
                        "sourceIPASN": "AS64500",
                        "destinationIPASN": "AS64501",
                        "sourcePort": "12345",
                        "destinationPort": "443",
                        "inboundIP": "127.0.0.1",
                        "inboundPort": "7890",
                        "inboundName": "mixed",
                        "inboundUser": "user",
                        "rematchName": "",
                        "host": "example.com",
                        "dnsMode": "normal",
                        "uid": 0,
                        "process": "browser",
                        "processPath": "C:/browser.exe",
                        "specialProxy": "",
                        "specialRules": "",
                        "remoteDestination": "example.com:443",
                        "dscp": 0,
                        "sniffHost": "example.com",
                        "metadataUnknown": {"enabled": true},
                        "metadataNull": null
                    },
                    "entryUnknown": {"items": [1, null, false]}
                }]
            }
            "#,
        )
        .unwrap();

        let snapshot: ConnectionSnapshot = serde_json::from_value(response).unwrap();
        let connection = &snapshot.connections[0];
        assert_eq!(connection.metadata.source_ip, "127.0.0.1");
        assert_eq!(
            connection.metadata.destination_geo_ip,
            Some(Vec::<String>::new())
        );
        assert_eq!(connection.metadata.inbound_port, "7890");
        assert_eq!(connection.metadata.uid, 0);
        assert_eq!(
            connection.metadata.extra["metadataUnknown"]["enabled"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            connection.metadata.extra["metadataNull"],
            serde_json::Value::Null
        );
        assert_eq!(
            connection.extra["entryUnknown"]["items"][1],
            serde_json::Value::Null
        );
    }

    #[test]
    fn connection_response_accepts_missing_optional_values() {
        let response = serde_json::json!({
            "connections": [{
                "id": "connection-2",
                "metadata": {},
                "upload": 0,
                "download": 0,
                "start": "",
                "rule": "",
                "rulePayload": ""
            }]
        });

        let snapshot: ConnectionSnapshot = serde_json::from_value(response).unwrap();
        let connection = &snapshot.connections[0];
        assert!(connection.metadata.host.is_empty());
        assert!(connection.metadata.source_geo_ip.is_none());
        assert!(connection.metadata.extra.is_empty());
        assert!(connection.extra.is_empty());
    }

    #[test]
    fn connection_response_treats_null_connections_as_empty() {
        let response = serde_json::json!({
            "downloadTotal": 0,
            "uploadTotal": 0,
            "connections": null,
            "memory": 0
        });

        let snapshot: ConnectionSnapshot = serde_json::from_value(response).unwrap();
        assert!(snapshot.connections.is_empty());
    }

    #[test]
    fn connection_response_preserves_geo_ip_null_empty_and_values() {
        let response = serde_json::json!({
            "connections": [
                {
                    "id": "null",
                    "metadata": {
                        "sourceGeoIP": null,
                        "destinationGeoIP": null
                    },
                    "upload": 0,
                    "download": 0,
                    "start": ""
                },
                {
                    "id": "empty",
                    "metadata": {
                        "sourceGeoIP": [],
                        "destinationGeoIP": []
                    },
                    "upload": 0,
                    "download": 0,
                    "start": ""
                },
                {
                    "id": "values",
                    "metadata": {
                        "sourceGeoIP": ["CN"],
                        "destinationGeoIP": ["US"]
                    },
                    "upload": 0,
                    "download": 0,
                    "start": ""
                }
            ]
        });

        let snapshot: ConnectionSnapshot = serde_json::from_value(response).unwrap();
        assert_eq!(snapshot.connections[0].metadata.source_geo_ip, None);
        assert_eq!(snapshot.connections[0].metadata.destination_geo_ip, None);
        assert_eq!(
            snapshot.connections[1].metadata.source_geo_ip,
            Some(Vec::new())
        );
        assert_eq!(
            snapshot.connections[1].metadata.destination_geo_ip,
            Some(Vec::new())
        );
        assert_eq!(
            snapshot.connections[2].metadata.source_geo_ip,
            Some(vec!["CN".to_string()])
        );
        assert_eq!(
            snapshot.connections[2].metadata.destination_geo_ip,
            Some(vec!["US".to_string()])
        );
    }

    #[test]
    fn api_models_map_actual_proxy_rule_and_memory_fields() {
        let proxy: ProxyEntry = serde_json::from_value(serde_json::json!({
            "name": "Group",
            "type": "Selector",
            "provider-name": "provider",
            "dialer-proxy": "DIRECT",
            "testUrl": "https://example.com",
            "emptyFallback": "DIRECT",
            "expectedStatus": "204"
        }))
        .unwrap();
        assert_eq!(proxy.provider_name.as_deref(), Some("provider"));
        assert_eq!(proxy.dialer_proxy.as_deref(), Some("DIRECT"));
        assert_eq!(proxy.test_url.as_deref(), Some("https://example.com"));
        assert_eq!(proxy.empty_fallback.as_deref(), Some("DIRECT"));
        assert_eq!(proxy.expected_status.as_deref(), Some("204"));

        let extra: RuleExtra = serde_json::from_value(serde_json::json!({
            "disabled": true,
            "hitCount": 3,
            "hitAt": "2026-08-12T00:00:00Z",
            "missCount": 4,
            "missAt": "2026-08-12T00:01:00Z"
        }))
        .unwrap();
        assert!(extra.disabled);
        assert_eq!(extra.hit_count, 3);
        assert_eq!(extra.hit_at.as_deref(), Some("2026-08-12T00:00:00Z"));
        assert_eq!(extra.miss_count, 4);
        assert_eq!(extra.miss_at.as_deref(), Some("2026-08-12T00:01:00Z"));

        let memory: MemorySnapshot = serde_json::from_value(serde_json::json!({
            "inuse": 1024,
            "oslimit": 0
        }))
        .unwrap();
        assert_eq!(memory.inuse, 1024);
        assert_eq!(memory.oslimit, 0);
    }
}

// ===== WebSocket 后台流 =====
static LOGS_TX: OnceLock<broadcast::Sender<LogLine>> = OnceLock::new();
static CONNS_TX: OnceLock<broadcast::Sender<ConnectionSnapshot>> = OnceLock::new();
static TRAFFIC_TX: OnceLock<broadcast::Sender<Traffic>> = OnceLock::new();
static MEMORY_TX: OnceLock<broadcast::Sender<MemorySnapshot>> = OnceLock::new();
static MEMORY_LATEST: OnceLock<RwLock<Option<MemorySnapshot>>> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static STREAM_GENERATION: AtomicU64 = AtomicU64::new(0);
const LOGS_PATH: &str = "/logs?level=debug&format=structured";

fn memory_latest() -> &'static RwLock<Option<MemorySnapshot>> {
    MEMORY_LATEST.get_or_init(|| RwLock::new(None))
}

fn update_memory(snapshot: &MemorySnapshot) {
    if let Ok(mut latest) = memory_latest().write() {
        *latest = Some(snapshot.clone());
    }
}

fn stream_is_current(generation: u64) -> bool {
    STREAM_GENERATION.load(Ordering::SeqCst) == generation
}

fn mark_streams_stopped(generation: u64) {
    if stream_is_current(generation) {
        STARTED.store(false, Ordering::SeqCst);
    }
}

fn ensure_senders() {
    let _ = LOGS_TX.get_or_init(|| broadcast::channel(1024).0);
    let _ = CONNS_TX.get_or_init(|| broadcast::channel(64).0);
    let _ = TRAFFIC_TX.get_or_init(|| broadcast::channel(256).0);
    let _ = MEMORY_TX.get_or_init(|| broadcast::channel(64).0);
}

#[allow(dead_code)]
pub fn logs_rx() -> Option<broadcast::Receiver<LogLine>> {
    ensure_senders();
    LOGS_TX.get().map(|s| s.subscribe())
}

pub fn conns_rx() -> Option<broadcast::Receiver<ConnectionSnapshot>> {
    ensure_senders();
    CONNS_TX.get().map(|s| s.subscribe())
}

pub fn traffic_rx() -> Option<broadcast::Receiver<Traffic>> {
    ensure_senders();
    TRAFFIC_TX.get().map(|s| s.subscribe())
}

#[allow(dead_code)]
pub fn memory_rx() -> Option<broadcast::Receiver<MemorySnapshot>> {
    ensure_senders();
    MEMORY_TX.get().map(|s| s.subscribe())
}

/// 使当前核心会话的流任务失效，并清除缓存的核心内存。
pub fn reset_streams() {
    STREAM_GENERATION.fetch_add(1, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
    if let Ok(mut latest) = memory_latest().write() {
        *latest = None;
    }
}

/// 启动后台 WS 流（幂等）。核心启动后调用，不依赖页面是否打开。
pub fn start_streams() {
    ensure_senders();
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let generation = STREAM_GENERATION.load(Ordering::SeqCst);
    let logs = LOGS_TX.get().unwrap();
    let conns = CONNS_TX.get().unwrap();
    let traffic = TRAFFIC_TX.get().unwrap();
    let memory = MEMORY_TX.get().unwrap();
    rt().spawn(ws_loop("/connections", conns, generation, |_| {}));
    rt().spawn(ws_loop(LOGS_PATH, logs, generation, |_| {}));
    rt().spawn(ws_loop("/traffic", traffic, generation, |_| {}));
    rt().spawn(ws_loop("/memory", memory, generation, update_memory));
}

async fn ws_loop<T, F>(
    path: &'static str,
    tx: &'static broadcast::Sender<T>,
    generation: u64,
    on_value: F,
) where
    T: DeserializeOwned + Clone + Send + 'static,
    F: Fn(&T) + Send + Sync + 'static,
{
    loop {
        if !stream_is_current(generation) {
            return;
        }
        // 核心未运行则退出并允许后续重启重新拉起。
        let port = match crate::clash::core::get_port() {
            Some(p) => p,
            None => {
                mark_streams_stopped(generation);
                return;
            }
        };
        let secret = crate::clash::core::get_secret().unwrap_or_default();
        let req = match build_ws_request(port, &secret, path) {
            Ok(r) => r,
            Err(_) => {
                eprintln!("WS {path} 请求构造失败");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let ws_stream = match connect_async(req).await {
            Ok((s, _)) => s,
            Err(error) => {
                eprintln!("WS {path} 连接失败: {error}");
                if !stream_is_current(generation) {
                    return;
                }
                if crate::clash::core::get_port().is_none() {
                    mark_streams_stopped(generation);
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let mut ws_stream = ws_stream;
        while let Some(msg) = ws_stream.next().await {
            if !stream_is_current(generation) {
                return;
            }
            match msg {
                Ok(Message::Text(t)) => match serde_json::from_str::<T>(&t) {
                    Ok(v) => {
                        on_value(&v);
                        let _ = tx.send(v);
                    }
                    Err(_) => eprintln!("WS {path} 消息解析失败"),
                },
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => {
                    eprintln!("WS {path} 消息读取失败");
                    break;
                }
            }
        }
        if !stream_is_current(generation) {
            return;
        }
        // 断开后：若核心已停止则退出，否则退避重连。
        if crate::clash::core::get_port().is_none() {
            mark_streams_stopped(generation);
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
