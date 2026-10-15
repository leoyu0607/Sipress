//! SDP 的 codec 協商（只處理 G.711：0 = PCMU、8 = PCMA，以及 101 = telephone-event）
//!
//! - 發話端（民眾端）：offer 把偏好的 codec 放第一個，依對方 answer 決定實際送出的 payload type。
//! - 接聽端（座席端）：依 INVITE 的 offer 選 codec（偏好的有被提供就用它，否則用 offer 中第一個
//!   支援的），answer 只宣告選中的那一個；offer 完全沒有 G.711 時應回 488。

pub const PT_PCMU: u8 = 0;
pub const PT_PCMA: u8 = 8;
pub const PT_DTMF: u8 = 101;

fn is_g711(pt: u8) -> bool {
    pt == PT_PCMU || pt == PT_PCMA
}

/// 取出訊息 body 中第一個 `m=audio` 行列出的 payload type（依原順序）；沒有 SDP 時為空
pub fn audio_payloads(raw: &str) -> Vec<u8> {
    let Some(start) = raw
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| raw.find("\n\n").map(|i| i + 2))
    else {
        return Vec::new();
    };
    raw[start..]
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("m=audio"))
        .map(|l| {
            // m=audio <port> <proto> <fmt> <fmt> ...
            l.split_whitespace()
                .skip(3)
                .filter_map(|f| f.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 接聽端選 codec：offer 有偏好的就用偏好的，否則用 offer 中第一個 G.711；
/// 沒有 SDP（offer 為空）時無從協商，沿用偏好的；offer 只有其他 codec 時回傳 None（應回 488）
pub fn negotiate(offer: &[u8], preferred: u8) -> Option<u8> {
    if offer.is_empty() || offer.contains(&preferred) {
        return Some(preferred);
    }
    offer.iter().copied().find(|&pt| is_g711(pt))
}

/// 發話端依 answer 決定送出的 codec：answer 中第一個 G.711；answer 沒有 SDP 或沒有 G.711 時沿用 `fallback`
pub fn answered(answer: &[u8], fallback: u8) -> u8 {
    answer
        .iter()
        .copied()
        .find(|&pt| is_g711(pt))
        .unwrap_or(fallback)
}

/// offer 的 payload 順序：偏好的 codec、另一個 G.711、telephone-event
pub fn offer_payloads(preferred: u8) -> Vec<u8> {
    let other = if preferred == PT_PCMU {
        PT_PCMA
    } else {
        PT_PCMU
    };
    vec![preferred, other, PT_DTMF]
}

/// answer 的 payload：選中的 codec，對方 offer 有 telephone-event 時一併接受
pub fn answer_payloads(codec: u8, offer: &[u8]) -> Vec<u8> {
    let mut v = vec![codec];
    if offer.contains(&PT_DTMF) {
        v.push(PT_DTMF);
    }
    v
}

/// 組 SDP body（`ip` 為本機 IP，`payloads` 依順序寫入 m= 行並附上 rtpmap）
pub fn body(ip: &str, port: u16, payloads: &[u8]) -> String {
    let fmts: Vec<String> = payloads.iter().map(u8::to_string).collect();
    let mut s = format!(
        "v=0\r\n\
         o=sipress 1000 1000 IN IP4 {ip}\r\n\
         s=sipress\r\n\
         c=IN IP4 {ip}\r\n\
         t=0 0\r\n\
         m=audio {port} RTP/AVP {fmts}\r\n",
        fmts = fmts.join(" "),
    );
    for &pt in payloads {
        match pt {
            PT_PCMA => s.push_str("a=rtpmap:8 PCMA/8000\r\n"),
            PT_PCMU => s.push_str("a=rtpmap:0 PCMU/8000\r\n"),
            PT_DTMF => s.push_str("a=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\n"),
            _ => {}
        }
    }
    s.push_str("a=ptime:20\r\na=sendrecv\r\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(m_line: &str) -> String {
        format!(
            "INVITE sip:1001@h SIP/2.0\r\nCall-ID: x\r\nContent-Type: application/sdp\r\n\r\n\
             v=0\r\nc=IN IP4 10.0.0.1\r\nt=0 0\r\n{m_line}\r\na=rtpmap:0 PCMU/8000\r\n"
        )
    }

    #[test]
    fn parses_payloads_in_offer_order() {
        assert_eq!(
            audio_payloads(&msg("m=audio 4000 RTP/AVP 0 8 101")),
            vec![0, 8, 101]
        );
        assert_eq!(
            audio_payloads(&msg("m=audio 4000 RTP/AVP 18 0")),
            vec![18, 0]
        );
        assert!(audio_payloads("BYE sip:a SIP/2.0\r\nCall-ID: x\r\n\r\n").is_empty());
        // 只看 audio，不看 video
        let both = msg("m=video 5000 RTP/AVP 96\r\nm=audio 4000 RTP/AVP 8");
        assert_eq!(audio_payloads(&both), vec![8]);
    }

    #[test]
    fn answerer_prefers_configured_codec_when_offered() {
        assert_eq!(negotiate(&[0, 8, 101], PT_PCMA), Some(PT_PCMA));
        assert_eq!(negotiate(&[8, 0], PT_PCMU), Some(PT_PCMU));
        assert_eq!(
            negotiate(&[18, 0, 101], PT_PCMA),
            Some(PT_PCMU),
            "偏好的沒有提供 → offer 中第一個 G.711"
        );
        assert_eq!(negotiate(&[18, 96], PT_PCMA), None, "沒有 G.711 → 488");
        assert_eq!(
            negotiate(&[], PT_PCMU),
            Some(PT_PCMU),
            "沒有 SDP → 沿用偏好的"
        );
    }

    #[test]
    fn offerer_follows_answer() {
        assert_eq!(answered(&[0, 101], PT_PCMA), PT_PCMU);
        assert_eq!(answered(&[101, 8], PT_PCMU), PT_PCMA);
        assert_eq!(answered(&[], PT_PCMU), PT_PCMU);
        assert_eq!(answered(&[18], PT_PCMA), PT_PCMA);
    }

    #[test]
    fn builds_offer_and_answer_bodies() {
        assert_eq!(offer_payloads(PT_PCMU), vec![0, 8, 101]);
        assert_eq!(offer_payloads(PT_PCMA), vec![8, 0, 101]);
        assert_eq!(answer_payloads(PT_PCMU, &[0, 8, 101]), vec![0, 101]);
        assert_eq!(answer_payloads(PT_PCMA, &[8]), vec![8]);

        let b = body("10.0.0.2", 16000, &[0, 101]);
        assert!(b.contains("m=audio 16000 RTP/AVP 0 101\r\n"));
        assert!(b.contains("a=rtpmap:0 PCMU/8000\r\n"));
        assert!(b.contains("a=rtpmap:101 telephone-event/8000\r\n"));
        assert!(!b.contains("PCMA"), "answer 只宣告選中的 codec");
        assert_eq!(
            audio_payloads(&format!("SIP/2.0 200 OK\r\n\r\n{}", b)),
            vec![0, 101]
        );
    }
}
