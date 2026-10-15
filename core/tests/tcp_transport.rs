/// TCP 傳輸：民眾端與座席端都只用一條 TCP 連線完成整通電話，訊息以 Content-Length 分框
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode, Transport};
use sipress_core::engine::Engine;
use sipress_core::sip::{SipMessage, SipParser};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::TcpListener;

type Log = Arc<Mutex<Vec<String>>>;

/// 從連線讀出下一則 SIP 訊息（測試用的簡易分框）
async fn read_msg(r: &mut OwnedReadHalf, buf: &mut Vec<u8>) -> Option<String> {
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).into_owned();
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + len {
                let msg = String::from_utf8_lossy(&buf[..end + 4 + len]).into_owned();
                buf.drain(..end + 4 + len);
                return Some(msg);
            }
        }
        let mut chunk = [0u8; 4096];
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

fn logs_dir(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("sipress-tcp-{}-{}", name, std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn requests(log: &Log, method: &str) -> Vec<String> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|m| m.starts_with(&format!("{} ", method)))
        .cloned()
        .collect()
}

// ── 民眾端 ───────────────────────────────────────────────

#[tokio::test]
async fn caller_completes_calls_over_a_single_tcp_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let connections = Arc::new(Mutex::new(0usize));
    let (log_rx, conn_rx) = (Arc::clone(&log), Arc::clone(&connections));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            *conn_rx.lock().unwrap() += 1;
            let log = Arc::clone(&log_rx);
            tokio::spawn(async move {
                let (mut r, w) = stream.into_split();
                let w = Arc::new(tokio::sync::Mutex::new(w));
                let mut buf = Vec::new();
                while let Some(raw) = read_msg(&mut r, &mut buf).await {
                    log.lock().unwrap().push(raw.clone());
                    match raw.split_whitespace().next().unwrap_or("") {
                        "INVITE" => {
                            // 超過 T1 才回應：UDP 會重傳 INVITE，TCP 不可以
                            let w = Arc::clone(&w);
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(1200)).await;
                                let ringing =
                                    SipMessage::response_with_to_tag(&raw, "180 Ringing", "uas");
                                let ok = SipMessage::response_with_to_tag(&raw, "200 OK", "uas");
                                // 兩則回應黏在同一次寫入
                                let _ = w
                                    .lock()
                                    .await
                                    .write_all(format!("{}{}", ringing, ok).as_bytes())
                                    .await;
                            });
                        }
                        "BYE" => {
                            let ok = SipMessage::ok_for_server_bye(&raw);
                            let _ = w.lock().await.write_all(ok.as_bytes()).await;
                        }
                        _ => {}
                    }
                }
            });
        }
    });

    let cfg = Config {
        server_addr: server.to_string(),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs_dir("caller"),
        transport: Transport::Tcp,
        callee_fixed: Some("168168999".into()),
        max_total_calls: Some(2),
        cps: 5.0,
        duration_secs: 15,
        call_duration_secs: 1,
        invite_timeout_secs: 5,
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(20), Engine::new(cfg).run(None))
        .await
        .expect("民眾端應正常結束")
        .unwrap();

    assert_eq!(report.calls_answered, 2, "TCP 應可接通");
    assert_eq!(report.calls_failed, 0);
    assert_eq!(*connections.lock().unwrap(), 1, "整個測試只用一條 TCP 連線");

    let invites = requests(&log, "INVITE");
    assert_eq!(invites.len(), 2, "TCP 不做 SIP 層重傳");
    for inv in &invites {
        assert!(inv.contains("Via: SIP/2.0/TCP "), "{}", inv);
        assert!(inv.contains(";transport=tcp>"), "{}", inv);
    }
    let acks = requests(&log, "ACK");
    assert_eq!(acks.len(), 2);
    assert!(acks.iter().all(|m| m.contains("Via: SIP/2.0/TCP ")));
    let byes = requests(&log, "BYE");
    assert_eq!(byes.len(), 2);
    assert!(byes.iter().all(|m| m.contains("Via: SIP/2.0/TCP ")));
}

// ── 座席端 ───────────────────────────────────────────────

