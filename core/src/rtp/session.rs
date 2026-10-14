/// 單通通話的 RTP Session
use crate::rtp::{
    audio::AudioSource,
    packet::RtpPacket,
    stats::{RtpStats, RtpStatsSnapshot},
};
use anyhow::{Context, Result};
use rand::Rng;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::UdpSocket, sync::{Mutex, Notify}, time};
use tracing::debug;

/// RTP session 設定
#[derive(Debug, Clone)]
pub struct RtpSessionConfig {
    /// RTP 起始 port（系統從此 port 往上找可用的 even port）
    pub base_port:   u16,
    /// 本機 IP
    pub local_ip:    String,
    /// 遠端 RTP 地址（從 SDP 解析，格式 "ip:port"）
    pub remote_addr: String,
    /// 音檔路徑（None = 靜音）
    pub audio_file:  Option<std::path::PathBuf>,
    /// SSRC（None = 隨機）
    pub ssrc:        Option<u32>,
    /// 預先分配的本機 RTP port（Some = 跳過動態分配）
    pub local_port:  Option<u16>,
    /// Codec payload type（8=PCMA, 0=PCMU，預設 8）
    pub codec_pt:    u8,
}

/// RTP 送出目標
///
/// socket 不做 `connect()`：connected UDP socket 會被 OS 過濾掉非該位址送來的封包，
/// 在 NAT / 媒體中繼環境（實際來源 ≠ SDP 位址）會誤判為 100% 掉包；
/// 且收到 ICMP port unreachable 後 send/recv 會回錯誤。
/// 改為 symmetric RTP：收到第一個有效 RTP 封包後，送出目標改為其實際來源位址。
struct RemoteTarget {
    addr:     SocketAddr,
    /// 已依實際收到的封包確認（學習）過送出目標
    latched:  bool,
    /// RE-INVITE 前的舊目標：切換後仍在途中的舊封包不可用來學習
    previous: Option<SocketAddr>,
}

/// 單通通話的 RTP session handle
///
/// handle 被丟棄時會一併停止收發 task，避免漏掉 `stop()` 時 RTP 持續傳送。
pub struct RtpSession {
    pub stats:      Arc<RtpStats>,
    stop_flag:      Arc<AtomicBool>,
    /// 喚醒阻塞在 recv 的接收 task（notify_one 會保留 permit，不會錯過）
    stop_notify:    Arc<Notify>,
    remote:         Arc<std::sync::Mutex<RemoteTarget>>,
    /// 傳送 DTMF（RFC 2833）用的同一個 RTP socket
    socket:         Arc<UdpSocket>,
    local_rtp_port: u16,
}

