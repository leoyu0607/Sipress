/// UDP / TCP 傳輸層封裝
use crate::config::Transport;
use anyhow::{anyhow, Context, Result};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpSocket, UdpSocket};
use tokio::sync::Mutex;
use tracing::debug;

/// 單一 SIP 訊息的上限（UDP datagram 與 TCP 分框都適用）
const MAX_MESSAGE: usize = 65536;

thread_local! {
    /// UDP 接收用的緩衝區（每個執行緒一份，所有 socket 共用）
    static RECV_BUF: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; MAX_MESSAGE]);
}

/// 連往 SIP 伺服器的 socket（UDP 或 TCP），多 task 共享
///
/// TCP 時整個生命週期只用一條連線：送出的請求、伺服器的回應與伺服器主動送來的請求
/// （例如打給座席的 INVITE）都走這條連線。
pub struct SipSocket {
    inner: Inner,
    pub local_addr: String,
    pub server: SocketAddr,
}

enum Inner {
    Udp(UdpSocket),
    Tcp {
        reader: Mutex<TcpReader>,
        writer: Mutex<OwnedWriteHalf>,
    },
}

struct TcpReader {
    half: OwnedReadHalf,
    buf: Vec<u8>,
}

impl SipSocket {
    /// 建立並連線至 SIP 伺服器
    /// `local` 為 `ip:port` 時綁定該 port；只有 IP 時由系統分配 port
    pub async fn connect(transport: Transport, server: SocketAddr, local: &str) -> Result<Self> {
        match transport {
            Transport::Udp => Self::udp(server, local).await,
            Transport::Tcp => Self::tcp(server, local).await,
        }
    }

    async fn udp(server: SocketAddr, local: &str) -> Result<Self> {
        let bind_addr = bind_addr(local);
        let socket = UdpSocket::bind(&bind_addr).await.with_context(|| {
            format!(
                "無法綁定 UDP socket: {}（port 可能已被其他程式佔用）",
                bind_addr
            )
        })?;
        // 設定預設傳送目標（讓 send() 不需要每次指定 addr）
        socket
            .connect(server)
            .await
            .with_context(|| format!("無法連接至 SIP 伺服器: {}", server))?;
        let local_addr = socket.local_addr().context("無法取得本機地址")?.to_string();
        debug!("UDP socket 已綁定: {} → {}", local_addr, server);
        Ok(Self {
            inner: Inner::Udp(socket),
            local_addr,
            server,
        })
    }

    async fn tcp(server: SocketAddr, local: &str) -> Result<Self> {
        let bind_addr: SocketAddr = bind_addr(local)
            .parse()
            .with_context(|| format!("本機位址格式錯誤: {}", local))?;
        let socket = if bind_addr.is_ipv6() {
            TcpSocket::new_v6()
        } else {
            TcpSocket::new_v4()
        }?;
        socket.bind(bind_addr).with_context(|| {
            format!(
                "無法綁定 TCP socket: {}（port 可能已被其他程式佔用）",
                bind_addr
            )
        })?;
        let stream = socket
            .connect(server)
            .await
            .with_context(|| format!("無法以 TCP 連線至 SIP 伺服器: {}", server))?;
        let _ = stream.set_nodelay(true);
        let local_addr = stream.local_addr().context("無法取得本機地址")?.to_string();
        debug!("TCP 已連線: {} → {}", local_addr, server);
        let (half, writer) = stream.into_split();
        Ok(Self {
            inner: Inner::Tcp {
                reader: Mutex::new(TcpReader {
                    half,
                    buf: Vec::new(),
                }),
                writer: Mutex::new(writer),
            },
            local_addr,
            server,
        })
    }

    /// 可靠傳輸（TCP）不需要 SIP 層的重傳
    pub fn is_reliable(&self) -> bool {
        matches!(self.inner, Inner::Tcp { .. })
    }