/// 伺服器主動送來的請求（INVITE / ACK / BYE）
fn server_request(
    method: &str,
    cseq: u32,
    to_tag: &str,
    addr: &str,
    agent: &str,
    body: &str,
) -> String {
    let ctype = if body.is_empty() {
        ""
    } else {
        "Content-Type: application/sdp\r\n"
    };
    let tag = if to_tag.is_empty() {
        String::new()
    } else {
        format!(";tag={}", to_tag)
    };
    format!(
        "{method} sip:1001@{agent};transport=tcp SIP/2.0\r\n\
         Via: SIP/2.0/TCP {addr};branch=z9hG4bK-{method}{cseq}\r\n\
         Max-Forwards: 70\r\nFrom: <sip:0912@{addr}>;tag=mock\r\nTo: <sip:1001@{agent}>{tag}\r\n\
         Call-ID: tcp-call@mock\r\nCSeq: {cseq} {method}\r\nContact: <sip:mock@{addr};transport=tcp>\r\n\
         {ctype}Content-Length: {len}\r\n\r\n{body}",
        len = body.len(),
    )
}

#[tokio::test]
async fn agent_registers_and_answers_over_its_tcp_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let log_rx = Arc::clone(&log);
    tokio::spawn(async move {
        let (stream, agent) = listener.accept().await.unwrap();
        let addr = server.to_string();
        let agent = agent.to_string();
        let (mut r, mut w) = stream.into_split();
        let mut buf = Vec::new();
        let mut started = false;
        while let Some(raw) = read_msg(&mut r, &mut buf).await {
            log_rx.lock().unwrap().push(raw.clone());
            if raw.starts_with("REGISTER") {
                let ok = SipMessage::ok_for_server_bye(&raw);
                let _ = w.write_all(ok.as_bytes()).await;
                if !std::mem::replace(&mut started, true) {
                    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 9 RTP/AVP 8\r\n";
                    let inv = server_request("INVITE", 1, "", &addr, &agent, sdp);
                    // keep-alive CRLF 夾在訊息前
                    let _ = w.write_all(format!("\r\n\r\n{}", inv).as_bytes()).await;
                }
            } else if raw.starts_with("SIP/2.0 200")
                && SipParser::cseq_method(&raw).as_deref() == Some("INVITE")
            {
                let tag = SipParser::to_tag(&raw).unwrap_or_default();
                let ack = server_request("ACK", 1, &tag, &addr, &agent, "");
                let _ = w.write_all(ack.as_bytes()).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                let bye = server_request("BYE", 2, &tag, &addr, &agent, "");
                let _ = w.write_all(bye.as_bytes()).await;
            }
        }
    });

    let cfg = Config {
        server_addr: server.to_string(),
        logs_dir: logs_dir("agent"),
        mode: Mode::Agent,
        transport: Transport::Tcp,
        duration_secs: 3,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username: "1001".into(),
            password: "pw".into(),
            domain: String::new(),
        }],
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(15), AgentEngine::new(cfg).run(None))
        .await
        .expect("座席引擎應正常結束")
        .unwrap();

    let msgs = log.lock().unwrap().clone();
    let reg = msgs
        .iter()
        .find(|m| m.starts_with("REGISTER"))
        .expect("座席應註冊");
    assert!(reg.contains("Via: SIP/2.0/TCP "), "{}", reg);
    assert!(
        reg.contains(";transport=tcp>"),
        "Contact 要標明 TCP，伺服器才會用這條連線找座席：\n{}",
        reg
    );

    let ok = msgs
        .iter()
        .find(|m| {
            SipParser::status_code(m) == Some(200)
                && SipParser::cseq_method(m).as_deref() == Some("INVITE")
        })
        .expect("座席應在同一條 TCP 連線上接聽");
    assert!(ok.contains(";transport=tcp>"), "{}", ok);
    assert!(
        msgs.iter().any(|m| SipParser::status_code(m) == Some(200)
            && SipParser::cseq_method(m).as_deref() == Some("BYE")),
        "座席應回應 BYE"
    );
    assert_eq!(report.calls_answered, 1);
}

#[tokio::test]
async fn tcp_connection_refused_fails_fast() {
    // 取一個沒人監聽的 port
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cfg = Config {
        server_addr: format!("127.0.0.1:{}", port),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs_dir("refused"),
        transport: Transport::Tcp,
        duration_secs: 1,
        ..Default::default()
    };
    let err = tokio::time::timeout(Duration::from_secs(10), Engine::new(cfg).run(None))
        .await
        .expect("連線失敗應立即回報")
        .unwrap_err();
    assert!(format!("{:#}", err).contains("TCP"), "{:#}", err);
}
