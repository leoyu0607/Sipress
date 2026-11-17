//! 雲端 TTS：把通話流程「說話」步驟的文字合成為電話音質（8 kHz 單聲道 G.711 A-law）的 WAV 並快取。
//!
//! 測試開始前每段文字只呼叫一次服務；相同服務 + 聲音 + 文字之後直接用快取，
//! 壓測期間不會連線到 TTS 服務。
use crate::config::{TtsConfig, TtsProvider};
use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 合成一段文字，回傳快取的 WAV 路徑（已存在就直接回傳，不連線）
pub async fn synthesize(cfg: &TtsConfig, text: &str) -> Result<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        bail!("TTS 文字不可空白");
    }
    let path = cache_path(cfg, text);
    if tokio::fs::metadata(&path)
        .await
        .is_ok_and(|m| m.len() > WAV_HEADER_LEN as u64)
    {
        return Ok(path);
    }
    let key = cfg.key().with_context(|| {
        format!(
            "尚未設定 TTS 金鑰（設定中填寫或設環境變數 {}）",
            TtsConfig::KEY_ENV
        )
    })?;
    let alaw = match cfg.provider {
        TtsProvider::Azure => azure(cfg, &key, text).await?,
        TtsProvider::Google => google(cfg, &key, text).await?,
    };
    if alaw.len() < 160 {
        bail!("TTS 服務回傳的音訊太短（{} bytes）", alaw.len());
    }
    tokio::fs::create_dir_all(&cfg.cache_dir)
        .await
        .with_context(|| format!("無法建立 TTS 快取資料夾 {}", cfg.cache_dir))?;
    // 先寫暫存檔再改名：中途失敗不會留下不完整的快取
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    tokio::fs::write(&tmp, alaw_wav(&alaw))
        .await
        .with_context(|| format!("無法寫入 TTS 快取 {}", tmp.display()))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .with_context(|| format!("無法寫入 TTS 快取 {}", path.display()))?;
    tracing::info!(
        "TTS 合成完成：{}（{:.1} 秒）→ {}",
        text,
        alaw.len() as f64 / 8000.0,
        path.display()
    );
    Ok(path)
}

/// 快取檔名：服務 + 聲音 + 文字的雜湊（換聲音或改字就重新合成）
pub fn cache_path(cfg: &TtsConfig, text: &str) -> PathBuf {
    let provider = match cfg.provider {
        TtsProvider::Azure => "azure",
        TtsProvider::Google => "google",
    };
    let mut h = Md5::new();
    h.update(format!("{}\n{}\n{}", provider, cfg.voice(), text.trim()).as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Path::new(&cfg.cache_dir).join(format!("{provider}_{hex}.wav"))
}

fn client() -> Result<reqwest::Client> {
    // TLS 用 ring（不需 cmake / NASM 即可在三個平台編譯）；已安裝過時回傳 Err，忽略即可
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("sipress/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("無法建立 HTTP 連線")
}

/// 聲音名稱前兩段即語系（`zh-TW-HsiaoChenNeural` → `zh-TW`、`cmn-TW-Wavenet-A` → `cmn-TW`）
fn voice_language(voice: &str) -> String {
    voice.splitn(3, '-').take(2).collect::<Vec<_>>().join("-")
}

/// Azure AI Speech：SSML → raw 8 kHz A-law
async fn azure(cfg: &TtsConfig, key: &str, text: &str) -> Result<Vec<u8>> {
    let region = cfg.region.trim();
    if region.is_empty() || !region.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("Azure TTS 需要填寫區域（例如 eastasia）");
    }
    let voice = cfg.voice();
    let ssml = format!(
        "<speak version='1.0' xml:lang='{lang}'><voice name='{voice}'>{text}</voice></speak>",
        lang = xml_escape(&voice_language(voice)),
        voice = xml_escape(voice),
        text = xml_escape(text),
    );
    let resp = client()?
        .post(format!(
            "https://{region}.tts.speech.microsoft.com/cognitiveservices/v1"
        ))
        .header("Ocp-Apim-Subscription-Key", key)
        .header("Content-Type", "application/ssml+xml")
        .header("X-Microsoft-OutputFormat", "raw-8khz-8bit-mono-alaw")
        .body(ssml)
        .send()
        .await
        .context("無法連線到 Azure TTS（請確認網路與區域）")?;
    let resp = check(resp, "Azure").await?;
    Ok(resp.bytes().await?.to_vec())
}

/// Google Cloud Text-to-Speech：JSON → base64 的 A-law（含 WAV header）
async fn google(cfg: &TtsConfig, key: &str, text: &str) -> Result<Vec<u8>> {
    let voice = cfg.voice();
    let body = serde_json::json!({
        "input": { "text": text },
        "voice": { "languageCode": voice_language(voice), "name": voice },
        "audioConfig": { "audioEncoding": "ALAW", "sampleRateHertz": 8000 },
    });
    let resp = client()?
        .post("https://texttospeech.googleapis.com/v1/text:synthesize")
        .header("X-Goog-Api-Key", key)
        .json(&body)
        .send()
        .await
        .context("無法連線到 Google TTS（請確認網路）")?;
    let resp = check(resp, "Google").await?;
    let v: serde_json::Value = resp.json().await.context("Google TTS 回應格式錯誤")?;
    let b64 = v["audioContent"]
        .as_str()
        .context("Google TTS 回應沒有 audioContent")?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("Google TTS 音訊解碼失敗")?;
    Ok(strip_wav_header(bytes))
}

/// 非 2xx 時把狀態碼與服務的錯誤訊息帶進錯誤
async fn check(resp: reqwest::Response, name: &str) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let detail = error_detail(&body);
    let hint = match status.as_u16() {
        401 | 403 => "（金鑰錯誤、沒有權限或區域不符）",
        // Google 對無效金鑰也回 400
        400 => "（金鑰、聲音名稱或文字有誤）",
        429 => "（超過服務的流量限制，請稍後再試）",
        _ => "",
    };
    if detail.is_empty() {
        bail!("{} TTS 回應 {}{}", name, status, hint)
    }
    bail!("{} TTS 回應 {}{}：{}", name, status, hint, detail)
}

