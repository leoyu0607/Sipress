/// 單通通話的 RTP Session
use crate::rtp::{
    audio::AudioSource,
    packet::RtpPacket,
    stats::{RtpStats, RtpStatsSnapshot},
};
use anyhow::{Context, Result};
use rand::Rng;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::UdpSocket, sync::Mutex, time};
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
}

/// 單通通話的 RTP session handle
///
/// handle 被丟棄時會一併停止收發 task，避免漏掉 `stop()` 時 RTP 持續傳送。
pub struct RtpSession {
    pub stats:      Arc<RtpStats>,
    stop_flag:      Arc<AtomicBool>,
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
        let (socket, local_port) = if let Some(sock) = pre_bound {
            let port = sock.local_addr()?.port();
            sock.connect(&config.remote_addr)
                .await
                .with_context(|| format!("無法連接 RTP 對端: {}", config.remote_addr))?;
            (Arc::new(sock), port)
        } else {
            // 沒有預分配 socket，動態尋找可用 port
            let (port, sock) = Self::allocate_port(&port_counter, &config.local_ip).await?;
            sock.connect(&config.remote_addr)
                .await
                .with_context(|| format!("無法連接 RTP 對端: {}", config.remote_addr))?;
            (Arc::new(sock), port)
        };

        let stats     = Arc::new(RtpStats::new());
        let stop_flag = Arc::new(AtomicBool::new(false));
        let ssrc      = config.ssrc.unwrap_or_else(|| rand::thread_rng().gen());

        // ── Task A：傳送（每 20ms 一個 frame）──
        {
            let socket     = Arc::clone(&socket);
            let stats      = Arc::clone(&stats);
            let stop       = Arc::clone(&stop_flag);
            let audio_path = config.audio_file.clone();

            tokio::spawn(async move {
                let mut source = match &audio_path {
                    Some(p) => {
                        match AudioSource::from_file(p) {
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

                // 使用音檔偵測到的 payload type（0=PCMU / 8=PCMA）
                let payload_type = source.payload_type;
                tracing::debug!("RTP 傳送 payload_type={} ({})",
                    payload_type,
                    if payload_type == 8 { "PCMA/A-law" } else { "PCMU/μ-law" });

                let mut seq: u16 = rand::thread_rng().gen();
                let mut ts:  u32 = rand::thread_rng().gen();
                let ptime = Duration::from_millis(20);
                let mut interval = time::interval(ptime);

                loop {
                    interval.tick().await;
                    if stop.load(Ordering::Relaxed) { break; }

                    let frame = match source.next_frame() {
                        Some(f) => f,
                        None    => break,
                    };

                    let pkt   = RtpPacket::new(payload_type, seq, ts, ssrc, frame.clone());
                    let bytes = pkt.encode();

                    match socket.send(&bytes).await {
                        Ok(_)  => stats.on_send(frame.len()),
                        Err(e) => { debug!("RTP 傳送失敗: {}", e); break; }
                    }

                    seq = seq.wrapping_add(1);
                    ts  = ts.wrapping_add(160);
                }
            });
        }

        // ── Task B：接收（計算 jitter/loss）──
        {
            let socket = Arc::clone(&socket);
            let stats  = Arc::clone(&stats);
            let stop   = Arc::clone(&stop_flag);

            tokio::spawn(async move {
                let mut buf = vec![0u8; 1500];
                loop {
                    if stop.load(Ordering::Relaxed) { break; }
                    match time::timeout(Duration::from_millis(50), socket.recv(&mut buf)).await {
                        Ok(Ok(n)) => {
                            if let Some(pkt) = RtpPacket::decode(&buf[..n]) {
                                let now_us = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_micros() as u64;
                                stats.on_recv(pkt.ssrc, pkt.sequence, pkt.timestamp, now_us);
                                debug!("RTP recv seq={} ts={}", pkt.sequence, pkt.timestamp);
                            }
                        }
                        Ok(Err(e)) => { debug!("RTP 接收錯誤: {}", e); break; }
                        Err(_) => {} // timeout，繼續
                    }
                }
            });
        }

        Ok(Self { stats, stop_flag, socket, local_rtp_port: local_port })
    }

    /// 停止 RTP session，回傳統計快照
    pub fn stop(&self) -> RtpStatsSnapshot {
        self.stop_flag.store(true, Ordering::Relaxed);
        self.stats.snapshot()
    }

    /// RE-INVITE 後更新 RTP 對端地址（connected UDP socket 重新 connect 即可切換目標）
    pub async fn update_remote(&self, new_addr: &str) -> anyhow::Result<()> {
        self.socket.connect(new_addr).await
            .with_context(|| format!("RE-INVITE 後無法切換 RTP 對端: {}", new_addr))?;
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

impl Drop for RtpSession {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}
