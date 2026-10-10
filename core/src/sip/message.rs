/// SIP 訊息建構器
/// 手刻 SIP RFC 3261 格式，不依賴外部複雜 crate，靜態編譯友好
use uuid::Uuid;

pub struct SipMessage;

impl SipMessage {
    /// 建構 INVITE 請求
    pub fn invite(
        call_id:      &str,
        from_number:  &str,
        from_domain:  &str,
        to_number:    &str,
        server_addr:  &str,
        local_addr:   &str,
        cseq:         u32,
        branch:       &str,
        tag:          &str,
        transport:    &str,   // "UDP" or "TCP"
        rtp_port:     u16,    // 本機 RTP port（寫入 SDP m= 行）
    ) -> String {
        let sdp = Self::minimal_sdp(local_addr, rtp_port);
        let sdp_len = sdp.len();

        format!(
            "INVITE sip:{to}@{server} SIP/2.0\r\n\
             Via: SIP/2.0/{transport} {local};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             From: <sip:{from}@{from_domain}>;tag={tag}\r\n\
             To: <sip:{to}@{server}>\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} INVITE\r\n\
             Contact: <sip:{from}@{local};transport={transport_lower}>\r\n\
             Content-Type: application/sdp\r\n\
             Content-Length: {sdp_len}\r\n\
             User-Agent: sipress/0.1\r\n\
             Allow: INVITE,ACK,BYE,CANCEL,OPTIONS\r\n\
             \r\n\
             {sdp}",
            to           = to_number,
            server       = server_addr,
            transport    = transport,
            transport_lower = transport.to_lowercase(),
            local        = local_addr,
            branch       = branch,
            from         = from_number,
            from_domain  = from_domain,
            tag          = tag,
            call_id      = call_id,
            cseq         = cseq,
            sdp_len      = sdp_len,
            sdp          = sdp,
        )
    }

    /// 建構 ACK（收到 200 OK 後送出）
    ///
    /// `request_uri`: 2xx ACK 應使用 Contact URI（RFC 3261 §13.2.2.4）；
    ///                non-2xx ACK 傳 None，回退使用 sip:{to}@{server}。
    pub fn ack(
        call_id:     &str,
        from_number: &str,
        from_domain: &str,
        to_number:   &str,
        to_tag:      &str,
        server_addr: &str,
        local_addr:  &str,
        cseq:        u32,
        branch:      &str,
        from_tag:    &str,
        transport:   &str,
        request_uri: Option<&str>,
    ) -> String {
        let uri = request_uri
            .map(|u| u.to_string())
            .unwrap_or_else(|| format!("sip:{}@{}", to_number, server_addr));
        format!(
            "ACK {uri} SIP/2.0\r\n\
             Via: SIP/2.0/{transport} {local};branch={branch}\r\n\
             Max-Forwards: 70\r\n\
             From: <sip:{from}@{from_domain}>;tag={from_tag}\r\n\
             To: <sip:{to}@{server}>;tag={to_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} ACK\r\n\
             Content-Length: 0\r\n\
             \r\n",
            uri        = uri,
            to         = to_number,
            server     = server_addr,
            transport  = transport,
            local      = local_addr,
            branch     = branch,
            from       = from_number,
            from_domain = from_domain,
            from_tag   = from_tag,
            to_tag     = to_tag,
            call_id    = call_id,
            cseq       = cseq,
        )
    }

