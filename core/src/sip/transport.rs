/// UDP / TCP 傳輸層封裝
use anyhow::{Context, Result};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tracing::debug;

/// 共用 UDP socket（Arc 包裹，多 task 共享）
pub struct SharedUdpSocket {
    pub socket:     UdpSocket,
    pub local_addr: String,
    pub server:     SocketAddr,
}

impl SharedUdpSocket {
    /// 建立並綁定 UDP socket，設定預設遠端地址
    /// `local` 為 `ip:port` 時綁定該 port；只有 IP 時由系統分配 port
    pub async fn new(server: SocketAddr, local: &str) -> Result<Self> {
        let bind_addr = bind_addr(local);

        let socket = UdpSocket::bind(&bind_addr)
            .await
            .with_context(|| format!("無法綁定 UDP socket: {}（port 可能已被其他程式佔用）", bind_addr))?;

        // 設定預設傳送目標（讓 send() 不需要每次指定 addr）
        socket.connect(server).await
            .with_context(|| format!("無法連接至 SIP 伺服器: {}", server))?;

        let local_addr = socket.local_addr()
            .context("無法取得本機地址")?
            .to_string();

        debug!("UDP socket 已綁定: {} → {}", local_addr, server);

        Ok(Self { socket, local_addr, server })
    }

    /// 傳送 SIP 訊息（raw text）
    pub async fn send(&self, msg: &str) -> Result<()> {
        let bytes = msg.as_bytes();
        self.socket.send(bytes).await
            .with_context(|| "UDP 傳送失敗")?;
        debug!("> {} bytes", bytes.len());
        Ok(())
    }
}

/// 決定綁定位址：`ip:port` 原樣使用；只有 IP 時 port 由系統分配
fn bind_addr(local: &str) -> String {
    if local.parse::<SocketAddr>().is_ok() {
        local.to_string()
    } else {
        format!("{}:0", local.split(':').next().unwrap_or("0.0.0.0"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> SocketAddr { "127.0.0.1:5060".parse().unwrap() }

    /// 取得一個目前沒人用的 UDP port
    fn free_port() -> u16 {
        std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn binds_requested_port() {
        let port = free_port();
        let s = SharedUdpSocket::new(server(), &format!("127.0.0.1:{}", port)).await.unwrap();
        assert_eq!(s.local_addr, format!("127.0.0.1:{}", port), "指定的 port 必須真的被綁定");
    }

    #[tokio::test]
    async fn ip_only_gets_os_assigned_port() {
        let s = SharedUdpSocket::new(server(), "127.0.0.1").await.unwrap();
        let addr: SocketAddr = s.local_addr.parse().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_ne!(addr.port(), 0);
    }

    #[tokio::test]
    async fn port_in_use_reports_clear_error() {
        let holder = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = holder.local_addr().unwrap().port();
        let err = SharedUdpSocket::new(server(), &format!("127.0.0.1:{}", port)).await
            .err().expect("port 已被佔用時應失敗");
        let msg = format!("{:#}", err);
        assert!(msg.contains(&port.to_string()) && msg.contains("佔用"), "錯誤訊息應指出 port 被佔用：{}", msg);
    }
}
