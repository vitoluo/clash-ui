// Clash 外部控制 API：定义核心数据模型并封装具体端点。

use std::time::Duration;

use serde::Deserialize;

use crate::network::{http as http_client, websocket};

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
    Core(super::core::CoreError),
    Http(http_client::Error),
    Ws(websocket::Error),
    Json(serde_json::Error),
    NoSession,
    InvalidUrl(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Core(error) => write!(f, "核心会话错误: {error}"),
            ApiError::Http(error) => write!(f, "{error}"),
            ApiError::Ws(error) => write!(f, "WebSocket 错误: {error}"),
            ApiError::Json(error) => write!(f, "JSON 解析失败: {error}"),
            ApiError::NoSession => write!(f, "核心会话未建立（未启动或无端口）"),
            ApiError::InvalidUrl(error) => write!(f, "非法 URL: {error}"),
        }
    }
}

impl std::error::Error for ApiError {}

const NORMAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const UPGRADE_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

struct Controller {
    http_base_url: String,
    ws_base_url: String,
    authorization: String,
}

impl Controller {
    fn http_url(&self, path: &str) -> String {
        format!("{}{path}", self.http_base_url)
    }

    fn ws_url(&self, path: &str) -> String {
        format!("{}{path}", self.ws_base_url)
    }
}

fn controller() -> Result<Controller, ApiError> {
    let snapshot = super::core::get_controller_snapshot()
        .map_err(ApiError::Core)?
        .ok_or(ApiError::NoSession)?;
    Ok(Controller {
        http_base_url: format!("http://127.0.0.1:{}", snapshot.port),
        ws_base_url: format!("ws://127.0.0.1:{}", snapshot.port),
        authorization: format!("Bearer {}", snapshot.secret),
    })
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

// ===== Clash API 端点 =====
pub async fn get_version() -> Result<Version, ApiError> {
    get_version_request(NORMAL_REQUEST_TIMEOUT).await
}

/// 仅供同步核心启动流程使用的就绪探测包装。
pub(crate) fn get_version_with_timeout(timeout: Duration) -> Result<Version, ApiError> {
    crate::runtime::block(get_version_request(timeout))
}

async fn get_version_request(timeout: Duration) -> Result<Version, ApiError> {
    let controller = controller()?;
    http_client::get_json(
        &controller.http_url("/version"),
        Some(&controller.authorization),
        None,
        timeout,
    )
    .await
    .map_err(ApiError::Http)
}

pub async fn get_proxies() -> Result<std::collections::HashMap<String, ProxyEntry>, ApiError> {
    let controller = controller()?;
    let response: ProxiesResponse = http_client::get_json(
        &controller.http_url("/proxies"),
        Some(&controller.authorization),
        None,
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)?;
    Ok(response.proxies)
}

pub async fn get_proxy_delay(name: &str, url: &str, timeout: u32) -> Result<u16, ApiError> {
    let controller = controller()?;
    let path = format!("/proxies/{}/delay", encode_path_segment(name));
    let timeout_str = timeout.to_string();
    let query = [("url", url), ("timeout", timeout_str.as_str())];
    #[derive(Deserialize)]
    struct DelayResponse {
        delay: u16,
    }
    let response: DelayResponse = http_client::get_json(
        &controller.http_url(&path),
        Some(&controller.authorization),
        Some(&query),
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)?;
    Ok(response.delay)
}

pub async fn get_group_delay(
    group: &str,
    url: &str,
    timeout: u32,
) -> Result<std::collections::HashMap<String, u16>, ApiError> {
    let controller = controller()?;
    let path = format!("/group/{}/delay", encode_path_segment(group));
    let timeout_str = timeout.to_string();
    let query = [("url", url), ("timeout", timeout_str.as_str())];
    http_client::get_json(
        &controller.http_url(&path),
        Some(&controller.authorization),
        Some(&query),
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

pub async fn select_proxy(group: &str, node: &str) -> Result<(), ApiError> {
    let controller = controller()?;
    let path = format!("/proxies/{}", encode_path_segment(group));
    let body = serde_json::json!({ "name": node });
    http_client::request_status(
        reqwest::Method::PUT,
        &controller.http_url(&path),
        Some(&controller.authorization),
        None,
        Some(&body),
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

pub async fn get_rules() -> Result<Vec<RuleEntry>, ApiError> {
    let controller = controller()?;
    let response: RulesResponse = http_client::get_json(
        &controller.http_url("/rules"),
        Some(&controller.authorization),
        None,
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)?;
    Ok(response.rules)
}

pub async fn get_configs() -> Result<Configs, ApiError> {
    let controller = controller()?;
    http_client::get_json(
        &controller.http_url("/configs"),
        Some(&controller.authorization),
        None,
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

pub async fn put_mode(mode: &str) -> Result<(), ApiError> {
    let controller = controller()?;
    let body = serde_json::json!({ "mode": mode });
    http_client::request_status(
        reqwest::Method::PATCH,
        &controller.http_url("/configs"),
        Some(&controller.authorization),
        None,
        Some(&body),
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

#[allow(dead_code)]
pub async fn patch_configs(patch: &PatchConfigs) -> Result<(), ApiError> {
    let controller = controller()?;
    let body = serde_json::to_value(patch).map_err(ApiError::Json)?;
    http_client::request_status(
        reqwest::Method::PATCH,
        &controller.http_url("/configs"),
        Some(&controller.authorization),
        None,
        Some(&body),
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

/// 更新核心
pub async fn upgrade() -> Result<(), ApiError> {
    let controller = controller()?;
    http_client::request_status(
        reqwest::Method::POST,
        &controller.http_url("/upgrade"),
        Some(&controller.authorization),
        None,
        None,
        UPGRADE_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

/// 请求核心下载并解压在线面板。
pub async fn upgrade_ui() -> Result<(), ApiError> {
    let controller = controller()?;
    http_client::request_status(
        reqwest::Method::POST,
        &controller.http_url("/upgrade/ui"),
        Some(&controller.authorization),
        None,
        None,
        UPGRADE_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

#[allow(dead_code)]
pub async fn get_connections() -> Result<ConnectionSnapshot, ApiError> {
    let controller = controller()?;
    websocket::read_first_json(
        &controller.ws_url("/connections"),
        Some(&controller.authorization),
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Ws)
}

pub async fn close_all_connections() -> Result<(), ApiError> {
    let controller = controller()?;
    http_client::request_status(
        reqwest::Method::DELETE,
        &controller.http_url("/connections"),
        Some(&controller.authorization),
        None,
        None,
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

pub async fn close_connection(id: &str) -> Result<(), ApiError> {
    let controller = controller()?;
    let path = format!("/connections/{}", encode_path_segment(id));
    http_client::request_status(
        reqwest::Method::DELETE,
        &controller.http_url(&path),
        Some(&controller.authorization),
        None,
        None,
        NORMAL_REQUEST_TIMEOUT,
    )
    .await
    .map_err(ApiError::Http)
}

#[cfg(test)]
mod tests {
    use super::{
        encode_path_segment, ConnectionSnapshot, LogLine, MemorySnapshot, ProxyEntry, RuleExtra,
        NORMAL_REQUEST_TIMEOUT, UPGRADE_REQUEST_TIMEOUT,
    };

    #[test]
    fn request_timeout_classes_remain_separate() {
        assert_eq!(NORMAL_REQUEST_TIMEOUT, std::time::Duration::from_secs(5));
        assert_eq!(UPGRADE_REQUEST_TIMEOUT, std::time::Duration::from_secs(120));
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