impl RtpSession {
    /// 啟動 RTP session
    ///
    /// `pre_bound`: 已綁定的本機 socket（從 `allocate_port` 取得）。
    /// 若傳入 Some，直接使用該 socket（避免 TOCTOU 競態）；
    /// 若傳入 None，重新動態分配 port。
    pub async fn start(
        config: RtpSessionConfig,
        port_counter: Arc<Mutex<u16>>,
        pre_bound: Option<UdpSocket>,
    ) -> Result<Self> {
        let remote_addr = resolve(&config.remote_addr).await?;
        let (socket, local_port) = if let Some(sock) = pre_bound {
            let port = sock.local_addr()?.port();
            (Arc::new(sock), port)
        } else {
            // 沒有預分配 socket，動態尋找可用 port
            let (port, sock) = Self::allocate_port(&port_counter, &config.local_ip).await?;
            (Arc::new(sock), port)
        };
        let remote = Arc::new(std::sync::Mutex::new(RemoteTarget { addr: remote_addr, latched: false, previous: None }));

        let stats     = Arc::new(RtpStats::new());
        let stop_flag   = Arc::new(AtomicBool::new(false));
        let stop_notify = Arc::new(Notify::new());
        let ssrc      = config.ssrc.unwrap_or_else(|| rand::thread_rng().gen());

        // ── Task A：傳送（每 20ms 一個 frame）──
        {
            let socket     = Arc::clone(&socket);
            let stats      = Arc::clone(&stats);
            let stop       = Arc::clone(&stop_flag);
            let remote     = Arc::clone(&remote);
            let audio_path = config.audio_file.clone();

            let codec_pt = config.codec_pt;
            tokio::spawn(async move {
                let mut source = match &audio_path {
                    Some(p) => {
                        match AudioSource::from_file_cached(p).await {
                            Ok(src) => {
                                tracing::info!(
                                    "音檔載入成功: {:?}  frames={} duration={:.1}s PT={}",
                                    p.file_name().unwrap_or_default(),
                                    src.frame_count(),
                                    src.duration_secs(),
                                    src.payload_type,
                                );
                                src
                            }
                            Err(e) => {
                                tracing::warn!("音檔載入失敗（{:?}），改用靜音: {}", p, e);
                                AudioSource::silence()
                            }
                        }
                    }
                    None => AudioSource::silence(),
                };

                // 音檔自帶 PT 優先，靜音模式用設定的 codec
                let payload_type = if source.payload_type == 8 && codec_pt != 8 {
                    source.payload_type = codec_pt;
                    codec_pt
                } else {
                    source.payload_type
                };
                tracing::debug!("RTP 傳送 payload_type={} ({})",
                    payload_type,
                    if payload_type == 8 { "PCMA/A-law" } else { "PCMU/μ-law" });

                let mut seq: u16 = rand::thread_rng().gen();
                let ts0:     u32 = rand::thread_rng().gen();
                let ptime = Duration::from_millis(20);
                let mut interval = time::interval(ptime);
                // CPU 卡頓錯過的 tick 直接略過，不事後補送（Burst 會讓接收端看到人為 jitter）
                interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
                let start = time::Instant::now();
                let mut last_tick: Option<u32> = None;

                loop {
                    let tick_at = interval.tick().await;
                    if stop.load(Ordering::Relaxed) { break; }

                    // timestamp 依實際經過的 20ms 數計算：卡頓呈現為 timestamp 跳躍，
                    // 音檔進度也同步跳過（與真實話機卡頓時的行為一致）
                    let tick = (tick_at.saturating_duration_since(start).as_millis() / 20) as u32;
                    if let Some(prev) = last_tick {
                        source.skip(tick.saturating_sub(prev + 1) as usize);
                    }
                    last_tick = Some(tick);
                    let ts = ts0.wrapping_add(tick.wrapping_mul(160));

                    let frame = match source.next_frame() {
                        Some(f) => f,
                        None    => break,
                    };

                    let pkt   = RtpPacket::new(payload_type, seq, ts, ssrc, frame.clone());
                    let bytes = pkt.encode();

                    let target = remote.lock().unwrap().addr;
                    match socket.send_to(&bytes, target).await {
                        Ok(_)  => stats.on_send(frame.len()),
                        // 暫時性錯誤（例如對端 port 尚未開啟）只略過這個 frame，不中止傳送
                        Err(e) => debug!("RTP 傳送失敗: {}", e),
                    }

                    seq = seq.wrapping_add(1);
                }
            });
        }

        // ── Task B：接收（計算 jitter/loss）──
        {
            let socket = Arc::clone(&socket);
            let stats  = Arc::clone(&stats);
            let notify = Arc::clone(&stop_notify);
            let remote = Arc::clone(&remote);

            tokio::spawn(async move {
                let mut buf = vec![0u8; 1500];
                loop {
                    // 阻塞等待封包或停止通知（不再每 50ms 逾時輪詢停止旗標）
                    let res = tokio::select! {
                        r = socket.recv_from(&mut buf) => r,
                        _ = notify.notified() => break,
                    };
                    match res {
                        Ok((n, src)) => {
                            if let Some(pkt) = RtpPacket::decode(&buf[..n]) {
                                latch_remote(&remote, src);
                                let now_us = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_micros() as u64;
                                stats.on_recv(pkt.ssrc, pkt.sequence, pkt.timestamp, now_us);
                                debug!("RTP recv seq={} ts={}", pkt.sequence, pkt.timestamp);
                            }
                        }
                        Err(e) => {
                            // 暫時性錯誤不中止接收；稍候再試避免空轉
                            debug!("RTP 接收錯誤: {}", e);
                            time::sleep(Duration::from_millis(20)).await;
                        }
                    }
                }
            });
        }

        Ok(Self { stats, stop_flag, stop_notify, remote, socket, local_rtp_port: local_port })
    }

    /// 停止 RTP session，回傳統計快照
    pub fn stop(&self) -> RtpStatsSnapshot {
        self.signal_stop();
        self.stats.snapshot()
    }

