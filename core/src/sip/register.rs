/// SIP REGISTER 訊息建構 + Digest auth (RFC 3261 §22.4 / RFC 2617)
use md5::{Digest, Md5};

/// REGISTER 訊息建構器
pub struct RegisterMessage;

impl RegisterMessage {
    /// 建構 REGISTER 請求
    /// auth_line: None = 第一次無認證；Some = 收到 401/407 後重送的完整認證標頭行
    ///            （由 `DigestChallenge::authorization_header` 產生）
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        username:    &str,
        server:      &str,           // 伺服器 ip:port (Request-URI + From/To domain)
        local_addr:  &str,           // 本機綁定 ip:port
        cseq:        u32,
        branch:      &str,
        from_tag:    &str,
        call_id:     &str,
        transport:   &str,           // "UDP" / "TCP"
        expires:     u32,            // 註冊有效秒數
        auth_line:   Option<&str>,   // 完整認證標頭行（Authorization / Proxy-Authorization）
    ) -> String {
        let auth_line = auth_line
            .map(|h| format!("{}\r\n", h))
            .unwrap_or_default();

        format!(
            "REGISTER sip:{server} SIP/2.0\r\n\
             Via: SIP/2.0/{tp} {local};rport;branch={branch}\r\n\
             From: <sip:{user}@{server}>;tag={tag}\r\n\
             To: <sip:{user}@{server}>\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} REGISTER\r\n\
             Contact: <sip:{user}@{local}>\r\n\
             Max-Forwards: 70\r\n\
             Expires: {expires}\r\n\
             {auth_line}\
             User-Agent: sipress/0.1\r\n\
             Allow: INVITE, INFO, PRACK, ACK, BYE, CANCEL, OPTIONS, NOTIFY, REGISTER, SUBSCRIBE, REFER, PUBLISH, UPDATE, MESSAGE\r\n\
             Allow-Events: talk,hold,conference,refer,check-sync\r\n\
             Content-Length: 0\r\n\
             \r\n",
            server    = server,
            tp        = transport,
            local     = local_addr,
            branch    = branch,
            user      = username,
            tag       = from_tag,
            call_id   = call_id,
            cseq      = cseq,
            expires   = expires,
            auth_line = auth_line,
        )
    }
}

// ─── Digest 認證質詢解析 ──────────────────────────────────────────

/// 從 401 / 407 回應解析的 Digest challenge
#[derive(Debug, Clone)]
pub struct DigestChallenge {
    pub realm:     String,
    pub nonce:     String,
    pub algorithm: String,           // 通常 "MD5"
    pub qop:       Option<String>,   // "auth" / "auth-int" / None
    pub opaque:    Option<String>,
    /// true = 來自 407 Proxy-Authenticate，回應須用 Proxy-Authorization
    pub proxy:     bool,
}

impl DigestChallenge {
    /// 從原始 401/407 回應解析
    pub fn parse(raw: &str) -> Option<Self> {
        // 找 WWW-Authenticate 或 Proxy-Authenticate
        let mut auth_value = String::new();
        let mut joining = false;
        let mut proxy = false;
        for line in raw.lines() {
            let lower = line.to_lowercase();
            if lower.starts_with("www-authenticate:") || lower.starts_with("proxy-authenticate:") {
                let val = line.split_once(':')?.1.trim();
                auth_value = val.to_string();
                joining = true;
                proxy = lower.starts_with("proxy-authenticate:");
            } else if joining && (line.starts_with(' ') || line.starts_with('\t')) {
                // 多行延續
                auth_value.push(' ');
                auth_value.push_str(line.trim());
            } else if joining {
                break;
            }
        }
        if auth_value.is_empty() {
            return None;
        }

        // 移除 "Digest" 前綴
        let after_digest = auth_value
            .strip_prefix("Digest")
            .or_else(|| auth_value.strip_prefix("digest"))?
            .trim();

        let mut realm = String::new();
        let mut nonce = String::new();
        let mut algorithm = "MD5".to_string();
        let mut qop = None;
        let mut opaque = None;

        for part in split_quoted_csv(after_digest) {
            let mut kv = part.splitn(2, '=');
            let k = kv.next().map(str::trim).unwrap_or("");
            let v_raw = kv.next().map(str::trim).unwrap_or("");
            let v = v_raw.trim_matches('"');
            match k.to_lowercase().as_str() {
                "realm"     => realm = v.to_string(),
                "nonce"     => nonce = v.to_string(),
                "algorithm" => algorithm = v.to_string(),
                "qop"       => qop = Some(v.to_string()),
                "opaque"    => opaque = Some(v.to_string()),
                _ => {}
            }
        }

        if realm.is_empty() || nonce.is_empty() {
            return None;
        }
        Some(Self { realm, nonce, algorithm, qop, opaque, proxy })
    }

