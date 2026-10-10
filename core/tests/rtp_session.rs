/// RTP session 傳輸測試：NAT / 媒體中繼下對端實際位址與 SDP 不同、對端 port 暫時不可達
use sipress_core::rtp::{RtpPacket, RtpSession, RtpSessionConfig};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

async fn start_session(remote: SocketAddr, base_port: u16) -> RtpSession {
    let cfg = RtpSessionConfig {
        base_port,
        local_ip:    "127.0.0.1".into(),
        remote_addr: remote.to_string(),
        audio_file:  None,
        ssrc:        None,
        local_port:  None,
    };
    RtpSession::start(cfg, Arc::new(Mutex::new(base_port)), None).await.unwrap()
}

/// 在 `wait` 時間內收集 sock 收到的封包數
async fn count_packets(sock: &UdpSocket, wait: Duration) -> usize {
    let mut buf = [0u8; 1500];
    let mut n = 0;
    let deadline = tokio::time::Instant::now() + wait;
    while let Ok(Ok(_)) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await {
        n += 1;
    }
    n
}

async fn send_rtp(from: &UdpSocket, to: SocketAddr, count: u16) {
    for seq in 0..count {
        let pkt = RtpPacket::new(8, seq, seq as u32 * 160, 0x1234, vec![0xD5; 160]);
        from.send_to(&pkt.encode(), to).await.unwrap();
    }
}

#[tokio::test]
async fn receives_rtp_from_address_other_than_sdp_and_latches_to_it() {
    // SDP 宣告的位址（例如 NAT 內部位址）：實際不會送封包
    let sdp_addr = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    // 實際送出 RTP 的位址（例如 NAT 外部位址 / 媒體中繼）
    let actual = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let session = start_session(sdp_addr.local_addr().unwrap(), 41000).await;
    let local: SocketAddr = format!("127.0.0.1:{}", session.local_port()).parse().unwrap();

    send_rtp(&actual, local, 10).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        session.stats.recv_packets.load(Ordering::Relaxed), 10,
        "來源位址與 SDP 不同的 RTP 也必須收得到"
    );

    // 學習後，送出的 RTP 應改送到實際來源位址
    let at_actual = count_packets(&actual, Duration::from_millis(300)).await;
    assert!(at_actual >= 5, "應改送到實際來源位址（收到 {} 個）", at_actual);
    session.stop();
}

#[tokio::test]
async fn update_remote_switches_target_and_relearns() {
    let first = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let session = start_session(first.local_addr().unwrap(), 42000).await;
    let local: SocketAddr = format!("127.0.0.1:{}", session.local_port()).parse().unwrap();
    send_rtp(&first, local, 3).await;
    tokio::time::sleep(Duration::from_millis(50)).await; // 學習到 first

    // RE-INVITE 換新目標
    let second = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    session.update_remote(&second.local_addr().unwrap().to_string()).await.unwrap();
    // 切換後才到的舊來源封包（在途中）不可把目標學習回舊位址
    send_rtp(&first, local, 3).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    count_packets(&first, Duration::from_millis(50)).await; // 清掉切換前送出的封包
    assert!(count_packets(&second, Duration::from_millis(200)).await >= 5, "應改送到 RE-INVITE 的新位址");
    assert_eq!(count_packets(&first, Duration::from_millis(100)).await, 0, "不可再送到舊位址");

    // 新目標的實際來源又不同 → 重新學習
    let relay = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    send_rtp(&relay, local, 3).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(count_packets(&relay, Duration::from_millis(200)).await >= 5, "RE-INVITE 後應重新學習實際來源");
    session.stop();
}

#[tokio::test]
async fn sending_survives_temporarily_unreachable_remote() {
    // 對端 port 尚未開啟（送出會觸發 ICMP port unreachable）
    let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let remote = probe.local_addr().unwrap();
    drop(probe);

    let session = start_session(remote, 43000).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 對端稍後才開始監聽：RTP 傳送不可因先前的錯誤而永久停止
    let late = UdpSocket::bind(remote).await.unwrap();
    assert!(
        count_packets(&late, Duration::from_millis(300)).await >= 5,
        "對端暫時不可達後，RTP 應繼續傳送"
    );
    session.stop();
}