    /// 傳送 DTMF 序列（在背景 task 中執行，不阻塞主迴圈）
    pub fn send_dtmf_sequence(&self, sequence: String, ssrc: u32) {
        let socket = Arc::clone(&self.socket);
        let stop = Arc::clone(&self.stop_flag);
        let remote = Arc::clone(&self.remote);
        tokio::spawn(async move {
            let actions = crate::rtp::dtmf::parse_dtmf_sequence(&sequence);
            let mut seq: u16 = rand::thread_rng().gen();
            let mut ts: u32 = rand::thread_rng().gen();

            for action in &actions {
                if stop.load(Ordering::Relaxed) { break; }
                match action {
                    crate::rtp::dtmf::DtmfAction::Digit(ch, dur_ms) => {
                        if let Some((pkts, advance)) =
                            crate::rtp::dtmf::build_dtmf_packets(*ch, *dur_ms, seq, ts, ssrc)
                        {
                            for pkt in &pkts {
                                if stop.load(Ordering::Relaxed) { break; }
                                let target = remote.lock().unwrap().addr;
                                let _ = socket.send_to(&pkt.encode(), target).await;
                                time::sleep(Duration::from_millis(20)).await;
                            }
                            seq = seq.wrapping_add(pkts.len() as u16);
                            ts = ts.wrapping_add(advance);
                        }
                    }
                    crate::rtp::dtmf::DtmfAction::Wait(ms) => {
                        time::sleep(Duration::from_millis(*ms as u64)).await;
                    }
                }
            }
        });
    }

    /// 通知傳送 task（檢查旗標）與接收 task（Notify）結束
    fn signal_stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        self.stop_notify.notify_one();
    }

    /// RE-INVITE 後更新 RTP 對端地址，並重新學習實際來源位址
    pub async fn update_remote(&self, new_addr: &str) -> anyhow::Result<()> {
        let addr = resolve(new_addr).await
            .with_context(|| format!("RE-INVITE 後無法切換 RTP 對端: {}", new_addr))?;
        let mut target = self.remote.lock().unwrap();
        let previous = Some(target.addr);
        *target = RemoteTarget { addr, latched: false, previous };
        Ok(())
    }

    /// 本機 RTP port
    pub fn local_port(&self) -> u16 {
        self.local_rtp_port
    }

    // ── Port 分配 ────────────────────────────────────────────────

    /// 從計數器指向的 port 開始，找到可用的偶數 port 並推進計數器。
    /// 回傳 `(port, 已綁定的 UdpSocket)`：socket 保持綁定狀態，
    /// 消除「測試 → 釋放 → 再綁定」的 TOCTOU 競態。
    pub async fn allocate_port(
        counter:  &Arc<Mutex<u16>>,
        local_ip: &str,
    ) -> Result<(u16, UdpSocket)> {
        let mut guard = counter.lock().await;
        let start = *guard;
        for offset in (0u16..4000).step_by(2) {
            let port = start.wrapping_add(offset);
            if port < 1024 { continue; }
            let addr = format!("{}:{}", local_ip, port);
            if let Ok(socket) = UdpSocket::bind(&addr).await {
                *guard = port.wrapping_add(2);
                return Ok((port, socket));
            }
        }
        anyhow::bail!("無法找到可用的 RTP port（從 {} 開始）", start);
    }
}

/// 解析 "ip:port"（或 "host:port"）為 SocketAddr
async fn resolve(addr: &str) -> Result<SocketAddr> {
    tokio::net::lookup_host(addr).await
        .with_context(|| format!("無效的 RTP 對端位址: {}", addr))?
        .next()
        .with_context(|| format!("無法解析 RTP 對端位址: {}", addr))
}

/// Symmetric RTP：第一個有效 RTP 封包的來源即為之後的送出目標
fn latch_remote(remote: &std::sync::Mutex<RemoteTarget>, src: SocketAddr) {
    let mut target = remote.lock().unwrap();
    if target.latched || target.previous == Some(src) {
        return;
    }
    if target.addr != src {
        tracing::info!("RTP 對端位址學習：{} → {}（實際來源與 SDP 不同）", target.addr, src);
        target.addr = src;
    }
    target.latched = true;
}

impl Drop for RtpSession {
    fn drop(&mut self) {
        self.signal_stop();
    }
}