    /// 對應 challenge 來源的認證標頭名稱（RFC 3261 §22.3：407 → Proxy-Authorization）
    pub fn header_name(&self) -> &'static str {
        if self.proxy { "Proxy-Authorization" } else { "Authorization" }
    }

    /// 建構完整認證標頭行（"Authorization: Digest ..." 或 "Proxy-Authorization: Digest ..."）
    pub fn authorization_header(
        &self,
        username: &str,
        password: &str,
        method:   &str,
        uri:      &str,
    ) -> String {
        format!("{}: {}", self.header_name(), self.build_authorization(username, password, method, uri))
    }

    /// 計算 response 並建構 Authorization header value
    pub fn build_authorization(
        &self,
        username: &str,
        password: &str,
        method:   &str,        // 通常 "REGISTER"
        uri:      &str,        // 例 "sip:server-ip:5060"
    ) -> String {
        let ha1 = md5_hex(&format!("{}:{}:{}", username, self.realm, password));
        let ha2 = md5_hex(&format!("{}:{}", method, uri));

        let (response, qop_part, nc_part, cnonce_part) = if let Some(qop_full) = &self.qop {
            // 取 qop 第一個值（"auth, auth-int" → "auth"）
            let qop_val = qop_full.split(',').next().unwrap_or("auth").trim();
            let nc = "00000001";
            let cnonce = format!("{:08x}", rand::random::<u32>());
            let resp = md5_hex(&format!(
                "{}:{}:{}:{}:{}:{}",
                ha1, self.nonce, nc, cnonce, qop_val, ha2
            ));
            (
                resp,
                format!(", qop={}", qop_val),
                format!(", nc={}", nc),
                format!(", cnonce=\"{}\"", cnonce),
            )
        } else {
            // RFC 2069 fallback：response = MD5(HA1:nonce:HA2)
            let resp = md5_hex(&format!("{}:{}:{}", ha1, self.nonce, ha2));
            (resp, String::new(), String::new(), String::new())
        };

        let opaque_part = self
            .opaque
            .as_ref()
            .map(|o| format!(", opaque=\"{}\"", o))
            .unwrap_or_default();

        format!(
            "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", \
             uri=\"{uri}\", response=\"{resp}\", algorithm={algo}{qop}{nc}{cnonce}{op}",
            user   = username,
            realm  = self.realm,
            nonce  = self.nonce,
            uri    = uri,
            resp   = response,
            algo   = self.algorithm,
            qop    = qop_part,
            nc     = nc_part,
            cnonce = cnonce_part,
            op     = opaque_part,
        )
    }
}

// ─── helpers ─────────────────────────────────────────────────────

fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(32);
    for b in out.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 拆 "key=val, key=\"val,with,comma\"" 為 Vec<String>