    /// 建構 BYE
    ///
    /// `request_uri`: 應傳入 200 OK Contact URI（RFC 3261 §12.2.1.1）；
    ///                None 時回退使用 sip:{to}@{server}。
    pub fn bye(
        call_id:     &str,
        from_number: &str,
        from_domain: &str,
        to_number:   &str,
        to_tag:      &str,
        server_addr: &str,
        local_addr:  &str,
        cseq:        u32,
        branch:      &str,
        from_tag:    &str,
        transport:   &str,
        request_uri: Option<&str>,
    ) -> String {
        let uri = request_uri
            .map(|u| u.to_string())
            .unwrap_or_else(|| format!("sip:{}@{}", to_number, server_addr));
        format!(
            "BYE {uri} SIP/2.0\r\n\
             Via: SIP/2.0/{transport} {local};branch={branch}\r\n\
             Max-Forwards: 70\r\n\
             From: <sip:{from}@{from_domain}>;tag={from_tag}\r\n\
             To: <sip:{to}@{server}>;tag={to_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} BYE\r\n\
             Content-Length: 0\r\n\
             \r\n",
            uri         = uri,
            to          = to_number,
            server      = server_addr,
            transport   = transport,
            local       = local_addr,
            branch      = branch,
            from        = from_number,
            from_domain = from_domain,
            from_tag    = from_tag,
            to_tag      = to_tag,
            call_id     = call_id,
            cseq        = cseq,
        )
    }

    /// 建構 CANCEL
    pub fn cancel(
        call_id:     &str,
        from_number: &str,
        from_domain: &str,
        to_number:   &str,
        server_addr: &str,
        local_addr:  &str,
        cseq:        u32,
        branch:      &str,
        from_tag:    &str,
        transport:   &str,
    ) -> String {
        format!(
            "CANCEL sip:{to}@{server} SIP/2.0\r\n\
             Via: SIP/2.0/{transport} {local};branch={branch}\r\n\
             Max-Forwards: 70\r\n\
             From: <sip:{from}@{from_domain}>;tag={from_tag}\r\n\
             To: <sip:{to}@{server}>\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} CANCEL\r\n\
             Content-Length: 0\r\n\
             \r\n",
            to          = to_number,
            server      = server_addr,
            transport   = transport,
            local       = local_addr,
            branch      = branch,
            from        = from_number,
            from_domain = from_domain,
            from_tag    = from_tag,
            call_id     = call_id,
            cseq        = cseq,
        )
    }

    /// 最小 SDP
    /// 固定使用 G.711A（PCMA，PT=8）作為媒體 codec，
    /// 伺服器通常以 offer 中第一個 codec 為主，明確指定避免協商錯誤。
    fn minimal_sdp(local_ip: &str, rtp_port: u16) -> String {
        let ip = local_ip.split(':').next().unwrap_or(local_ip);
        format!(
            "v=0\r\n\
             o=sipress 1000 1000 IN IP4 {ip}\r\n\
             s=sipress\r\n\
             c=IN IP4 {ip}\r\n\
             t=0 0\r\n\
             m=audio {port} RTP/AVP 8\r\n\
             a=rtpmap:8 PCMA/8000\r\n\
             a=ptime:20\r\n\
             a=sendrecv\r\n",
            ip = ip,
            port = rtp_port,
        )
    }

    /// 產生 branch 參數（RFC 3261 要求以 z9hG4bK 開頭）
    pub fn new_branch() -> String {
        format!("z9hG4bK-{}", Uuid::new_v4().simple())
    }

    /// 產生 tag
    pub fn new_tag() -> String {
        Uuid::new_v4().simple().to_string()[..8].to_string()
    }

    /// 產生 Call-ID
    pub fn new_call_id(domain: &str) -> String {
        format!("{}@{}", Uuid::new_v4().simple(), domain)
    }
}

