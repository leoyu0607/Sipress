/// 音檔讀取與 G.711 編碼
/// 支援輸入格式（全部自動轉換為 G.711A / PCMA / PT=8 輸出）：
///   - WAV PCM16（8kHz/16kHz mono/stereo，自動重新取樣至 8kHz）
///   - WAV A-law（format=6，直接使用原始 bytes）
///   - WAV μ-law（format=7，decode → A-law 重新編碼）
///   - raw G.711 μ-law（.ul/.ulaw/.pcmu）→ decode → A-law 重新編碼
///   - raw G.711 A-law（.al/.alaw/.pcma）→ 直接使用
/// 輸出：循環播放的 160-byte（20ms @ 8kHz）G.711A（PCMA，PT=8）封包流

use anyhow::{bail, Context, Result};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};
use tokio::sync::OnceCell;

/// 已解碼音檔的快取鍵：路徑 + 修改時間 + 大小（檔案被替換時會重新載入）
type ClipKey = (PathBuf, Option<SystemTime>, u64);
/// 已解碼的 frames（所有通話共用，只讀）
type Frames = Arc<Vec<Vec<u8>>>;

/// 行程內音檔快取：同一音檔只讀取 / 解碼 / 重新取樣一次
fn clip_cache() -> &'static Mutex<HashMap<ClipKey, Arc<OnceCell<Frames>>>> {
    static CACHE: OnceLock<Mutex<HashMap<ClipKey, Arc<OnceCell<Frames>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// 音訊來源（已解碼為 G.711 bytes，可循環）
pub struct AudioSource {
    /// 所有 frames（每個 frame = 160 bytes = 20ms），多個通話共用同一份
    frames:          Frames,
    /// 目前播放位置
    cursor:          usize,
    /// 是否循環播放
    pub looping:     bool,
    /// RTP Payload Type：0 = PCMU，8 = PCMA
    pub payload_type: u8,
}

impl AudioSource {
    /// 從檔案載入（自動偵測格式，統一輸出 G.711A PCMA PT=8）
    pub fn from_file(path: &Path) -> Result<Self> {
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let bytes = match ext.as_str() {
            "wav" => {
                // WAV 自動偵測 format，統一轉成 G.711A
                Self::load_wav_to_pcma(path)?
            }
            // raw G.711 μ-law → 轉 A-law
            "ul" | "ulaw" | "pcmu" | "raw" => {
                let raw = fs::read(path)
                    .with_context(|| format!("無法讀取音檔: {}", path.display()))?;
                Self::ulaw_bytes_to_alaw(raw)
            }
            // raw G.711 A-law → 直接使用
            "al" | "alaw" | "pcma" => {
                fs::read(path)
                    .with_context(|| format!("無法讀取音檔: {}", path.display()))?
            }
            other => bail!("不支援的音檔格式: .{}（支援 .wav / .ul / .ulaw / .al / .alaw / .raw）", other),
        };

        let frames = Self::split_frames(bytes);
        if frames.is_empty() {
            bail!("音檔內容為空或太短（最少需要 160 bytes / 20ms）");
        }

        // 永遠使用 PT=8（G.711A PCMA），與 SDP offer 一致
        Ok(Self { frames: Arc::new(frames), cursor: 0, looping: true, payload_type: 8 })
    }

    /// 從快取載入音檔：同一檔案（路徑 + 修改時間 + 大小相同）只解碼一次，
    /// 之後每通電話只複製 `Arc`；首次載入在 blocking 執行緒進行，不阻塞 async runtime。
    /// 同時啟動的多通電話會等待同一次載入，不會重複解碼。
    pub async fn from_file_cached(path: &Path) -> Result<Self> {
        let meta = tokio::fs::metadata(path).await
            .with_context(|| format!("無法讀取音檔: {}", path.display()))?;
        let key: ClipKey = (path.to_path_buf(), meta.modified().ok(), meta.len());
        let cell = {
            let mut cache = clip_cache().lock().unwrap();
            // 同一路徑的舊版本（檔案已被替換）不再需要
            cache.retain(|k, _| k.0 != key.0 || *k == key);
            Arc::clone(cache.entry(key).or_default())
        };
        let frames = cell.get_or_try_init(|| async {
            let path = path.to_path_buf();
            let src = tokio::task::spawn_blocking(move || Self::from_file(&path)).await??;
            Ok::<_, anyhow::Error>(src.frames)
        }).await?;
        Ok(Self { frames: Arc::clone(frames), cursor: 0, looping: true, payload_type: 8 })
    }

