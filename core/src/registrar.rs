/// 座席帳號註冊器：對 SIP server 發送 REGISTER，處理 Digest 認證
use crate::config::Transport;
use crate::sip::{
    register::{DigestChallenge, RegisterMessage},
    SipMessage, SipParser, SipSocket,
};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time;

/// 註冊結果狀態
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterStatus {
    /// 註冊成功（200 OK）
    Registered,
    /// 認證失敗（密碼錯誤 / 帳號不存在）
    AuthFailed,
    /// 伺服器明確拒絕（4xx/5xx/6xx，非 401/407）
    Rejected,
    /// 等待回應逾時
    Timeout,
    /// 網路 / IO 錯誤
    NetworkError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResult {
    pub status: RegisterStatus,
    /// 給人看的訊息（含狀態碼或錯誤描述）
    pub message: String,
    /// 伺服器同意的 Expires 秒數（成功時）
    pub expires_secs: Option<u32>,
    /// 觸發的 SIP 狀態碼（若有）
    pub sip_code: Option<u16>,
}

/// 對單一帳號執行一次完整 REGISTER 握手（含 401 重送）
///
/// - `server_addr`: SIP server `ip:port`
/// - `domain`: From/To URI 中的 domain（通常等於 server IP，或自訂）
/// - `username` / `password`: 帳號認證資訊
/// - `expires`: 註冊有效期（秒）
/// - `transport`: "UDP" 或 "TCP"（不分大小寫）
pub async fn register_once(
    server_addr: &str,
    domain: &str,
    username: &str,
    password: &str,
    expires: u32,
    transport: &str,
) -> Result<RegisterResult> {
    let server: std::net::SocketAddr = server_addr
        .parse()
        .with_context(|| format!("無效的伺服器位址: {}", server_addr))?;

    let (tp, transport) = if transport.eq_ignore_ascii_case("tcp") {
        (Transport::Tcp, "TCP")
    } else {
        (Transport::Udp, "UDP")
    };

    // ── 取得本機 socket（短期，用完即關）──
    let sock = match SipSocket::connect(tp, server, crate::net::unspecified_for(&server)).await {
        Ok(s) => s,
        Err(e) => {
            return Ok(RegisterResult {
                status: RegisterStatus::NetworkError,
                message: format!("{:#}", e),
                expires_secs: None,
                sip_code: None,
            })
        }
    };
    let local_sock_str = sock.local_addr.clone();

    let call_id = SipMessage::new_call_id(domain);
    let from_tag = SipMessage::new_tag();
    let branch1 = SipMessage::new_branch();

    // ── 第一次 REGISTER（不帶 Authorization）──
    let req1 = RegisterMessage::build(
        username,
        server_addr,
        &local_sock_str,
        1,
        &branch1,
        &from_tag,
        &call_id,
        transport,
        expires,
        None,
    );
    if let Err(e) = sock.send(&req1).await {
        return Ok(RegisterResult {
            status: RegisterStatus::NetworkError,
            message: format!("傳送失敗: {}", e),
            expires_secs: None,
            sip_code: None,
        });
    }

    let resp1 = match recv_with_timeout(&sock, Duration::from_secs(5)).await {
        Ok(s) => s,
        Err(e) => {
            return Ok(RegisterResult {
                status: RegisterStatus::Timeout,
                message: format!("REGISTER 等待回應逾時: {}", e),
                expires_secs: None,
                sip_code: None,
            });
        }
    };

    let code1 = SipParser::status_code(&resp1).unwrap_or(0);

    match code1 {
        200 => Ok(RegisterResult {
            status: RegisterStatus::Registered,
            message: format!("註冊成功 ({})", username),
            expires_secs: Some(SipParser::expires(&resp1).unwrap_or(expires)),
            sip_code: Some(200),
        }),

        401 | 407 => {
            // 解析 challenge 並重送
            let chal = DigestChallenge::parse(&resp1)
                .ok_or_else(|| anyhow!("無法解析 Digest challenge ({})", code1))?;

            let uri = format!("sip:{}", server_addr);
            let auth_line = chal.authorization_header(username, password, "REGISTER", &uri);

            let branch2 = SipMessage::new_branch();
            let req2 = RegisterMessage::build(
                username,
                server_addr,
                &local_sock_str,
                2,
                &branch2,
                &from_tag,
                &call_id,
                transport,
                expires,
                Some(&auth_line),
            );
            if let Err(e) = sock.send(&req2).await {
                return Ok(RegisterResult {
                    status: RegisterStatus::NetworkError,
                    message: format!("認證重送失敗: {}", e),
                    expires_secs: None,
                    sip_code: None,
                });
            }

            let resp2 = match recv_with_timeout(&sock, Duration::from_secs(5)).await {
                Ok(s) => s,
                Err(_) => {
                    return Ok(RegisterResult {
                        status: RegisterStatus::Timeout,
                        message: "認證重送回應逾時".into(),
                        expires_secs: None,
                        sip_code: None,
                    });
                }
            };

            let code2 = SipParser::status_code(&resp2).unwrap_or(0);
            match code2 {
                200 => Ok(RegisterResult {
                    status: RegisterStatus::Registered,
                    message: format!("註冊成功 ({})", username),
                    expires_secs: Some(SipParser::expires(&resp2).unwrap_or(expires)),
                    sip_code: Some(200),
                }),
                401 | 403 | 407 => Ok(RegisterResult {
                    status: RegisterStatus::AuthFailed,
                    message: format!("認證失敗 (SIP {})", code2),
                    expires_secs: None,
                    sip_code: Some(code2),
                }),
                other => Ok(RegisterResult {
                    status: RegisterStatus::Rejected,
                    message: format!("註冊被拒 (SIP {})", other),
                    expires_secs: None,
                    sip_code: Some(other),
                }),
            }
        }

        other if other > 0 => Ok(RegisterResult {
            status: RegisterStatus::Rejected,
            message: format!("意外回應 (SIP {})", other),
            expires_secs: None,
            sip_code: Some(other),
        }),

        _ => Ok(RegisterResult {
            status: RegisterStatus::NetworkError,
            message: "無法解析回應".into(),
            expires_secs: None,
            sip_code: None,
        }),
    }
}

// ─── helpers ─────────────────────────────────────────────────────

async fn recv_with_timeout(sock: &SipSocket, dur: Duration) -> Result<String> {
    // 100/180 之類的中繼回應會被一起送來，我們忽略它們，等到最終回應為止
    let deadline = tokio::time::Instant::now() + dur;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("逾時");
        }
        let raw = match time::timeout(remaining, sock.recv()).await {
            Ok(r) => r?,
            Err(_) => anyhow::bail!("逾時"),
        };
        let code = SipParser::status_code(&raw).unwrap_or(0);
        // 100/180 / provisional → 繼續等
        if (100..200).contains(&code) {
            continue;
        }
        return Ok(raw);
    }
}
