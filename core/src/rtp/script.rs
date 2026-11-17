//! 民眾端通話流程的媒體播放：每 20 ms 決定送出音檔、靜音或按鍵（RFC 4733）封包，
//! 並偵測對方是否在說話（「等對方講完」步驟用）。
//!
//! 與網路無關：`RtpSession` 的傳送 task 每個 tick 呼叫 [`ScriptPlayer::next`]，
//! 接收 task 把收到的語音交給 [`VoiceActivity::on_frame`]。
use crate::config::{CallerFlow, CallerStep};
use crate::rtp::audio::{alaw_to_linear, ulaw_to_linear, AudioSource};
use crate::rtp::dtmf::build_dtmf_packets;
use crate::rtp::packet::{PT_PCMA, PT_PCMU};
use anyhow::{Context, Result};
use rand::seq::SliceRandom;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// 每個 tick 20 ms
const TICK_MS: u64 = 20;
/// 每個按鍵送 160 ms（8 個事件封包），之後再送 3 個結束封包
const DIGIT_MS: u32 = 160;
/// 兩個按鍵之間的靜音（tick 數，100 ms）
const DIGIT_GAP_TICKS: usize = 5;
/// 判定為說話的音量：RMS ≥ -40 dBFS（電話線路的背景雜訊通常低於 -50 dBFS）
const VOICE_RMS: f64 = 32768.0 * 0.01;

/// A-law 靜音 frame（線性 0）
pub fn silence_frame() -> Vec<u8> {
    vec![0xD5; 160]
}

/// 已解析的媒體步驟：TTS 已合成為音檔，每一步都只剩送什麼聲音
#[derive(Debug, Clone, PartialEq)]
pub enum MediaStep {
    Silence {
        secs: f64,
    },
    Dtmf {
        digits: String,
    },
    /// 播放一次；多個檔案時每通隨機挑一個
    Play {
        files: Vec<PathBuf>,
    },
    WaitSilence {
        silence_ms: u32,
        timeout_secs: f64,
    },
}

/// 測試開始前把通話流程轉成媒體步驟：「說話」用 TTS 合成（或取用快取），
/// 並預先載入所有音檔，有問題時在開始前就失敗，而不是每通電話改送靜音
pub async fn prepare_flow(flow: &CallerFlow) -> Result<Vec<MediaStep>> {
    let mut out = Vec::with_capacity(flow.steps.len());
    for step in &flow.steps {
        out.push(match step {
            CallerStep::Wait { secs } => MediaStep::Silence { secs: *secs },
            CallerStep::Dtmf { digits } => MediaStep::Dtmf {
                digits: digits.trim().to_string(),
            },
            CallerStep::Play { files } => MediaStep::Play {
                files: files.clone(),
            },
            CallerStep::Say { texts } => {
                let mut files = Vec::with_capacity(texts.len());
                for text in texts {
                    files.push(
                        crate::tts::synthesize(&flow.tts, text)
                            .await
                            .with_context(|| format!("TTS 合成失敗：「{}」", text.trim()))?,
                    );
                }
                MediaStep::Play { files }
            }
            CallerStep::WaitSilence {
                silence_ms,
                timeout_secs,
            } => MediaStep::WaitSilence {
                silence_ms: *silence_ms,
                timeout_secs: *timeout_secs,
            },
        });
    }
    for step in &out {
        if let MediaStep::Play { files } = step {
            for f in files {
                AudioSource::preload(f).await?;
            }
        }
    }
    Ok(out)
}

/// 對方最近一次說話的時間（毫秒，從 session 開始算；0 = 還沒說過話）
#[derive(Default)]
pub struct VoiceActivity {
    last_voice_ms_plus1: AtomicU64,
}

impl VoiceActivity {
    /// 收到一個語音封包；`now_ms` 與傳送端使用同一個起點
    pub fn on_frame(&self, payload_type: u8, payload: &[u8], now_ms: u64) {
        if is_voice(payload_type, payload) {
            self.last_voice_ms_plus1
                .fetch_max(now_ms + 1, Ordering::Relaxed);
        }
    }

    pub fn last_voice_ms(&self) -> Option<u64> {
        self.last_voice_ms_plus1
            .load(Ordering::Relaxed)
            .checked_sub(1)
    }
}