    /// 產生靜音來源（G.711A PCMA 靜音，PT=8）
    pub fn silence() -> Self {
        // G.711A 零電平 = 0xD5（linear 0 → A-law 編碼）
        let frame = vec![0xD5u8; 160];
        Self { frames: Arc::new(vec![frame; 50]), cursor: 0, looping: true, payload_type: 8 }
    }

    /// 取得下一個 20ms frame（160 bytes）
    /// 返回 None 表示播放結束（非循環模式）
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.cursor >= self.frames.len() {
            if self.looping {
                self.cursor = 0;
            } else {
                return None;
            }
        }
        let frame = self.frames[self.cursor].clone();
        self.cursor += 1;
        Some(frame)
    }

    /// 重置到開頭
    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// 總 frame 數（每 frame 20ms）
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// 音檔總時長（秒）
    pub fn duration_secs(&self) -> f64 {
        self.frames.len() as f64 * 0.020
    }

    // ── 私有：WAV 解析（統一輸出 G.711A PCMA bytes）──────────────

    /// 載入 WAV 並轉換為 G.711A（PCMA）bytes
    /// 支援 WAV format=1（PCM）、6（A-law）、7（μ-law），自動重新取樣至 8kHz
    fn load_wav_to_pcma(path: &Path) -> Result<Vec<u8>> {
        let data = fs::read(path)
            .with_context(|| format!("無法讀取 WAV 檔案: {}", path.display()))?;

        if data.len() < 44 {
            bail!("WAV 檔案過小（< 44 bytes）");
        }
        if &data[0..4] != b"RIFF" {
            bail!("不是有效的 WAV 檔案（缺少 RIFF header）");
        }
        if &data[8..12] != b"WAVE" {
            bail!("不是 WAVE 格式");
        }

        let (audio_format, channels, sample_rate, bits) = Self::parse_fmt_chunk(&data)?;

        tracing::info!(
            "音檔 {:?} format={} channels={} rate={} bits={}",
            path.file_name().unwrap_or_default(),
            audio_format, channels, sample_rate, bits
        );

        let pcma_bytes: Vec<u8> = match audio_format {
            // ── PCM16 → G.711A ─────────────────────────────────────
            1 => {
                if bits != 16 {
                    bail!("PCM WAV 只支援 16-bit，收到 {}-bit", bits);
                }
                let pcm16 = Self::find_wav_data_i16(&data)?;
                let mono = if channels == 2 {
                    Self::stereo_to_mono(&pcm16)
                } else {
                    pcm16
                };
                let at8k = if sample_rate != 8000 {
                    Self::resample(&mono, sample_rate, 8000)
                } else {
                    mono
                };
                // PCM16 → A-law
                at8k.iter().map(|&s| linear_to_alaw(s)).collect()
            }

            // ── G.711A WAV → 直接使用（若需重取樣則 decode→resample→encode）
            6 => {
                if bits != 8 {
                    bail!("A-law WAV 期望 8-bit，收到 {}-bit", bits);
                }
                let mut raw = Self::find_wav_data_raw(&data)?;
                if channels == 2 {
                    raw = raw.into_iter().step_by(2).collect();
                }
                if sample_rate != 8000 {
                    let linear: Vec<i16> = raw.iter().map(|&b| alaw_to_linear(b)).collect();
                    let at8k = Self::resample(&linear, sample_rate, 8000);
                    at8k.iter().map(|&s| linear_to_alaw(s)).collect()
                } else {
                    raw // 已是 8kHz G.711A，直接使用
                }
            }

            // ── G.711μ WAV → 轉 G.711A ────────────────────────────
            7 => {
                if bits != 8 {
                    bail!("μ-law WAV 期望 8-bit，收到 {}-bit", bits);
                }
                let mut raw = Self::find_wav_data_raw(&data)?;
                if channels == 2 {
                    raw = raw.into_iter().step_by(2).collect();
                }
                let linear: Vec<i16> = raw.iter().map(|&b| ulaw_to_linear(b)).collect();
                let at8k = if sample_rate != 8000 {
                    Self::resample(&linear, sample_rate, 8000)
                } else {
                    linear
                };
                // μ-law → linear → A-law
                at8k.iter().map(|&s| linear_to_alaw(s)).collect()
            }

            other => bail!(
                "不支援的 WAV 格式 format={}（支援：1=PCM16 / 6=A-law / 7=μ-law）",
                other
            ),
        };

        Ok(pcma_bytes)
    }

    /// raw μ-law bytes → G.711A（A-law）bytes
    fn ulaw_bytes_to_alaw(raw: Vec<u8>) -> Vec<u8> {
        raw.iter()
            .map(|&b| linear_to_alaw(ulaw_to_linear(b)))
            .collect()
    }

    /// 尋找並解析 fmt chunk，回傳 (audio_format, channels, sample_rate, bits_per_sample)
    fn parse_fmt_chunk(data: &[u8]) -> Result<(u16, u16, u32, u16)> {
        let mut i = 12usize;
        while i + 8 <= data.len() {
            let chunk_id   = &data[i..i+4];
            let chunk_size = u32::from_le_bytes([data[i+4],data[i+5],data[i+6],data[i+7]]) as usize;
            if chunk_id == b"fmt " && chunk_size >= 16 {
                let audio_format = u16::from_le_bytes([data[i+8],  data[i+9]]);
                let channels     = u16::from_le_bytes([data[i+10], data[i+11]]);
                let sample_rate  = u32::from_le_bytes([data[i+12], data[i+13], data[i+14], data[i+15]]);
                let bits         = u16::from_le_bytes([data[i+22], data[i+23]]);
                return Ok((audio_format, channels, sample_rate, bits));
            }
            i += 8 + chunk_size;
            if chunk_size % 2 != 0 { i += 1; }
        }
        // fallback：嘗試固定偏移（舊版簡單 WAV）
        if data.len() >= 36 {
            let audio_format = u16::from_le_bytes([data[20], data[21]]);
            let channels     = u16::from_le_bytes([data[22], data[23]]);
            let sample_rate  = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
            let bits         = u16::from_le_bytes([data[34], data[35]]);
            return Ok((audio_format, channels, sample_rate, bits));
        }
        bail!("找不到有效的 fmt chunk");
    }

    /// 找 data chunk，回傳 i16 PCM 樣本（用於 PCM WAV）
    fn find_wav_data_i16(data: &[u8]) -> Result<Vec<i16>> {
        let raw = Self::find_wav_data_raw(data)?;
        Ok(raw.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect())
    }

    /// 找 data chunk，回傳原始 bytes（用於 A-law / μ-law WAV）
    fn find_wav_data_raw(data: &[u8]) -> Result<Vec<u8>> {
        let mut i = 12usize;
        while i + 8 <= data.len() {
            let chunk_id   = &data[i..i+4];
            let chunk_size = u32::from_le_bytes([data[i+4],data[i+5],data[i+6],data[i+7]]) as usize;
            if chunk_id == b"data" {
                let start = i + 8;
                let end   = (start + chunk_size).min(data.len());
                return Ok(data[start..end].to_vec());
            }
            i += 8 + chunk_size;
            if chunk_size % 2 != 0 { i += 1; } // padding byte
        }
        bail!("WAV 檔案中找不到 data chunk");
    }

    fn stereo_to_mono(samples: &[i16]) -> Vec<i16> {
        samples.chunks_exact(2)
            .map(|s| ((s[0] as i32 + s[1] as i32) / 2) as i16)
            .collect()
    }

    /// 線性插值重採樣（src_rate → dst_rate）
    fn resample(samples: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
        if src_rate == dst_rate { return samples.to_vec(); }
        let ratio  = src_rate as f64 / dst_rate as f64;
        let n      = (samples.len() as f64 / ratio) as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = pos - idx as f64;
            let a = *samples.get(idx).unwrap_or(&0) as f64;
            let b = *samples.get(idx + 1).unwrap_or(&0) as f64;
            out.push((a + frac * (b - a)) as i16);
        }
        out
    }

    fn split_frames(data: Vec<u8>) -> Vec<Vec<u8>> {
        data.chunks(160)
            .filter(|c| c.len() == 160)
            .map(|c| c.to_vec())
            .collect()
    }
}