/// 服務回傳的錯誤說明：JSON 的 `error.message`（Google）或純文字，壓成一行
fn error_detail(body: &str) -> String {
    let text = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.to_string());
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.chars().take(300).collect()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

const WAV_HEADER_LEN: usize = 44;

/// 若是 WAV 就取出 data chunk，否則視為 raw A-law
fn strip_wav_header(bytes: Vec<u8>) -> Vec<u8> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return bytes;
    }
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        if id == b"data" {
            let end = start.saturating_add(len).min(bytes.len());
            return bytes[start..end].to_vec();
        }
        pos = start + len + (len & 1);
    }
    Vec::new()
}

/// raw A-law → 8 kHz 單聲道 A-law WAV（format 6）
pub fn alaw_wav(alaw: &[u8]) -> Vec<u8> {
    let mut w = Vec::with_capacity(WAV_HEADER_LEN + alaw.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + alaw.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&6u16.to_le_bytes()); // A-law
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&8000u32.to_le_bytes());
    w.extend_from_slice(&8000u32.to_le_bytes()); // byte rate
    w.extend_from_slice(&1u16.to_le_bytes()); // block align
    w.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(alaw.len() as u32).to_le_bytes());
    w.extend_from_slice(alaw);
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::audio::AudioSource;

    #[test]
    fn wav_round_trip_is_readable_by_audio_source() {
        let alaw: Vec<u8> = (0..1600u32).map(|i| (i % 251) as u8).collect();
        let wav = alaw_wav(&alaw);
        assert_eq!(strip_wav_header(wav.clone()), alaw);
        assert_eq!(strip_wav_header(alaw.clone()), alaw, "raw 原樣保留");

        let path = std::env::temp_dir().join(format!("sipress-tts-{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&path, &wav).unwrap();
        let src = AudioSource::from_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(src.frame_count(), 10, "1600 bytes = 10 個 20 ms frame");
    }

    #[test]
    fn cache_key_depends_on_provider_voice_and_text() {
        let base = TtsConfig::default();
        let p = |c: &TtsConfig, t: &str| cache_path(c, t);
        assert_eq!(p(&base, "您好"), p(&base, " 您好 "));
        assert_ne!(p(&base, "您好"), p(&base, "你好"));
        let google = TtsConfig {
            provider: TtsProvider::Google,
            ..base.clone()
        };
        assert_ne!(p(&base, "您好"), p(&google, "您好"));
        let voice = TtsConfig {
            voice: "zh-TW-YunJheNeural".into(),
            ..base.clone()
        };
        assert_ne!(p(&base, "您好"), p(&voice, "您好"));
        // 金鑰不影響快取
        let key = TtsConfig {
            api_key: "x".into(),
            ..base.clone()
        };
        assert_eq!(p(&base, "您好"), p(&key, "您好"));
    }

    #[test]
    fn error_detail_extracts_service_message() {
        let google = r#"{ "error": { "code": 400, "message": "API key not valid. Please pass a valid API key.", "status": "INVALID_ARGUMENT" } }"#;
        assert_eq!(
            error_detail(google),
            "API key not valid. Please pass a valid API key."
        );
        assert_eq!(error_detail("Bad\r\n  request"), "Bad request");
        assert_eq!(error_detail(""), "");
    }

    #[test]
    fn language_and_escaping() {
        assert_eq!(voice_language("zh-TW-HsiaoChenNeural"), "zh-TW");
        assert_eq!(voice_language("cmn-TW-Wavenet-A"), "cmn-TW");
        assert_eq!(xml_escape("A&B <x> 'y'"), "A&amp;B &lt;x&gt; &apos;y&apos;");
    }

    #[tokio::test]
    async fn cached_file_is_used_without_network_or_key() {
        let dir = std::env::temp_dir().join(format!("sipress-ttsc-{}", uuid::Uuid::new_v4()));
        let cfg = TtsConfig {
            cache_dir: dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        std::fs::create_dir_all(&dir).unwrap();
        let path = cache_path(&cfg, "請按 1");
        std::fs::write(&path, alaw_wav(&[0xD5; 320])).unwrap();
        assert_eq!(synthesize(&cfg, "請按 1").await.unwrap(), path);
        std::fs::remove_dir_all(&dir).ok();
    }
}