/// G.711 frame 的音量是否達到說話的程度
fn is_voice(payload_type: u8, payload: &[u8]) -> bool {
    let decode: fn(u8) -> i16 = match payload_type {
        PT_PCMA => alaw_to_linear,
        PT_PCMU => ulaw_to_linear,
        _ => return false,
    };
    if payload.is_empty() {
        return false;
    }
    let sum: f64 = payload
        .iter()
        .map(|&b| {
            let x = decode(b) as f64;
            x * x
        })
        .sum();
    (sum / payload.len() as f64).sqrt() >= VOICE_RMS
}

/// 一個 tick 要送出的內容
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// 一個 20 ms 的 A-law 語音 frame（送出前依協商結果轉碼）
    Audio(Vec<u8>),
    /// 一個 telephone-event 封包；同一個按鍵的封包使用按鍵開始那個 tick 的時間戳記
    Event { payload: Vec<u8>, start_tick: u64 },
}

enum Pending {
    Silence(f64),
    Dtmf(String),
    Play(AudioSource),
    WaitSilence { silence_ms: u32, timeout_secs: f64 },
}

enum Active {
    Silence {
        until_tick: u64,
    },
    Queue(VecDeque<Out>),
    Play(AudioSource),
    WaitSilence {
        start_ms: u64,
        silence_ms: u64,
        deadline_ms: u64,
    },
}

/// 一通電話的流程播放狀態
pub struct ScriptPlayer {
    pending: VecDeque<Pending>,
    active: Option<Active>,
    done: bool,
}

impl ScriptPlayer {
    /// 載入流程用到的音檔（已快取，每通只複製 `Arc`），並替每個播放步驟隨機挑一個檔案
    pub async fn load(steps: &[MediaStep]) -> Result<Self> {
        let mut pending = VecDeque::with_capacity(steps.len());
        for step in steps {
            pending.push_back(match step {
                MediaStep::Silence { secs } => Pending::Silence(*secs),
                MediaStep::Dtmf { digits } => Pending::Dtmf(digits.clone()),
                MediaStep::Play { files } => {
                    let path = files
                        .choose(&mut rand::thread_rng())
                        .ok_or_else(|| anyhow::anyhow!("播放步驟沒有音檔"))?;
                    let mut src = AudioSource::from_file_cached(path).await?;
                    src.looping = false;
                    Pending::Play(src)
                }
                MediaStep::WaitSilence {
                    silence_ms,
                    timeout_secs,
                } => Pending::WaitSilence {
                    silence_ms: *silence_ms,
                    timeout_secs: *timeout_secs,
                },
            });
        }
        Ok(Self {
            pending,
            active: None,
            done: false,
        })
    }

    /// 流程是否已全部跑完（之後只送靜音）
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// 傳送卡頓略過了 `n` 個 tick：音檔與按鍵進度跟著跳過（時間型步驟依 tick 計算，不需處理）
    pub fn skip(&mut self, n: usize) {
        match self.active.as_mut() {
            Some(Active::Play(src)) => src.skip(n),
            Some(Active::Queue(q)) => {
                q.drain(..n.min(q.len()));
            }
            _ => {}
        }
    }

    /// 第 `tick` 個 20 ms 要送出的內容
    pub fn next(&mut self, tick: u64, voice: &VoiceActivity) -> Out {
        let now_ms = tick * TICK_MS;
        loop {
            let Some(active) = self.active.as_mut() else {
                match self.pending.pop_front() {
                    Some(p) => {
                        self.active = Some(start(p, tick));
                        continue;
                    }
                    None => {
                        self.done = true;
                        return Out::Audio(silence_frame());
                    }
                }
            };
            match active {
                Active::Silence { until_tick } => {
                    if tick < *until_tick {
                        return Out::Audio(silence_frame());
                    }
                }
                Active::Queue(q) => {
                    if let Some(out) = q.pop_front() {
                        return out;
                    }
                }
                Active::Play(src) => {
                    if let Some(frame) = src.next_frame() {
                        return Out::Audio(frame);
                    }
                }
                Active::WaitSilence {
                    start_ms,
                    silence_ms,
                    deadline_ms,
                } => {
                    let finished = voice
                        .last_voice_ms()
                        .filter(|&last| last >= *start_ms)
                        .is_some_and(|last| now_ms.saturating_sub(last) >= *silence_ms);
                    if !finished && now_ms < *deadline_ms {
                        return Out::Audio(silence_frame());
                    }
                }
            }
            self.active = None;
        }
    }
}

