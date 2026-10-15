/// 民眾端撥號認證：INVITE 收到 401 / 407 時帶 Digest 重送一次
use md5::{Digest, Md5};
use sipress_core::config::Config;
use sipress_core::engine::Engine;
use sipress_core::sip::{SipMessage, SipParser};
use sipress_core::stats::FinalReport;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

const REALM: &str = "sipx";
const NONCE: &str = "n0nce";
const PASSWORD: &str = "s3cret";

fn md5_hex(s: &str) -> String {
    format!("{:x}", Md5::digest(s.as_bytes()))
}

fn param(header: &str, name: &str) -> Option<String> {
    let start = header.find(&format!("{}=", name))? + name.len() + 1;
    let rest = header[start..].trim_start_matches('"');
    let end = rest.find(['"', ',']).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// 驗證 Digest：response = MD5(MD5(user:realm:pass):nonce:MD5(INVITE:uri))（無 qop）
fn digest_ok(header: &str) -> bool {
    let (Some(user), Some(uri), Some(resp)) = (
        param(header, "username"),
        param(header, "uri"),
        param(header, "response"),
    ) else {
        return false;
    };
    let ha1 = md5_hex(&format!("{}:{}:{}", user, REALM, PASSWORD));
    let ha2 = md5_hex(&format!("INVITE:{}", uri));
    resp == md5_hex(&format!("{}:{}:{}", ha1, NONCE, ha2))
}

/// mock UAS：沒有（或錯誤的）認證時回 401 / 407，正確時接聽；記錄收到的所有訊息
async fn spawn_uas(proxy: bool) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let log_rx = Arc::clone(&log);
    let (status, challenge_hdr, auth_hdr) = if proxy {
        (
            "407 Proxy Authentication Required",
            "Proxy-Authenticate",
            "proxy-authorization:",
        )
    } else {
        ("401 Unauthorized", "WWW-Authenticate", "authorization:")
    };
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            log_rx.lock().unwrap().push(raw.clone());
            let reply = match raw.split_whitespace().next().unwrap_or("") {
                "INVITE" => {
                    let auth = raw.lines().find(|l| l.to_lowercase().starts_with(auth_hdr));
                    if auth.is_some_and(digest_ok) {
                        let ok = SipMessage::response_with_to_tag(&raw, "200 OK", "uas");
                        vec![
                            SipMessage::response_with_to_tag(&raw, "180 Ringing", "uas"),
                            ok,
                        ]
                    } else {
                        let r = SipMessage::response_with_to_tag(&raw, status, "uas").replacen(
                            "Content-Length:",
                            &format!("{}: Digest realm=\"{}\", nonce=\"{}\", algorithm=MD5\r\nContent-Length:", challenge_hdr, REALM, NONCE),
                            1,
                        );
                        // 送兩次：模擬伺服器重傳（舊 transaction 的 401 不可讓已重送的通話失敗）
                        vec![r.clone(), r]
                    }
                }
                "BYE" | "CANCEL" => vec![SipMessage::ok_for_server_bye(&raw)],
                _ => vec![],
            };
            for r in reply {
                let _ = sock.send_to(r.as_bytes(), peer).await;
            }
        }
    });
    (addr, log)
}

async fn run(proxy: bool, cfg: impl FnOnce(Config) -> Config) -> (FinalReport, Vec<String>) {
    let (server, log) = spawn_uas(proxy).await;
    let logs = std::env::temp_dir().join(format!("sipress-invite-auth-{}", std::process::id()));
    let base = Config {
        server_addr: server.to_string(),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs.to_string_lossy().into_owned(),
        caller_number: "0912000000".into(),
        callee_fixed: Some("168168999".into()),
        max_total_calls: Some(2),
        cps: 5.0,
        duration_secs: 10,
        call_duration_secs: 1,
        invite_timeout_secs: 3,
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(20), Engine::new(cfg(base)).run(None))
        .await
        .expect("民眾端應正常結束")
        .unwrap();
    let msgs = log.lock().unwrap().clone();
    (report, msgs)
}

fn requests<'a>(msgs: &'a [String], method: &str) -> Vec<&'a String> {
    msgs.iter()
        .filter(|m| m.starts_with(&format!("{} ", method)))
        .collect()
}

