// Windows 网络相关平台辅助。

pub fn proxy_bypass_string(bypass: &[String]) -> String {
    let mut values = Vec::new();
    for item in bypass {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        match sysproxy::utils::ipv4_cidr_to_wildcard(item) {
            Ok(wildcards) => values.extend(wildcards),
            Err(_) => values.push(item.to_string()),
        }
    }
    values.join(";")
}

/// Windows 不需要为 TUN 网卡名增加平台前缀。
pub fn normalize_tun_device(value: &str) -> String {
    value.trim().to_string()
}