/// 建構對伺服器發來請求的 200 OK 回應（RE-INVITE / BYE）
impl SipMessage {
    /// 從請求訊息中擷取必要標頭（Via / From / To / Call-ID / CSeq）
    pub(crate) fn extract_request_headers(raw: &str) -> (String, String, String, String, String) {
        let (mut vias, mut from, mut to, mut call_id, mut cseq) =
            (Vec::<String>::new(), String::new(), String::new(), String::new(), String::new());
        for line in raw.lines() {
            let lower = line.to_lowercase();
            if lower.starts_with("via:") || lower.starts_with("v:") {
                vias.push(line.to_string());
            } else if from.is_empty() && (lower.starts_with("from:") || lower.starts_with("f:")) {
                from = line.to_string();
            } else if to.is_empty() && (lower.starts_with("to:") || lower.starts_with("t:")) {
                to = line.to_string();
            } else if call_id.is_empty() && (lower.starts_with("call-id:") || lower.starts_with("i:")) {
                call_id = line.to_string();
            } else if cseq.is_empty() && lower.starts_with("cseq:") {
                cseq = line.to_string();
            }
        }
        let via = vias.join("\r\n");
        (via, from, to, call_id, cseq)
    }

    /// 依 INVITE 的非 2xx 最終回應（3xx–6xx）建構 ACK（RFC 3261 §17.1.1.3）
    ///
    /// 屬於 INVITE client transaction，不需要 dialog 狀態：
    /// Via 沿用原 INVITE 的 top Via（同 branch）、To 帶回應的 tag、CSeq 號碼不變，
    /// Request-URI 與原 INVITE 相同（即 To URI）。
    pub fn ack_for_non2xx(raw_response: &str) -> String {
        let (via, from, to, call_id, cseq) = Self::extract_request_headers(raw_response);
        // 只保留 top Via，並移除伺服器加上的 received / rport 值
        let top_via = via.split("\r\n").next().unwrap_or("");
        let top_via: Vec<&str> = top_via.split(';')
            .filter(|p| !p.trim().to_lowercase().starts_with("received="))
            .map(|p| if p.trim().to_lowercase().starts_with("rport=") { "rport" } else { p })
            .collect();
        let to_value = to.split_once(':').map_or("", |(_, v)| v).trim();
        let request_uri = match (to_value.find('<'), to_value.find('>')) {
            (Some(s), Some(e)) if s < e => to_value[s + 1..e].to_string(),
            _ => to_value.split(';').next().unwrap_or("").trim().to_string(),
        };
        let cseq_num = cseq.split_once(':')
            .and_then(|(_, v)| v.split_whitespace().next())
            .unwrap_or("1");
        format!(
            "ACK {uri} SIP/2.0\r\n\
             {via}\r\n\
             Max-Forwards: 70\r\n\
             {from}\r\n\
             {to}\r\n\
             {call_id}\r\n\
             CSeq: {cseq} ACK\r\n\
             Content-Length: 0\r\n\
             \r\n",
            uri = request_uri, via = top_via.join(";"), from = from, to = to,
            call_id = call_id, cseq = cseq_num,
        )
    }

    /// 對伺服器發來的 BYE 回應 200 OK（不含 SDP）
    pub fn ok_for_server_bye(raw_request: &str) -> String {
        Self::response_for_request(raw_request, "200 OK")
    }

    /// 對伺服器發來的請求回應任意狀態（不含 body），鏡射 Via/From/To/Call-ID/CSeq
    /// `status_line` 例："481 Call/Transaction Does Not Exist"
    pub fn response_for_request(raw_request: &str, status_line: &str) -> String {
        Self::response_with_to_tag(raw_request, status_line, "")
    }

    /// 同 `response_for_request`，並在 To 尚無 tag 時加上本端 tag（UAS 對 INVITE 的 1xx/最終回應）
    pub fn response_with_to_tag(raw_request: &str, status_line: &str, to_tag: &str) -> String {
        let (via, from, to, call_id, cseq) = Self::extract_request_headers(raw_request);
        let to = inject_to_tag_if_missing(&to, to_tag);
        format!(
            "SIP/2.0 {status}\r\n\
             {via}\r\n\
             {from}\r\n\
             {to}\r\n\
             {call_id}\r\n\
             {cseq}\r\n\
             Content-Length: 0\r\n\
             \r\n",
            status = status_line,
            via = via, from = from, to = to, call_id = call_id, cseq = cseq,
        )
    }

