//! SIP 回應解析器
//! 處理常見 header 縮寫（t: = To:, v: = Via: 等）

pub struct SipParser;

impl SipParser {
    /// 取得第一行的 SIP 狀態碼（SIP/2.0 200 OK）
    pub fn status_code(raw: &str) -> Option<u16> {
        let line = raw.lines().next()?;
        let mut parts = line.splitn(3, ' ');
        parts.next()?; // "SIP/2.0"
        parts.next()?.parse().ok()
    }

    /// 取得 Call-ID header 值
    pub fn call_id(raw: &str) -> Option<String> {
        Self::header_value(raw, &["call-id", "i"])
    }

    /// 取得 To header 的 tag 參數
    pub fn to_tag(raw: &str) -> Option<String> {
        let val = Self::header_value(raw, &["to", "t"])?;
        Self::extract_param(&val, "tag")
    }

    /// 取得 From header 的 tag 參數
    pub fn from_tag(raw: &str) -> Option<String> {
        let val = Self::header_value(raw, &["from", "f"])?;
        Self::extract_param(&val, "tag")
    }

    /// 取得 CSeq 的 method 部分（"1 INVITE" → "INVITE"）
    pub fn cseq_method(raw: &str) -> Option<String> {
        let val = Self::header_value(raw, &["cseq"])?;
        val.split_whitespace().nth(1).map(|s| s.to_uppercase())
    }

    /// 取得 CSeq 的序號
    pub fn cseq_number(raw: &str) -> Option<u32> {
        let val = Self::header_value(raw, &["cseq"])?;
        val.split_whitespace().next()?.parse().ok()
    }

    /// 取得 Via header（第一個）
    pub fn via(raw: &str) -> Option<String> {
        Self::header_value(raw, &["via", "v"])
    }

    /// 取得 Contact header
    pub fn contact(raw: &str) -> Option<String> {
        Self::header_value(raw, &["contact", "m"])
    }

    /// 從 Contact header 取出 SIP URI（剝除角括號與參數）
    pub fn contact_uri(raw: &str) -> Option<String> {
        let val = Self::header_value(raw, &["contact", "m"])?;
        // <sip:user@host:port;params> → sip:user@host:port
        if let Some(start) = val.find('<') {
            if let Some(end_rel) = val[start..].find('>') {
                return Some(val[start + 1..start + end_rel].to_string());
            }
        }
        // Plain URI（無角括號），去掉 ;params
        Some(val.split(';').next().unwrap_or(&val).trim().to_string())
    }

    /// 取得註冊有效秒數：優先 Expires 標頭，其次 Contact 的 ;expires= 參數
    pub fn expires(raw: &str) -> Option<u32> {
        if let Some(n) = Self::header_value(raw, &["expires"]).and_then(|v| v.parse().ok()) {
            return Some(n);
        }
        let contact = Self::header_value(raw, &["contact", "m"])?;
        let idx = contact.to_lowercase().find("expires=")?;
        let digits: String = contact[idx + 8..].chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    }

    /// 取得 Content-Length
    pub fn content_length(raw: &str) -> Option<usize> {
        let val = Self::header_value(raw, &["content-length", "l"])?;
        val.trim().parse().ok()
    }

    /// 取得 Reason-Phrase（200 OK 中的 "OK" 部分）
    pub fn reason_phrase(raw: &str) -> Option<String> {
        let line = raw.lines().next()?;
        let mut parts = line.splitn(3, ' ');
        parts.next()?;
        parts.next()?;
        Some(parts.next().unwrap_or("").trim().to_string())
    }

    // ── 內部輔助 ─────────────────────────────────────────────────

    /// 查找 header（支援多個別名），回傳 colon 後的值（已 trim）
    fn header_value(raw: &str, names: &[&str]) -> Option<String> {
        for line in raw.lines() {
            if line.is_empty() { break; } // header 區塊結束
            let lower = line.to_lowercase();
            for name in names {
                let prefix = format!("{}:", name);
                if lower.starts_with(&prefix) {
                    let val = line[prefix.len()..].trim().to_string();
                    return Some(val);
                }
            }
        }
        None
    }

    /// 從 header 值中取得 ;param=value
    fn extract_param(value: &str, param: &str) -> Option<String> {
        let needle = format!(";{}=", param.to_lowercase());
        let lower  = value.to_lowercase();
        let pos    = lower.find(&needle)?;
        let start  = pos + needle.len();
        let rest   = &value[start..];
        let end    = rest.find(';').unwrap_or(rest.len());
        Some(rest[..end].trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE_200: &str = "\
SIP/2.0 200 OK\r\n\
Via: SIP/2.0/UDP 192.168.1.10:5060;branch=z9hG4bK-abc\r\n\
From: <sip:1000@192.168.1.10>;tag=aabbccdd\r\n\
To: <sip:2001@192.168.1.100>;tag=server123\r\n\
Call-ID: abc123@192.168.1.10\r\n\
CSeq: 1 INVITE\r\n\
Contact: <sip:2001@192.168.1.100>\r\n\
Content-Length: 0\r\n\
\r\n";

    #[test]
    fn test_status_code() {
        assert_eq!(SipParser::status_code(RESPONSE_200), Some(200));
    }

    #[test]
    fn test_call_id() {
        assert_eq!(
            SipParser::call_id(RESPONSE_200),
            Some("abc123@192.168.1.10".to_string())
        );
    }

    #[test]
    fn test_to_tag() {
        assert_eq!(
            SipParser::to_tag(RESPONSE_200),
            Some("server123".to_string())
        );
    }

    #[test]
    fn test_from_tag() {
        assert_eq!(
            SipParser::from_tag(RESPONSE_200),
            Some("aabbccdd".to_string())
        );
    }

    #[test]
    fn test_expires_prefers_header_then_contact_param() {
        let both = "SIP/2.0 200 OK\r\nContact: <sip:a@b>;expires=60\r\nExpires: 120\r\n\r\n";
        assert_eq!(SipParser::expires(both), Some(120));
        let contact_only = "SIP/2.0 200 OK\r\nContact: <sip:a@b>;Expires=300;q=1\r\n\r\n";
        assert_eq!(SipParser::expires(contact_only), Some(300));
        assert_eq!(SipParser::expires(RESPONSE_200), None);
    }

    #[test]
    fn test_cseq_method() {
        assert_eq!(
            SipParser::cseq_method(RESPONSE_200),
            Some("INVITE".to_string())
        );
    }
}
