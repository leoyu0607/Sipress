/// codec 協商：SDP offer / answer 與實際送出的 RTP payload type、編碼一致
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Codec, Config, Mode};
use sipress_core::engine::Engine;
use sipress_core::sip::{SipMessage, SipParser};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

/// 收集 RTP 封包（payload type, payload）
type Packets = Arc<Mutex<Vec<(u8, Vec<u8>)>>>;

async fn rtp_sink() -> (UdpSocket, u16, Packets) {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = sock.local_addr().unwrap().port();
    (sock, port, Arc::default())
}

fn spawn_rtp_collector(sock: UdpSocket, packets: Packets) {
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, _)) = sock.recv_from(&mut buf).await {
            if n > 12 {
                packets
                    .lock()
                    .unwrap()
                    .push((buf[1] & 0x7f, buf[12..n].to_vec()));
            }
        }
    });
}

fn sdp(port: u16, fmts: &str) -> String {
    format!(
        "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {port} RTP/AVP {fmts}\r\n"
    )
}

// ── 座席端：依 INVITE 的 offer 選 codec ─────────────────────────

/// mock SIPX：座席註冊後送一通 INVITE（offer 由參數指定），收到 200 OK 回 ACK，1.5 秒後 BYE
async fn agent_call(offer_fmts: &'static str) -> (Vec<String>, Packets) {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    let (rtp, rtp_port, packets) = rtp_sink().await;
    spawn_rtp_collector(rtp, Arc::clone(&packets));
    let received: Arc<Mutex<Vec<String>>> = Arc::default();
    let rx = Arc::clone(&received);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let mut started = false;
        while let Ok((n, agent)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            rx.lock().unwrap().push(raw.clone());
            let req = |method: &str, cseq: u32, to_tag: &str, body: &str| {
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
                    "{method} sip:1001@{agent} SIP/2.0\r\nVia: SIP/2.0/UDP {addr};branch=z9hG4bK-{method}{cseq}\r\n\
                     Max-Forwards: 70\r\nFrom: <sip:0912@{addr}>;tag=mock\r\nTo: <sip:1001@{agent}>{tag}\r\n\
                     Call-ID: codec-call@mock\r\nCSeq: {cseq} {method}\r\nContact: <sip:mock@{addr}>\r\n\
                     {ctype}Content-Length: {len}\r\n\r\n{body}",
                    len = body.len(),
                )
            };
            if raw.starts_with("REGISTER") {
                let _ = sock
                    .send_to(SipMessage::ok_for_server_bye(&raw).as_bytes(), agent)
                    .await;
                if !std::mem::replace(&mut started, true) {
                    let inv = req("INVITE", 1, "", &sdp(rtp_port, offer_fmts));
                    let _ = sock.send_to(inv.as_bytes(), agent).await;
                }
            } else if raw.starts_with("SIP/2.0 200")
                && SipParser::cseq_method(&raw).as_deref() == Some("INVITE")
            {
                let tag = SipParser::to_tag(&raw).unwrap_or_default();
                let _ = sock
                    .send_to(req("ACK", 1, &tag, "").as_bytes(), agent)
                    .await;
                let sock = Arc::clone(&sock);
                let bye = req("BYE", 2, &tag, "");
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let _ = sock.send_to(bye.as_bytes(), agent).await;
                });
            }
        }
    });

    let logs = std::env::temp_dir().join(format!("sipress-codec-agent-{}", std::process::id()));
    let cfg = Config {
        server_addr: addr.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        mode: Mode::Agent,
        duration_secs: 3,
        enable_rtp: true,
        rtp_base_port: 41000,
        codec: Codec::Pcma,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username: "1001".into(),
            password: "pw".into(),
            domain: String::new(),
        }],
        ..Default::default()
    };
    tokio::time::timeout(Duration::from_secs(15), AgentEngine::new(cfg).run(None))
        .await
        .expect("座席引擎應正常結束")
        .unwrap();
    let msgs = received.lock().unwrap().clone();
    (msgs, packets)
}

fn invite_response(msgs: &[String], code: u16) -> Option<String> {
    msgs.iter()
        .find(|m| {
            SipParser::status_code(m) == Some(code)
                && SipParser::cseq_method(m).as_deref() == Some("INVITE")
        })
        .cloned()
}

#[tokio::test]
async fn agent_answers_pcmu_offer_with_pcmu_and_sends_ulaw() {
    // 設定偏好 PCMA，但 offer 只有 PCMU（+ telephone-event）
    let (msgs, packets) = agent_call("0 101").await;
    let ok = invite_response(&msgs, 200).expect("座席應接聽");
    assert!(ok.contains("m=audio"), "{}", ok);
    assert!(
        ok.contains(" RTP/AVP 0 101\r\n"),
        "answer 只能宣告 offer 中的 codec：\n{}",
        ok
    );
    assert!(!ok.contains("PCMA"), "{}", ok);

    let packets = packets.lock().unwrap();
    assert!(!packets.is_empty(), "座席應送出 RTP");
    assert!(
        packets.iter().all(|(pt, _)| *pt == 0),
        "RTP payload type 應為 0（PCMU）"
    );
    // 沒有音檔 → 靜音；μ-law 的零電平是 0xFF（A-law 是 0xD5）
    assert!(
        packets.iter().all(|(_, p)| p.iter().all(|&b| b == 0xFF)),
        "內容必須是 μ-law 編碼"
    );
}