// ── G.711 A-law 編解碼（ITU-T G.711）────────────────────────────

/// G.711 A-law → 16-bit linear PCM
/// 遵循 ITU-T G.711 / sox alaw2linear 實作
pub fn alaw_to_linear(a: u8) -> i16 {
    let a = a ^ 0x55;
    let sign = a & 0x80;
    let seg  = ((a & 0x70) >> 4) as i32;
    let mut t = (a & 0x0f) as i32;
    match seg {
        0 => { t <<= 1; }
        1 => { t += 0x10; t <<= 1; }
        _ => { t += 0x10; t <<= seg; }
    }
    if sign != 0 { t as i16 } else { -(t as i16) }
}

/// 16-bit linear PCM → G.711 A-law
/// 遵循 ITU-T G.711 / sox linear2alaw 實作
pub fn linear_to_alaw(sample: i16) -> u8 {
    // G.711 A-law 段落端點（16-bit 輸入）
    const SEG_END: [i32; 8] = [0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF, 0x3FFF, 0x7FFF];

    let (mask, mut s): (u8, i32) = if sample >= 0 {
        (0xD5, sample as i32)
    } else {
        (0x55, -(sample as i32) - 8)
    };

    if s < 0 { s = 0; }

    // 找段落（exponent）
    let mut seg = 8usize;
    for (i, &end) in SEG_END.iter().enumerate() {
        if s <= end { seg = i; break; }
    }
    if seg >= 8 { return 0x7F ^ mask; }

    let mant = if seg < 2 {
        ((s >> 1) & 0x0f) as u8
    } else {
        ((s >> (seg + 1)) & 0x0f) as u8
    };

    let aval = ((seg as u8) << 4) | mant;
    aval ^ mask
}