fn header(msg: &str, name: &str) -> String {
    msg.lines()
        .find(|l| {
            l.to_lowercase()
                .starts_with(&format!("{}:", name.to_lowercase()))
        })
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn retries_invite_with_authorization_after_401() {
    let (report, msgs) = run(false, |c| Config {
        caller_auth_username: Some("trunk".into()),
        caller_auth_password: Some(PASSWORD.into()),
        ..c
    })
    .await;
    assert_eq!(report.calls_initiated, 2);
    assert_eq!(report.calls_answered, 2, "帶認證重送後應接通");
    assert_eq!(report.calls_failed, 0, "被 challenge 不算失敗");

    let invites = requests(&msgs, "INVITE");
    assert_eq!(invites.len(), 4, "每通：第一次 + 帶認證重送一次");
    for call_id in invites
        .iter()
        .map(|m| SipParser::call_id(m).unwrap())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let of_call: Vec<_> = invites
            .iter()
            .filter(|m| SipParser::call_id(m).as_deref() == Some(&call_id))
            .collect();
        assert_eq!(SipParser::cseq_number(of_call[0]), Some(1));
        assert_eq!(
            SipParser::cseq_number(of_call[1]),
            Some(2),
            "重送是新的 transaction（CSeq + 1）"
        );
        assert_ne!(
            SipParser::via(of_call[0]),
            SipParser::via(of_call[1]),
            "重送要用新的 branch"
        );
        assert_eq!(
            SipParser::from_tag(of_call[0]),
            SipParser::from_tag(of_call[1]),
            "同一個 dialog：From tag 不變"
        );
        let auth = header(of_call[1], "Authorization");
        assert!(auth.contains("username=\"trunk\""), "{}", auth);
        assert!(auth.contains("uri=\"sip:168168999@"), "{}", auth);

        // ACK（2xx）與 BYE 的 CSeq 接續重送後的 INVITE
        let bye = requests(&msgs, "BYE")
            .into_iter()
            .find(|m| SipParser::call_id(m).as_deref() == Some(&call_id))
            .unwrap();
        assert_eq!(SipParser::cseq_number(bye), Some(3));
    }
}

#[tokio::test]
async fn uses_proxy_authorization_after_407_and_caller_number_as_username() {
    let (report, msgs) = run(true, |c| Config {
        caller_auth_password: Some(PASSWORD.into()),
        caller_number_seq: true,
        ..c
    })
    .await;
    assert_eq!(report.calls_answered, 2);
    let retried: Vec<_> = requests(&msgs, "INVITE")
        .into_iter()
        .filter(|m| SipParser::cseq_number(m) == Some(2))
        .collect();
    assert_eq!(retried.len(), 2);
    for m in &retried {
        let auth = header(m, "Proxy-Authorization");
        let from_user = SipParser::from_user(m).unwrap();
        assert!(
            auth.contains(&format!("username=\"{}\"", from_user)),
            "帳號未填時用該通的主叫號碼：{}",
            auth
        );
    }
    // --from-seq：BYE 的 From 與該通 INVITE 相同
    for bye in requests(&msgs, "BYE") {
        let call_id = SipParser::call_id(bye).unwrap();
        let invite = retried
            .iter()
            .find(|m| SipParser::call_id(m).as_deref() == Some(&call_id))
            .unwrap();
        assert_eq!(SipParser::from_user(bye), SipParser::from_user(invite));
    }
}

#[tokio::test]
async fn without_password_a_challenge_fails_the_call() {
    let (report, msgs) = run(false, |c| c).await;
    assert_eq!(report.calls_answered, 0);
    assert_eq!(report.calls_failed, 2);
    assert_eq!(report.fail_4xx, 2);
    assert_eq!(requests(&msgs, "INVITE").len(), 2, "沒有密碼不重送");
}

#[tokio::test]
async fn wrong_password_retries_only_once() {
    let (report, msgs) = run(false, |c| Config {
        caller_auth_password: Some("wrong".into()),
        ..c
    })
    .await;
    assert_eq!(report.calls_answered, 0);
    assert_eq!(report.calls_failed, 2, "帶認證仍被拒 → 失敗");
    assert_eq!(
        requests(&msgs, "INVITE").len(),
        4,
        "每通只重送一次，不會無限重試"
    );
}