#[tokio::test]
async fn agent_keeps_preferred_codec_when_offered() {
    let (msgs, packets) = agent_call("0 8 101").await;
    let ok = invite_response(&msgs, 200).expect("座席應接聽");
    assert!(ok.contains(" RTP/AVP 8 101\r\n"), "{}", ok);
    let packets = packets.lock().unwrap();
    assert!(!packets.is_empty());
    assert!(packets
        .iter()
        .all(|(pt, p)| *pt == 8 && p.iter().all(|&b| b == 0xD5)));
}

#[tokio::test]
async fn agent_rejects_offer_without_g711_with_488() {
    let (msgs, packets) = agent_call("18 96").await;
    assert!(
        invite_response(&msgs, 488).is_some(),
        "沒有 G.711 的 offer 應回 488"
    );
    assert!(invite_response(&msgs, 200).is_none());
    assert!(invite_response(&msgs, 180).is_none(), "應在響鈴前拒絕");
    assert!(packets.lock().unwrap().is_empty());
}

// ── 民眾端：依 200 OK 的 answer 決定送出的 codec ─────────────────

#[tokio::test]
async fn caller_follows_pcmu_answer() {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server = sock.local_addr().unwrap();
    let (rtp, rtp_port, packets) = rtp_sink().await;
    spawn_rtp_collector(rtp, Arc::clone(&packets));
    let invites: Arc<Mutex<Vec<String>>> = Arc::default();
    let inv_rx = Arc::clone(&invites);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            let resp = |status: &str, body: &str| {
                let base = SipMessage::response_with_to_tag(&raw, status, "uas");
                if body.is_empty() {
                    return base;
                }
                base.replace(
                    "Content-Length: 0\r\n\r\n",
                    &format!(
                        "Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    ),
                )
            };
            let replies = match raw.split_whitespace().next().unwrap_or("") {
                "INVITE" => {
                    inv_rx.lock().unwrap().push(raw.clone());
                    vec![resp("180 Ringing", ""), resp("200 OK", &sdp(rtp_port, "0"))]
                }
                "BYE" | "CANCEL" => vec![resp("200 OK", "")],
                _ => vec![],
            };
            for r in replies {
                let _ = sock.send_to(r.as_bytes(), peer).await;
            }
        }
    });

    let logs = std::env::temp_dir().join(format!("sipress-codec-caller-{}", std::process::id()));
    let cfg = Config {
        server_addr: server.to_string(),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs.to_string_lossy().into_owned(),
        max_total_calls: Some(1),
        duration_secs: 10,
        call_duration_secs: 1,
        enable_rtp: true,
        rtp_base_port: 42000,
        codec: Codec::Pcma,
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(15), Engine::new(cfg).run(None))
        .await
        .expect("民眾端應正常結束")
        .unwrap();
    assert_eq!(report.calls_answered, 1);

    let invite = invites.lock().unwrap()[0].clone();
    assert!(
        invite.contains(" RTP/AVP 8 0 101\r\n"),
        "offer 的第一個應是偏好的 PCMA：\n{}",
        invite
    );

    let packets = packets.lock().unwrap();
    assert!(!packets.is_empty(), "民眾端應送出 RTP");
    assert!(
        packets.iter().all(|(pt, _)| *pt == 0),
        "answer 是 PCMU → 送 PT 0"
    );
    assert!(
        packets.iter().all(|(_, p)| p.iter().all(|&b| b == 0xFF)),
        "內容必須是 μ-law 編碼"
    );
}

// ── 音檔無法使用時在開始前失敗 ───────────────────────────────

#[tokio::test]
async fn unsupported_audio_fails_before_the_test_starts() {
    let dir = std::env::temp_dir().join(format!("sipress-codec-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mp3 = dir.join("prompt.mp3");
    std::fs::write(&mp3, [0u8; 1024]).unwrap();
    let base = Config {
        server_addr: "127.0.0.1:9".into(),
        logs_dir: dir.to_string_lossy().into_owned(),
        enable_rtp: true,
        audio_file: Some(mp3),
        duration_secs: 1,
        ..Default::default()
    };

    let err = Engine::new(base.clone()).run(None).await.unwrap_err();
    let msg = format!("{:#}", err);
    assert!(
        msg.contains("音檔無法使用") && msg.contains("mp3"),
        "{}",
        msg
    );

    let agent = Config {
        mode: Mode::Agent,
        agent_accounts: vec![AgentAccount {
            extension: "1".into(),
            ..Default::default()
        }],
        ..base
    };
    let err = AgentEngine::new(agent).run(None).await.unwrap_err();
    assert!(format!("{:#}", err).contains("音檔無法使用"));

    let _: SocketAddr = "127.0.0.1:9".parse().unwrap();
}