    /// 傳送 SIP 訊息（raw text）
    pub async fn send(&self, msg: &str) -> Result<()> {
        let bytes = msg.as_bytes();
        match &self.inner {
            Inner::Udp(s) => {
                s.send(bytes).await.context("UDP 傳送失敗")?;
            }
            Inner::Tcp { writer, .. } => {
                writer
                    .lock()
                    .await
                    .write_all(bytes)
                    .await
                    .context("TCP 傳送失敗")?;
            }
        }
        debug!("> {} bytes", bytes.len());
        Ok(())
    }

    /// 接收下一則 SIP 訊息
    ///
    /// 可在 `select!` / `timeout` 中使用：取消時 TCP 已讀入的位元組會保留到下一次呼叫。
    pub async fn recv(&self) -> Result<String> {
        match &self.inner {
            // 等到有資料才取緩衝區讀出：等待中不佔記憶體（座席端每個帳號各一個 socket，
            // 上千個帳號時每個都握著 64 KB 緩衝區會佔掉數百 MB）
            Inner::Udp(s) => loop {
                s.readable().await.context("UDP 接收失敗")?;
                let n = RECV_BUF.with_borrow_mut(|buf| match s.try_recv(buf) {
                    Ok(n) => Ok(Some(String::from_utf8_lossy(&buf[..n]).into_owned())),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                    Err(e) => Err(e),
                });
                if let Some(msg) = n.context("UDP 接收失敗")? {
                    return Ok(msg);
                }
            },
            Inner::Tcp { reader, .. } => {
                let mut r = reader.lock().await;
                loop {
                    if let Some(msg) = take_frame(&mut r.buf)? {
                        return Ok(msg);
                    }
                    let TcpReader { half, buf } = &mut *r;
                    if half.read_buf(buf).await.context("TCP 接收失敗")? == 0 {
                        return Err(anyhow!("SIP 伺服器已關閉 TCP 連線"));
                    }
                }
            }
        }
    }
}

/// 從 TCP 位元組流切出一則完整的 SIP 訊息（依 `Content-Length` 分框，RFC 3261 §18.3）
///
/// 資料還不夠時回傳 None；訊息之間的 CRLF（keep-alive）會被略過。
fn take_frame(buf: &mut Vec<u8>) -> Result<Option<String>> {
    let skip = buf
        .iter()
        .take_while(|&&b| b == b'\r' || b == b'\n')
        .count();
    buf.drain(..skip);
    let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        if buf.len() > MAX_MESSAGE {
            return Err(anyhow!("SIP 訊息標頭超過 {} bytes", MAX_MESSAGE));
        }
        return Ok(None);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let body_len = head
        .lines()
        .skip(1)
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            let name = name.trim();
            (name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("l"))
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    let total = head_end + 4 + body_len;
    if total > MAX_MESSAGE {
        return Err(anyhow!("SIP 訊息超過 {} bytes", MAX_MESSAGE));
    }
    if buf.len() < total {
        return Ok(None);
    }
    let msg = String::from_utf8_lossy(&buf[..total]).into_owned();
    buf.drain(..total);
    Ok(Some(msg))
}