// ── G.711 μ-law 編碼（ITU-T G.711）──────────────────────────────

/// 16-bit linear PCM → 8-bit μ-law
pub fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;

    let sign  = if sample < 0 { 0x80u8 } else { 0u8 };
    let mut s = sample as i32;
    if s < 0 { s = -s; }
    s += BIAS;
    if s > CLIP { s = CLIP; }

    let exponent = ULAW_EXP_TABLE[((s >> 7) & 0xFF) as usize];
    let mantissa = ((s >> (exponent as u32 + 3)) & 0x0F) as u8;
    let ulaw     = !(sign | ((exponent << 4) as u8) | mantissa);
    ulaw
}

/// μ-law → 16-bit linear（用於 MOS 計算對比）
pub fn ulaw_to_linear(ulaw: u8) -> i16 {
    let ulaw   = !ulaw;
    let sign   = ulaw & 0x80;
    let exp    = ((ulaw >> 4) & 0x07) as u32;
    let mant   = (ulaw & 0x0F) as i32;
    let mut s  = ((mant << 1) | 33) << (exp + 2);
    s         -= 33;
    if sign != 0 { -s as i16 } else { s as i16 }
}

static ULAW_EXP_TABLE: [u8; 256] = [
    0,0,1,1,2,2,2,2,3,3,3,3,3,3,3,3,
    4,4,4,4,4,4,4,4,4,4,4,4,4,4,4,4,
    5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,
    5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_alaw(name: &str, frames: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sipress-audio-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, vec![0xD5u8; 160 * frames]).unwrap();
        path
    }

    #[tokio::test]
    async fn cached_loads_share_decoded_frames() {
        let path = temp_alaw("shared.al", 10);
        let a = AudioSource::from_file_cached(&path).await.unwrap();
        let b = AudioSource::from_file_cached(&path).await.unwrap();
        assert!(Arc::ptr_eq(&a.frames, &b.frames), "同一音檔應共用同一份解碼結果");
        assert_eq!(a.frame_count(), 10);
    }

    #[tokio::test]
    async fn concurrent_first_loads_decode_once() {
        let path = temp_alaw("concurrent.al", 10);
        let loads = (0..20).map(|_| {
            let p = path.clone();
            async move { AudioSource::from_file_cached(&p).await }
        });
        let sources = futures_join_all(loads).await;
        let first = &sources[0].frames;
        assert!(sources.iter().all(|s| Arc::ptr_eq(&s.frames, first)));
    }

    #[tokio::test]
    async fn replaced_file_is_reloaded() {
        let path = temp_alaw("replaced.al", 10);
        let a = AudioSource::from_file_cached(&path).await.unwrap();
        fs::write(&path, vec![0xD5u8; 160 * 25]).unwrap();
        let b = AudioSource::from_file_cached(&path).await.unwrap();
        assert_eq!(a.frame_count(), 10);
        assert_eq!(b.frame_count(), 25, "檔案內容變更後應重新載入");
    }

    #[tokio::test]
    async fn load_errors_are_not_cached() {
        let path = temp_alaw("bad.al", 0); // 空檔 → 解碼失敗
        assert!(AudioSource::from_file_cached(&path).await.is_err());
        fs::write(&path, vec![0xD5u8; 160 * 3]).unwrap();
        // 大小不同 → 不同快取鍵；即使鍵相同，失敗的載入也不會留在 OnceCell 中
        assert_eq!(AudioSource::from_file_cached(&path).await.unwrap().frame_count(), 3);
    }

    /// 簡易 join_all（不額外引入 futures crate）
    async fn futures_join_all<F: std::future::Future<Output = Result<AudioSource>> + Send + 'static>(
        futs: impl Iterator<Item = F>,
    ) -> Vec<AudioSource> {
        let handles: Vec<_> = futs.map(tokio::spawn).collect();
        let mut out = Vec::new();
        for h in handles {
            out.push(h.await.unwrap().unwrap());
        }
        out
    }
}
