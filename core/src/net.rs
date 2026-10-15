//! 位址字串處理（IPv4 / IPv6 共用）
//!
//! SIP / SDP 會用到三種形式：`ip:port`（IPv6 為 `[ip]:port`）、單獨的 IP、或 `host:port`。
//! 不可以用 `split(':')` 取 IP：IPv6 位址本身就含冒號。

use std::net::{IpAddr, SocketAddr};

/// 取出位址中的主機部分（IPv6 不含中括號）：
/// `10.0.0.1:5060` → `10.0.0.1`、`[2001:db8::1]:5060` → `2001:db8::1`、
/// `2001:db8::1` → `2001:db8::1`、`pbx.example:5060` → `pbx.example`
pub fn host(addr: &str) -> String {
    let addr = addr.trim();
    if let Ok(sa) = addr.parse::<SocketAddr>() {
        return sa.ip().to_string();
    }
    if let Ok(ip) = addr.parse::<IpAddr>() {
        return ip.to_string();
    }
    if let Some(rest) = addr.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest).to_string();
    }
    match addr.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h.to_string(),
        _ => addr.to_string(),
    }
}

/// SIP URI 用的主機（IPv6 加中括號）：`[2001:db8::1]:5060` → `[2001:db8::1]`、`10.0.0.1:5060` → `10.0.0.1`
pub fn uri_host(addr: &str) -> String {
    let h = host(addr);
    if is_ipv6(&h) {
        format!("[{}]", h)
    } else {
        h
    }
}

/// 組成 `ip:port`（IPv6 加中括號）
pub fn join(ip: &str, port: u16) -> String {
    let ip = ip.trim_start_matches('[').trim_end_matches(']');
    if ip.contains(':') {
        format!("[{}]:{}", ip, port)
    } else {
        format!("{}:{}", ip, port)
    }
}

/// 是否為 IPv6 位址（可含中括號）
pub fn is_ipv6(ip: &str) -> bool {
    ip.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::Ipv6Addr>()
        .is_ok()
}

/// 連往 `target` 時使用的萬用綁定位址（系統分配 port）
pub fn unspecified_for(target: &SocketAddr) -> &'static str {
    if target.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_handles_ipv4_ipv6_and_names() {
        assert_eq!(host("10.0.0.1:5060"), "10.0.0.1");
        assert_eq!(host("10.0.0.1"), "10.0.0.1");
        assert_eq!(host("[2001:db8::1]:5060"), "2001:db8::1");
        assert_eq!(host("[2001:db8::1]"), "2001:db8::1");
        assert_eq!(
            host("2001:db8::1"),
            "2001:db8::1",
            "沒有中括號的 IPv6 不可被截斷"
        );
        assert_eq!(host("::1"), "::1");
        assert_eq!(host("pbx.example:5060"), "pbx.example");
        assert_eq!(host("pbx.example"), "pbx.example");
    }

    #[test]
    fn uri_host_brackets_ipv6_only() {
        assert_eq!(uri_host("[2001:db8::1]:5070"), "[2001:db8::1]");
        assert_eq!(uri_host("2001:db8::1"), "[2001:db8::1]");
        assert_eq!(uri_host("10.0.0.1:5070"), "10.0.0.1");
        assert_eq!(uri_host("pbx.example"), "pbx.example");
    }

    #[test]
    fn join_brackets_ipv6() {
        assert_eq!(join("10.0.0.1", 16000), "10.0.0.1:16000");
        assert_eq!(join("2001:db8::1", 16000), "[2001:db8::1]:16000");
        assert_eq!(join("[2001:db8::1]", 16000), "[2001:db8::1]:16000");
        assert!(join("::1", 5060).parse::<SocketAddr>().is_ok());
    }

    #[test]
    fn detects_ipv6_and_picks_bind_family() {
        assert!(is_ipv6("::1") && is_ipv6("[2001:db8::1]"));
        assert!(!is_ipv6("10.0.0.1") && !is_ipv6("pbx.example"));
        assert_eq!(unspecified_for(&"[::1]:5060".parse().unwrap()), "[::]:0");
        assert_eq!(
            unspecified_for(&"10.0.0.1:5060".parse().unwrap()),
            "0.0.0.0:0"
        );
    }
}