    /// 對伺服器發來的 RE-INVITE 回應 200 OK（含 SDP，維持 PCMA 通話）
    ///
    /// `contact_user`: Contact URI 的 user 部分。INVITE 的 2xx 必須帶 Contact
    /// （RFC 3261 §13.3.1.4），對端以此作為後續請求（ACK/BYE）的目標。
    pub fn ok_for_server_reinvite(
        raw_request:  &str,
        local_addr:   &str,
        rtp_port:     u16,
        contact_user: &str,
    ) -> String {
        let (via, from, to, call_id, cseq) = Self::extract_request_headers(raw_request);
        let sdp     = Self::minimal_sdp(local_addr, rtp_port);
        let sdp_len = sdp.len();
        format!(
            "SIP/2.0 200 OK\r\n\
             {via}\r\n\
             {from}\r\n\
             {to}\r\n\
             {call_id}\r\n\
             {cseq}\r\n\
             Contact: <sip:{user}@{local};transport=udp>\r\n\
             Content-Type: application/sdp\r\n\
             Content-Length: {sdp_len}\r\n\
             \r\n\
             {sdp}",
            via     = via,
            from    = from,
            to      = to,
            call_id = call_id,
            cseq    = cseq,
            user    = contact_user,
            local   = local_addr,
            sdp_len = sdp_len,
            sdp     = sdp,
        )
    }
}

/// To 標頭尚無 tag 時附加 tag（tag 為空字串則原樣回傳）
pub(crate) fn inject_to_tag_if_missing(to_line: &str, tag: &str) -> String {
    if tag.is_empty() || to_line.to_lowercase().contains(";tag=") {
        to_line.to_string()
    } else {
        format!("{};tag={}", to_line.trim_end(), tag)
    }
}

/// SIP 回應 SDP 解析（一般標頭解析請用 `SipParser`）
pub struct SipResponse;

impl SipResponse {
    /// 從 200 OK 的 SDP body 中解析對端 RTP 地址（IP:port）
    /// 同時解析 c= connection line 與 m=audio port，回傳 "ip:port" 字串。
    /// 若 c= 不存在，以 fallback_ip（SIP server IP）代替。
    pub fn sdp_rtp_addr(raw: &str, fallback_ip: &str) -> Option<String> {
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4)
            .or_else(|| raw.find("\n\n").map(|i| i + 2))?;
        let body = &raw[body_start..];

        // 解析 c= line（例：c=IN IP4 192.168.1.10）
        let mut conn_ip = fallback_ip.to_string();
        // 先掃一輪拿 session-level c=
        for line in body.lines() {
            let line = line.trim();
            if line.starts_with("c=") {
                // c=IN IP4 <ip>  或  c=IN IP6 <ip>
                let parts: Vec<&str> = line.splitn(4, ' ').collect();
                if parts.len() >= 3 {
                    let ip = parts[2].trim().trim_end_matches('\r');
                    if !ip.is_empty() && ip != "0.0.0.0" {
                        conn_ip = ip.to_string();
                    }
                }
                break; // 取第一個 c= (session level)
            }
        }

        // 解析 m=audio port（取第一個 audio m= 行）
        let mut rtp_port: Option<u16> = None;
        let mut in_audio_section = false;
        for line in body.lines() {
            let line = line.trim();
            if line.starts_with("m=") {
                in_audio_section = line.starts_with("m=audio");
                if in_audio_section {
                    let parts: Vec<&str> = line.splitn(4, ' ').collect();
                    if parts.len() >= 2 {
                        if let Ok(p) = parts[1].parse::<u16>() {
                            rtp_port = Some(p);
                        }
                    }
                }
            }
            // media-level c= 覆蓋 session-level（取 audio section 內的 c=）
            if in_audio_section && line.starts_with("c=") {
                let parts: Vec<&str> = line.splitn(4, ' ').collect();
                if parts.len() >= 3 {
                    let ip = parts[2].trim().trim_end_matches('\r');
                    if !ip.is_empty() && ip != "0.0.0.0" {
                        conn_ip = ip.to_string();
                    }
                }
            }
            // 找到 audio port 後，繼續讀取直到遇到下一個 m= 行（或結束），
            // 確保 media-level c= 能覆蓋 session-level c=
            if rtp_port.is_some() && in_audio_section
                && !line.starts_with("m=audio") && line.starts_with("m=") {
                break;
            }
        }