fn start(p: Pending, tick: u64) -> Active {
    let now_ms = tick * TICK_MS;
    match p {
        Pending::Silence(secs) => Active::Silence {
            until_tick: tick + (secs.max(0.0) * 1000.0 / TICK_MS as f64).round() as u64,
        },
        Pending::Dtmf(digits) => {
            let mut q = VecDeque::new();
            for ch in digits.chars() {
                let Some((pkts, _)) = build_dtmf_packets(ch, DIGIT_MS, 0, 0, 0) else {
                    continue;
                };
                let start_tick = tick + q.len() as u64;
                q.extend(pkts.into_iter().map(|p| Out::Event {
                    payload: p.payload,
                    start_tick,
                }));
                q.extend((0..DIGIT_GAP_TICKS).map(|_| Out::Audio(silence_frame())));
            }
            Active::Queue(q)
        }
        Pending::Play(src) => Active::Play(src),
        Pending::WaitSilence {
            silence_ms,
            timeout_secs,
        } => Active::WaitSilence {
            start_ms: now_ms,
            silence_ms: silence_ms as u64,
            deadline_ms: now_ms + (timeout_secs.max(0.0) * 1000.0) as u64,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::audio::linear_to_alaw;

    /// 振幅 `amp` 的方波 frame（A-law）
    fn tone(amp: i16) -> Vec<u8> {
        (0..160)
            .map(|i| linear_to_alaw(if i % 2 == 0 { amp } else { -amp }))
            .collect()
    }

    fn wav(dir: &std::path::Path, name: &str, frames: usize) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, tone(8000).repeat(frames)).unwrap();
        path
    }

    async fn player(steps: Vec<MediaStep>) -> ScriptPlayer {
        ScriptPlayer::load(&steps).await.unwrap()
    }

    fn is_silence(o: &Out) -> bool {
        *o == Out::Audio(silence_frame())
    }

    #[test]
    fn voice_detection_threshold() {
        assert!(!is_voice(PT_PCMA, &silence_frame()));
        assert!(!is_voice(PT_PCMA, &tone(100)), "背景雜訊不算說話");
        assert!(is_voice(PT_PCMA, &tone(3000)));
        assert!(!is_voice(101, &tone(3000)), "按鍵事件不算說話");
        let ulaw: Vec<u8> = (0..160)
            .map(|i| crate::rtp::audio::linear_to_ulaw(if i % 2 == 0 { 3000 } else { -3000 }))
            .collect();
        assert!(is_voice(PT_PCMU, &ulaw));
    }

    #[tokio::test]
    async fn wait_then_play_then_silence_forever() {
        let dir = tempdir();
        let clip = wav(&dir, "a.al", 3);
        let mut p = player(vec![
            MediaStep::Silence { secs: 0.1 },
            MediaStep::Play { files: vec![clip] },
        ])
        .await;
        let v = VoiceActivity::default();
        let outs: Vec<Out> = (0..10).map(|t| p.next(t, &v)).collect();
        assert!(outs[..5].iter().all(is_silence), "前 100 ms 靜音");
        assert!(
            outs[5..8].iter().all(|o| *o == Out::Audio(tone(8000))),
            "接著播 3 個 frame"
        );
        assert!(!p.is_done() || outs[8..].iter().all(is_silence));
        assert!(outs[8..].iter().all(is_silence), "播完只送靜音、不循環");
        assert!(p.is_done());
    }

    #[tokio::test]
    async fn dtmf_events_share_start_timestamp_and_end_with_gap() {
        let mut p = player(vec![MediaStep::Dtmf {
            digits: "1#".into(),
        }])
        .await;
        let v = VoiceActivity::default();
        let outs: Vec<Out> = (100..140).map(|t| p.next(t, &v)).collect();
        // 每個按鍵：8 個事件 + 3 個結束封包 + 5 個靜音 = 16 tick
        let starts: Vec<u64> = outs
            .iter()
            .filter_map(|o| match o {
                Out::Event { start_tick, .. } => Some(*start_tick),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 22);
        assert!(starts[..11].iter().all(|&s| s == 100));
        assert!(starts[11..].iter().all(|&s| s == 116));
        match &outs[0] {
            Out::Event { payload, .. } => assert_eq!(payload[0], 1),
            o => panic!("{o:?}"),
        }
        match &outs[16] {
            Out::Event { payload, .. } => assert_eq!(payload[0], 11, "# = event 11"),
            o => panic!("{o:?}"),
        }
        assert!(outs[11..16].iter().all(is_silence), "按鍵之間 100 ms 靜音");
        assert!(outs[32..].iter().all(is_silence));
        assert!(p.is_done());
    }

    #[tokio::test]
    async fn wait_silence_waits_for_remote_to_finish() {
        let dir = tempdir();
        let clip = wav(&dir, "b.al", 1);
        let mut p = player(vec![
            MediaStep::WaitSilence {
                silence_ms: 200,
                timeout_secs: 10.0,
            },
            MediaStep::Play { files: vec![clip] },
        ])
        .await;
        let v = VoiceActivity::default();
        // 對方 0–1 秒在說話（tick 0–49），之後安靜
        let mut played_at = None;
        for t in 0..200u64 {
            if t < 50 {
                v.on_frame(PT_PCMA, &tone(3000), t * 20);
            }
            if p.next(t, &v) == Out::Audio(tone(8000)) {
                played_at = Some(t);
                break;
            }
        }
        // 最後一次說話在 980 ms，安靜 200 ms 後（1180 ms = tick 59）才播
        assert_eq!(played_at, Some(59));
    }

    #[tokio::test]
    async fn wait_silence_times_out_when_remote_never_speaks() {
        let mut p = player(vec![
            MediaStep::WaitSilence {
                silence_ms: 300,
                timeout_secs: 1.0,
            },
            MediaStep::Dtmf { digits: "5".into() },
        ])
        .await;
        let v = VoiceActivity::default();
        let first_event = (0..200u64)
            .find(|&t| matches!(p.next(t, &v), Out::Event { .. }))
            .unwrap();
        assert_eq!(first_event, 50, "等 1 秒逾時後按鍵");
    }

    #[tokio::test]
    async fn wait_silence_ignores_speech_before_the_step() {
        let mut p = player(vec![
            MediaStep::Silence { secs: 1.0 },
            MediaStep::WaitSilence {
                silence_ms: 200,
                timeout_secs: 2.0,
            },
            MediaStep::Dtmf { digits: "1".into() },
        ])
        .await;
        let v = VoiceActivity::default();
        // 招呼語在第一步期間就講完了：等對方講完的步驟不能把它當成「已經講完」
        v.on_frame(PT_PCMA, &tone(3000), 500);
        let first_event = (0..300u64)
            .find(|&t| matches!(p.next(t, &v), Out::Event { .. }))
            .unwrap();
        assert_eq!(first_event, 150, "沒有新的說話 → 等到逾時（1 s + 2 s）");
    }

    #[tokio::test]
    async fn play_picks_one_of_the_files() {
        let dir = tempdir();
        let a = wav(&dir, "x.al", 1);
        std::fs::write(dir.join("y.al"), tone(2000)).unwrap();
        let files = vec![a, dir.join("y.al")];
        let mut seen = std::collections::HashSet::new();
        for _ in 0..40 {
            let mut p = player(vec![MediaStep::Play {
                files: files.clone(),
            }])
            .await;
            if let Out::Audio(f) = p.next(0, &VoiceActivity::default()) {
                seen.insert(f);
            }
        }
        assert_eq!(seen.len(), 2, "兩個檔案都會被挑到");
    }

    #[tokio::test]
    async fn skip_keeps_audio_in_step_with_time() {
        let dir = tempdir();
        let path = dir.join("c.al");
        let frames: Vec<u8> = [tone(1000), tone(2000), tone(3000), tone(4000)].concat();
        std::fs::write(&path, frames).unwrap();
        let mut p = player(vec![MediaStep::Play { files: vec![path] }]).await;
        let v = VoiceActivity::default();
        assert_eq!(p.next(0, &v), Out::Audio(tone(1000)));
        p.skip(2);
        assert_eq!(p.next(3, &v), Out::Audio(tone(4000)));
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sipress-script-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