/// 決定綁定位址：`ip:port` 原樣使用；只有 IP 時 port 由系統分配
fn bind_addr(local: &str) -> String {
    if local.parse::<SocketAddr>().is_ok() {
        local.to_string()
    } else {
        crate::net::join(&crate::net::host(local), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> SocketAddr {
        "127.0.0.1:5060".parse().unwrap()
    }

    /// 取得一個目前沒人用的 UDP port
    fn free_port() -> u16 {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn binds_requested_port() {
        let port = free_port();
        let s = SipSocket::connect(Transport::Udp, server(), &format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        assert_eq!(
            s.local_addr,
            format!("127.0.0.1:{}", port),
            "指定的 port 必須真的被綁定"
        );
    }

    #[test]
    fn tcp_framing_follows_content_length() {
        let a = "INVITE sip:a@b SIP/2.0\r\nCall-ID: 1\r\nContent-Length: 5\r\n\r\nhello";
        let b = "SIP/2.0 200 OK\r\nl: 0\r\n\r\n";
        // keep-alive CRLF + 兩則訊息黏在一起
        let mut buf = format!("\r\n\r\n{}{}", a, b).into_bytes();
        assert_eq!(take_frame(&mut buf).unwrap().as_deref(), Some(a));
        assert_eq!(take_frame(&mut buf).unwrap().as_deref(), Some(b));
        assert!(take_frame(&mut buf).unwrap().is_none());
        assert!(buf.is_empty());

        // 分段到達：標頭或 body 不完整時等待
        let mut buf = a.as_bytes()[..20].to_vec();
        assert!(take_frame(&mut buf).unwrap().is_none());
        buf.extend_from_slice(&a.as_bytes()[20..a.len() - 2]);
        assert!(take_frame(&mut buf).unwrap().is_none(), "body 還差 2 bytes");
        buf.extend_from_slice(&a.as_bytes()[a.len() - 2..]);
        assert_eq!(take_frame(&mut buf).unwrap().as_deref(), Some(a));

        // 沒有 Content-Length 視為沒有 body
        let mut buf = b"BYE sip:a@b SIP/2.0\r\nCall-ID: 2\r\n\r\n".to_vec();
        assert!(take_frame(&mut buf).unwrap().is_some());
    }

    #[test]
    fn tcp_framing_rejects_oversized_messages() {
        let mut buf =
            format!("SIP/2.0 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_MESSAGE).into_bytes();
        assert!(take_frame(&mut buf).is_err());
        let mut buf = vec![b'a'; MAX_MESSAGE + 1];
        assert!(take_frame(&mut buf).is_err(), "沒有標頭結尾又超過上限");
    }

    #[tokio::test]
    async fn tcp_send_and_recv_over_one_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = s.read(&mut buf).await.unwrap();
            let got = String::from_utf8_lossy(&buf[..n]).into_owned();
            // 一則訊息分兩次寫出
            s.write_all(b"SIP/2.0 200 OK\r\nContent-Le").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            s.write_all(b"ngth: 2\r\n\r\nok").await.unwrap();
            got
        });
        let sock = SipSocket::connect(Transport::Tcp, server, "127.0.0.1")
            .await
            .unwrap();
        assert!(sock.is_reliable());
        sock.send("OPTIONS sip:a@b SIP/2.0\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        let resp = sock.recv().await.unwrap();
        assert_eq!(resp, "SIP/2.0 200 OK\r\nContent-Length: 2\r\n\r\nok");
        assert!(peer.await.unwrap().starts_with("OPTIONS"));
        let err = sock.recv().await.unwrap_err();
        assert!(format!("{:#}", err).contains("關閉"), "{:#}", err);
    }

    #[test]
    fn bind_addr_keeps_ipv6_intact() {
        assert_eq!(bind_addr("127.0.0.1"), "127.0.0.1:0");
        assert_eq!(bind_addr("127.0.0.1:5070"), "127.0.0.1:5070");
        assert_eq!(bind_addr("::1"), "[::1]:0", "IPv6 只給 IP 時不可被截斷");
        assert_eq!(bind_addr("2001:db8::2"), "[2001:db8::2]:0");
        assert_eq!(bind_addr("[2001:db8::2]:5070"), "[2001:db8::2]:5070");
    }

    #[tokio::test]
    async fn ip_only_gets_os_assigned_port() {
        let s = SipSocket::connect(Transport::Udp, server(), "127.0.0.1")
            .await
            .unwrap();
        let addr: SocketAddr = s.local_addr.parse().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_ne!(addr.port(), 0);
    }

    #[tokio::test]
    async fn port_in_use_reports_clear_error() {
        let holder = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = holder.local_addr().unwrap().port();
        let err = SipSocket::connect(Transport::Udp, server(), &format!("127.0.0.1:{}", port))
            .await
            .err()
            .expect("port 已被佔用時應失敗");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains(&port.to_string()) && msg.contains("佔用"),
            "錯誤訊息應指出 port 被佔用：{}",
            msg
        );
    }
}