        rtp_port.map(|p| format!("{}:{}", conn_ip, p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── SDP parsing ─────────────────────────────────────────────

    #[test]
    fn sdp_rtp_addr_session_level_c() {
        let raw = "SIP/2.0 200 OK\r\nContent-Length: 100\r\n\r\n\
                   v=0\r\n\
                   o=- 1 1 IN IP4 10.0.0.1\r\n\
                   s=-\r\n\
                   c=IN IP4 10.0.0.1\r\n\
                   t=0 0\r\n\
                   m=audio 20000 RTP/AVP 8\r\n\
                   a=rtpmap:8 PCMA/8000\r\n";
        let addr = SipResponse::sdp_rtp_addr(raw, "192.168.1.1");
        assert_eq!(addr, Some("10.0.0.1:20000".to_string()));
    }

    #[test]
    fn sdp_rtp_addr_media_level_c_overrides_session() {
        let raw = "SIP/2.0 200 OK\r\n\r\n\
                   v=0\r\n\
                   c=IN IP4 10.0.0.1\r\n\
                   t=0 0\r\n\
                   m=audio 20000 RTP/AVP 8\r\n\
                   c=IN IP4 172.16.0.5\r\n\
                   a=rtpmap:8 PCMA/8000\r\n";
        let addr = SipResponse::sdp_rtp_addr(raw, "192.168.1.1");
        assert_eq!(addr, Some("172.16.0.5:20000".to_string()));
    }

    #[test]
    fn sdp_rtp_addr_fallback_ip_when_no_c_line() {
        let raw = "SIP/2.0 200 OK\r\n\r\n\
                   v=0\r\n\
                   t=0 0\r\n\
                   m=audio 16000 RTP/AVP 8\r\n";
        let addr = SipResponse::sdp_rtp_addr(raw, "192.168.1.1");
        assert_eq!(addr, Some("192.168.1.1:16000".to_string()));
    }

    #[test]
    fn sdp_rtp_addr_zero_ip_uses_fallback() {
        let raw = "SIP/2.0 200 OK\r\n\r\n\
                   v=0\r\n\
                   c=IN IP4 0.0.0.0\r\n\
                   m=audio 16000 RTP/AVP 8\r\n";
        let addr = SipResponse::sdp_rtp_addr(raw, "10.1.1.1");
        assert_eq!(addr, Some("10.1.1.1:16000".to_string()));
    }

    // ─── SIP message construction ────────────────────────────────

    #[test]
    fn invite_contains_required_headers() {
        let msg = SipMessage::invite(
            "abc@test", "1000", "10.0.0.1:5060", "2001",
            "10.0.0.2:5060", "10.0.0.1:5070", 1,
            "z9hG4bK-test", "tag123", "UDP", 16000,
        );
        assert!(msg.starts_with("INVITE sip:2001@10.0.0.2:5060 SIP/2.0\r\n"));
        assert!(msg.contains("Call-ID: abc@test\r\n"));
        assert!(msg.contains("CSeq: 1 INVITE\r\n"));
        assert!(msg.contains("From: <sip:1000@10.0.0.1:5060>;tag=tag123\r\n"));
        assert!(msg.contains("To: <sip:2001@10.0.0.2:5060>\r\n"));
        assert!(msg.contains("m=audio 16000 RTP/AVP 8\r\n"));
        assert!(msg.contains("Content-Type: application/sdp\r\n"));
    }

    #[test]
    fn ack_uses_contact_uri_when_provided() {
        let msg = SipMessage::ack(
            "abc@test", "1000", "10.0.0.1", "2001", "srv-tag",
            "10.0.0.2:5060", "10.0.0.1:5070", 1,
            "z9hG4bK-ack", "tag123", "UDP",
            Some("sip:2001@10.0.0.2:5061"),
        );
        assert!(msg.starts_with("ACK sip:2001@10.0.0.2:5061 SIP/2.0\r\n"));
    }

    #[test]
    fn ack_falls_back_to_to_uri() {
        let msg = SipMessage::ack(
            "abc@test", "1000", "10.0.0.1", "2001", "srv-tag",
            "10.0.0.2:5060", "10.0.0.1:5070", 1,
            "z9hG4bK-ack", "tag123", "UDP", None,
        );
        assert!(msg.starts_with("ACK sip:2001@10.0.0.2:5060 SIP/2.0\r\n"));
    }

    #[test]
    fn bye_contains_correct_cseq() {
        let msg = SipMessage::bye(
            "abc@test", "1000", "10.0.0.1", "2001", "srv-tag",
            "10.0.0.2:5060", "10.0.0.1:5070", 2,
            "z9hG4bK-bye", "tag123", "UDP", None,
        );
        assert!(msg.contains("CSeq: 2 BYE\r\n"));
        assert!(msg.contains(";tag=srv-tag\r\n"));
    }

    #[test]
    fn cancel_preserves_branch() {
        let msg = SipMessage::cancel(
            "abc@test", "1000", "10.0.0.1", "2001",
            "10.0.0.2:5060", "10.0.0.1:5070", 1,
            "z9hG4bK-orig", "tag123", "UDP",
        );
        assert!(msg.starts_with("CANCEL sip:2001@10.0.0.2:5060 SIP/2.0\r\n"));
        assert!(msg.contains("branch=z9hG4bK-orig"));
        assert!(msg.contains("CSeq: 1 CANCEL\r\n"));
    }

    #[test]
    fn ok_for_server_bye_echoes_headers() {
        let bye_req = "BYE sip:1000@10.0.0.1:5070 SIP/2.0\r\n\
                       Via: SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bK-srv\r\n\
                       From: <sip:2001@10.0.0.2>;tag=srv-tag\r\n\
                       To: <sip:1000@10.0.0.1>;tag=my-tag\r\n\
                       Call-ID: test123@10.0.0.2\r\n\
                       CSeq: 5 BYE\r\n\
                       Content-Length: 0\r\n\
                       \r\n";
        let ok = SipMessage::ok_for_server_bye(bye_req);
        assert!(ok.starts_with("SIP/2.0 200 OK\r\n"));
        assert!(ok.contains("Call-ID: test123@10.0.0.2\r\n"));
        assert!(ok.contains("CSeq: 5 BYE\r\n"));
        assert!(ok.contains("tag=srv-tag"));
        assert!(ok.contains("tag=my-tag"));
    }

    #[test]
    fn ok_for_reinvite_includes_sdp() {
        let reinvite = "INVITE sip:1000@10.0.0.1:5070 SIP/2.0\r\n\
                        Via: SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bK-ri\r\n\
                        From: <sip:2001@10.0.0.2>;tag=srv-tag\r\n\
                        To: <sip:1000@10.0.0.1>;tag=my-tag\r\n\
                        Call-ID: test123@10.0.0.2\r\n\
                        CSeq: 3 INVITE\r\n\
                        \r\n";
        let ok = SipMessage::ok_for_server_reinvite(reinvite, "10.0.0.1:5070", 18000, "1000");
        assert!(ok.starts_with("SIP/2.0 200 OK\r\n"));
        assert!(ok.contains("Content-Type: application/sdp\r\n"));
        assert!(ok.contains("m=audio 18000 RTP/AVP 8\r\n"));
    }

    #[test]
    fn ok_for_reinvite_includes_contact() {
        let reinvite = "INVITE sip:1000@10.0.0.1:5070 SIP/2.0\r\n\
                        Via: SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bK-ri\r\n\
                        From: <sip:2001@10.0.0.2>;tag=srv-tag\r\n\
                        To: <sip:1000@10.0.0.1>;tag=my-tag\r\n\
                        Call-ID: test123@10.0.0.2\r\n\
                        CSeq: 3 INVITE\r\n\
                        \r\n";
        let ok = SipMessage::ok_for_server_reinvite(reinvite, "10.0.0.1:5070", 18000, "1000");
        let headers = &ok[..ok.find("\r\n\r\n").unwrap()];
        assert!(
            headers.contains("\r\nContact: <sip:1000@10.0.0.1:5070;transport=udp>\r\n"),
            "INVITE 的 2xx 必須帶 Contact：\n{}", headers
        );
    }

    // ─── non-2xx ACK ─────────────────────────────────────────────

    const RESP_487: &str = "SIP/2.0 487 Request Terminated\r\n\
        Via: SIP/2.0/UDP 10.0.0.1:5070;branch=z9hG4bK-inv1;rport=5070;received=1.2.3.4\r\n\
        From: <sip:1000@10.0.0.1>;tag=from1\r\n\
        To: <sip:2001@10.0.0.2:5060>;tag=srv9\r\n\
        Call-ID: abc@10.0.0.1\r\n\
        CSeq: 1 INVITE\r\n\
        Content-Length: 0\r\n\
        \r\n";

    #[test]
    fn ack_for_non2xx_mirrors_invite_transaction() {
        let ack = SipMessage::ack_for_non2xx(RESP_487);
        assert!(ack.starts_with("ACK sip:2001@10.0.0.2:5060 SIP/2.0\r\n"), "{}", ack);
        assert!(ack.contains("Via: SIP/2.0/UDP 10.0.0.1:5070;branch=z9hG4bK-inv1;rport\r\n"),
            "沿用 INVITE branch 並去掉 received / rport 值：{}", ack);
        assert!(ack.contains("From: <sip:1000@10.0.0.1>;tag=from1\r\n"));
        assert!(ack.contains("To: <sip:2001@10.0.0.2:5060>;tag=srv9\r\n"));
        assert!(ack.contains("Call-ID: abc@10.0.0.1\r\n"));
        assert!(ack.contains("CSeq: 1 ACK\r\n"));
        assert!(ack.ends_with("Content-Length: 0\r\n\r\n"));
    }

    #[test]
    fn ack_for_non2xx_handles_compact_headers() {
        let resp = "SIP/2.0 486 Busy Here\r\n\
            v: SIP/2.0/UDP 10.0.0.1:5070;branch=z9hG4bK-c\r\n\
            f: <sip:1000@10.0.0.1>;tag=f\r\n\
            t: sip:2001@10.0.0.2;tag=t\r\n\
            i: cid@x\r\n\
            CSeq: 7 INVITE\r\n\r\n";
        let ack = SipMessage::ack_for_non2xx(resp);
        assert!(ack.starts_with("ACK sip:2001@10.0.0.2 SIP/2.0\r\n"), "{}", ack);
        assert!(ack.contains("branch=z9hG4bK-c"));
        assert!(ack.contains("CSeq: 7 ACK\r\n"));
    }

    // ─── Unique ID generation ────────────────────────────────────

    #[test]
    fn branch_starts_with_magic_cookie() {
        let branch = SipMessage::new_branch();
        assert!(branch.starts_with("z9hG4bK-"), "branch must start with RFC 3261 magic cookie");
    }

    #[test]
    fn tag_is_8_chars() {
        let tag = SipMessage::new_tag();
        assert_eq!(tag.len(), 8);
    }

    #[test]
    fn call_id_contains_domain() {
        let cid = SipMessage::new_call_id("example.com");
        assert!(cid.ends_with("@example.com"));
    }
}