/// 引號內的逗號不視為分隔符
fn split_quoted_csv(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            ',' if !in_quote => {
                let t = cur.trim().to_string();
                if !t.is_empty() {
                    result.push(t);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        result.push(t);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_digest_challenge() {
        let raw = "SIP/2.0 401 Unauthorized\r\n\
                   Via: SIP/2.0/UDP 1.2.3.4:5060\r\n\
                   WWW-Authenticate: Digest realm=\"sip.example.com\", \
                   nonce=\"abc123\", algorithm=MD5, qop=\"auth\"\r\n\
                   \r\n";
        let c = DigestChallenge::parse(raw).expect("should parse");
        assert_eq!(c.realm, "sip.example.com");
        assert_eq!(c.nonce, "abc123");
        assert_eq!(c.algorithm, "MD5");
        assert_eq!(c.qop.as_deref(), Some("auth"));
    }

    #[test]
    fn www_authenticate_uses_authorization_header() {
        let raw = "SIP/2.0 401 Unauthorized\r\n\
                   WWW-Authenticate: Digest realm=\"r\", nonce=\"n\"\r\n\r\n";
        let c = DigestChallenge::parse(raw).unwrap();
        assert!(!c.proxy);
        assert!(c.authorization_header("u", "p", "REGISTER", "sip:x")
            .starts_with("Authorization: Digest "));
    }

    #[test]
    fn proxy_authenticate_uses_proxy_authorization_header() {
        let raw = "SIP/2.0 407 Proxy Authentication Required\r\n\
                   Proxy-Authenticate: Digest realm=\"r\", nonce=\"n\"\r\n\r\n";
        let c = DigestChallenge::parse(raw).unwrap();
        assert!(c.proxy);
        assert!(c.authorization_header("u", "p", "REGISTER", "sip:x")
            .starts_with("Proxy-Authorization: Digest "));
    }

    #[test]
    fn build_auth_response() {
        let c = DigestChallenge {
            realm:     "sip.example.com".into(),
            nonce:     "abc123".into(),
            algorithm: "MD5".into(),
            qop:       None,
            opaque:    None,
            proxy:     false,
        };
        let h = c.build_authorization("alice", "secret", "REGISTER", "sip:1.2.3.4:5060");
        assert!(h.contains("username=\"alice\""));
        assert!(h.contains("response=\""));
    }

    #[test]
    fn register_build_uses_server_as_domain() {
        let msg = RegisterMessage::build(
            "1001", "10.0.0.1:5066", "192.168.1.10:5070",
            1, "z9hG4bK-test", "tag123", "call-1@test",
            "UDP", 240, None,
        );
        assert!(msg.starts_with("REGISTER sip:10.0.0.1:5066 SIP/2.0\r\n"));
        assert!(msg.contains("From: <sip:1001@10.0.0.1:5066>;tag=tag123"));
        assert!(msg.contains("To: <sip:1001@10.0.0.1:5066>"));
        assert!(msg.contains("Contact: <sip:1001@192.168.1.10:5070>"));
        assert!(msg.contains("Expires: 240\r\n"));
    }

    #[test]
    fn register_build_includes_allow_events() {
        let msg = RegisterMessage::build(
            "1001", "10.0.0.1:5066", "192.168.1.10:5070",
            1, "z9hG4bK-test", "tag123", "call-1@test",
            "UDP", 240, None,
        );
        assert!(msg.contains("Allow-Events: talk,hold,conference,refer,check-sync"));
        assert!(msg.contains("Allow: INVITE, INFO, PRACK, ACK, BYE, CANCEL, OPTIONS, NOTIFY, REGISTER, SUBSCRIBE, REFER, PUBLISH, UPDATE, MESSAGE"));
    }

    #[test]
    fn register_build_with_auth() {
        let msg = RegisterMessage::build(
            "1001", "10.0.0.1:5066", "192.168.1.10:5070",
            2, "z9hG4bK-auth", "tag123", "call-1@test",
            "UDP", 240, Some("Authorization: Digest username=\"1001\", response=\"abc\""),
        );
        assert!(msg.contains("Authorization: Digest username=\"1001\", response=\"abc\""));
        assert!(msg.contains("CSeq: 2 REGISTER\r\n"));
    }

    #[test]
    fn register_deregister_expires_zero() {
        let msg = RegisterMessage::build(
            "1001", "10.0.0.1:5066", "192.168.1.10:5070",
            3, "z9hG4bK-dereg", "tag123", "call-1@test",
            "UDP", 0, None,
        );
        assert!(msg.contains("Expires: 0\r\n"));
    }
}
