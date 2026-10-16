/// 座席流程：註冊後 / 結束前撥特殊碼（登入、就緒、未就緒、登出），以及測試中手動撥碼
///
/// mock SIPX 依撥的號碼回應：
///   *94…（登入）：先回 401 要求認證，帶認證後回 200 OK，等座席保持時間到後送 BYE
///   *96（就緒）、*95（登出）：回 200 OK，100ms 後由 mock 送 BYE（模擬播完提示音掛斷）
///   *97（未就緒）：回 486
///   *98：回 183 播提示音、不送最終回應（座席保持時間到要 CANCEL，mock 回 487）
///   *99：回 183，100ms 後 mock 自己送 487（SIPX 特殊碼的正常結束）
///   *93：回 603，Reason 帶 `CTIError2140`
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, AgentFlow, Config, FlowStep, Mode};
use sipress_core::sip::{SipMessage, SipParser};
use sipress_core::stats::FinalReport;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

type Log = Arc<Mutex<Vec<(Instant, String)>>>;

fn dial(code: &str, label: &str) -> FlowStep {
    FlowStep::Dial {
        code: code.into(),
        label: label.into(),
    }
}

/// 對 INVITE 回 200 OK（帶 SDP 與 Contact）
fn ok_with_sdp(invite: &str, mock: SocketAddr) -> String {
    let base = SipMessage::response_with_to_tag(invite, "200 OK", "mocktag");
    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
               m=audio 9 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n";
    base.replace(
        "Content-Length: 0\r\n\r\n",
        &format!(
            "Contact: <sip:ivr@{mock}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            sdp.len(),
            sdp
        ),
    )
}

/// mock 對撥碼 dialog 送 BYE（From = mock 端的 To tag）
fn server_bye(invite: &str, mock: SocketAddr, agent: SocketAddr) -> String {
    let call_id = SipParser::call_id(invite).unwrap();
    let from_tag = SipParser::from_tag(invite).unwrap();
    let user = SipParser::from_user(invite).unwrap();
    format!(
        "BYE sip:{user}@{agent} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {mock};branch=z9hG4bK-bye{n}\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:ivr@{mock}>;tag=mocktag\r\n\
         To: <sip:{user}@{mock}>;tag={from_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 BYE\r\n\
         Content-Length: 0\r\n\r\n",
        n = rand_suffix(),
    )
}

fn rand_suffix() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}

fn request_user(raw: &str) -> String {
    raw.split_whitespace()
        .nth(1)
        .and_then(|u| u.strip_prefix("sip:"))
        .and_then(|u| u.split('@').next())
        .unwrap_or("")
        .to_string()
}

async fn spawn_server() -> (SocketAddr, Log) {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    let log: Log = Arc::default();
    let log_rx = Arc::clone(&log);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        // Call-ID → 尚未送最終回應的 INVITE（收到 CANCEL 時回 487）
        let mut pending: std::collections::HashMap<String, String> = Default::default();
        while let Ok((n, agent)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            log_rx.lock().unwrap().push((Instant::now(), raw.clone()));
            let reply = |m: String| {
                let sock = Arc::clone(&sock);
                async move {
                    let _ = sock.send_to(m.as_bytes(), agent).await;
                }
            };
            if raw.starts_with("REGISTER") || raw.starts_with("BYE") {
                reply(SipMessage::ok_for_server_bye(&raw)).await;
            } else if raw.starts_with("CANCEL") {
                reply(SipMessage::response_for_request(&raw, "200 OK")).await;
                if let Some(inv) = pending.remove(&SipParser::call_id(&raw).unwrap()) {
                    reply(SipMessage::response_with_to_tag(
                        &inv,
                        "487 Request Terminated",
                        "ivrtag",
                    ))
                    .await;
                }
            } else if raw.starts_with("INVITE") {
                let user = request_user(&raw);
                if user == "*98" || user == "*99" {
                    reply(SipMessage::response_with_to_tag(
                        &raw,
                        "183 Session Progress",
                        "ivrtag",
                    ))
                    .await;
                    if user == "*98" {
                        pending.insert(SipParser::call_id(&raw).unwrap(), raw.clone());
                    } else {
                        let sock = Arc::clone(&sock);
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            let r = SipMessage::response_with_to_tag(
                                &raw,
                                "487 Request Terminated",
                                "ivrtag",
                            );
                            let _ = sock.send_to(r.as_bytes(), agent).await;
                        });
                    }
                } else if user == "*93" {
                    let r = SipMessage::response_with_to_tag(&raw, "603 Decline", "ctitag")
                        .replace(
                            "Content-Length: 0
",
                            "Reason: SIP;cause=603;text=\"CTIError2140\";location=\"acd\"
Content-Length: 0
",
                        );
                    reply(r).await;
                } else if user.starts_with("*94") {
                    if raw.contains("Authorization: Digest") {
                        reply(ok_with_sdp(&raw, addr)).await;
                    } else {
                        let r = SipMessage::response_with_to_tag(&raw, "401 Unauthorized", "authtag")
                            .replace(
                                "Content-Length: 0\r\n",
                                "WWW-Authenticate: Digest realm=\"mock\", nonce=\"abc\"\r\nContent-Length: 0\r\n",
                            );
                        reply(r).await;
                    }
                } else if user == "*97" {
                    reply(SipMessage::response_with_to_tag(
                        &raw,
                        "486 Busy Here",
                        "busytag",
                    ))
                    .await;
                } else {
                    reply(ok_with_sdp(&raw, addr)).await;
                    let sock = Arc::clone(&sock);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let _ = sock
                            .send_to(server_bye(&raw, addr, agent).as_bytes(), agent)
                            .await;
                    });
                }
            }
        }
    });
    (addr, log)
}

fn config(server: SocketAddr, flow: AgentFlow, duration_secs: u64) -> Config {
    let logs = std::env::temp_dir().join(format!("sipress-dialflow-test-{}", std::process::id()));
    Config {
        server_addr: server.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        mode: Mode::Agent,
        duration_secs,
        agent_accounts: vec![AgentAccount {
            extension: "0000021113331001".into(),
            username: "0000021113331001".into(),
            password: "pw".into(),
            domain: String::new(),
        }],
        agent_flow: flow,
        ..Default::default()
    }
}

async fn run(engine: AgentEngine) -> FinalReport {
    tokio::time::timeout(Duration::from_secs(30), engine.run(None))
        .await
        .expect("agent engine 應正常結束")
        .unwrap()
}

/// mock 收到的 INVITE 的號碼（依順序）
fn invited(msgs: &[(Instant, String)]) -> Vec<String> {
    msgs.iter()
        .filter(|(_, m)| m.starts_with("INVITE"))
        .map(|(_, m)| request_user(m))
        .collect()
}

#[tokio::test]
async fn start_and_stop_flows_dial_codes_in_order() {
    let (server, log) = spawn_server().await;
    let flow = AgentFlow {
        on_start: vec![
            dial("*94{agent}*{pwd}", "登入"),
            FlowStep::Wait { secs: 0.2 },
            dial("*96", "就緒"),
        ],
        on_stop: vec![dial("*97", "未就緒"), dial("*95", "登出")],
        agent_password: "000000".into(),
        hold_secs: 0.3,
        ..Default::default()
    };
    let report = run(AgentEngine::new(config(server, flow, 2))).await;
    let msgs = log.lock().unwrap().clone();

    assert_eq!(
        invited(&msgs),
        vec!["*941001*000000", "*941001*000000", "*96", "*97", "*95"],
        "登入先被要求認證後重送；{{agent}} 取分機末 4 碼"
    );
    let auth = msgs
        .iter()
        .filter(|(_, m)| m.starts_with("INVITE sip:*94"))
        .nth(1)
        .unwrap();
    assert!(auth.1.contains("username=\"0000021113331001\""));
    assert!(auth.1.contains("uri=\"sip:*941001*000000@"));

    // 登入：保持 0.3 秒後由座席掛斷
    let ok_at = msgs
        .iter()
        .find(|(_, m)| m.starts_with("ACK") && m.contains("CSeq: 2 ACK"))
        .map(|(t, _)| *t)
        .expect("登入的 200 OK 要回 ACK");
    let bye_at = msgs
        .iter()
        .find(|(_, m)| m.starts_with("BYE"))
        .map(|(t, _)| *t)
        .expect("座席保持時間到要送 BYE");
    assert!(bye_at.duration_since(ok_at) >= Duration::from_millis(250));

    // 未就緒被拒（486）要 ACK；登出在解除註冊之前
    assert!(msgs
        .iter()
        .any(|(_, m)| m.starts_with("ACK") && m.contains("CSeq: 1 ACK") && m.contains("busytag")));
    let logout = msgs
        .iter()
        .position(|(_, m)| m.starts_with("INVITE sip:*95"))
        .unwrap();
    let dereg = msgs
        .iter()
        .position(|(_, m)| m.starts_with("REGISTER") && m.contains("Expires: 0"))
        .expect("最後要解除註冊");
    assert!(logout < dereg, "結束流程跑完才解除註冊");

    let summary: Vec<(String, u64, u64)> = report
        .agent_dials
        .iter()
        .map(|d| (d.code.clone(), d.ok, d.failed))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("*94{agent}*{pwd}".to_string(), 1, 0),
            ("*96".to_string(), 1, 0),
            ("*97".to_string(), 0, 1),
            ("*95".to_string(), 1, 0),
        ]
    );
    assert_eq!(report.agent_dials[2].fail_codes.get(&486), Some(&1));
    assert_eq!(report.agent_dials[0].label, "登入");
}

#[tokio::test]
async fn manual_steps_dial_on_registered_agents() {
    let (server, log) = spawn_server().await;
    let engine = AgentEngine::new(config(server, AgentFlow::default(), 2));
    let manual = engine.manual_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = manual.send(vec![dial("*96", "就緒")]);
    });
    let report = run(engine).await;
    let msgs = log.lock().unwrap().clone();
    assert_eq!(invited(&msgs), vec!["*96"]);
    assert_eq!(report.agent_dials.len(), 1);
    assert_eq!(report.agent_dials[0].ok, 1);
}

#[tokio::test]
async fn no_flow_means_no_dials() {
    let (server, log) = spawn_server().await;
    let report = run(AgentEngine::new(config(server, AgentFlow::default(), 1))).await;
    assert!(invited(&log.lock().unwrap()).is_empty());
    assert!(report.agent_dials.is_empty());
}

#[tokio::test]
async fn sipx_style_prompts_count_as_success_and_free_the_line() {
    let (server, log) = spawn_server().await;
    let flow = AgentFlow {
        on_start: vec![
            dial("*98", "就緒"),
            dial("*99", "登入"),
            dial("*93", "就緒2"),
        ],
        hold_secs: 0.3,
        ..Default::default()
    };
    let report = run(AgentEngine::new(config(server, flow, 2))).await;
    let msgs = log.lock().unwrap().clone();
    assert_eq!(invited(&msgs), vec!["*98", "*99", "*93"]);

    // *98：183 後保持 0.3 秒就 CANCEL（不等交換機），487 要 ACK
    let invite_at = msgs
        .iter()
        .find(|(_, m)| m.starts_with("INVITE sip:*98"))
        .unwrap()
        .0;
    let cancel_at = msgs
        .iter()
        .find(|(_, m)| m.starts_with("CANCEL sip:*98"))
        .expect("提示音保持時間到要 CANCEL")
        .0;
    let held = cancel_at.duration_since(invite_at);
    assert!(
        held >= Duration::from_millis(250) && held < Duration::from_secs(2),
        "{:?}",
        held
    );
    let acks = msgs
        .iter()
        .filter(|(_, m)| m.starts_with("ACK") && m.contains("ivrtag"))
        .count();
    assert_eq!(acks, 2, "*98 與 *99 的 487 都要 ACK");

    let summary: Vec<(String, u64, u64)> = report
        .agent_dials
        .iter()
        .map(|d| (d.code.clone(), d.ok, d.failed))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("*98".to_string(), 1, 0),
            ("*99".to_string(), 1, 0),
            ("*93".to_string(), 0, 1),
        ],
        "提示音後由座席掛斷或交換機 487 結束都算成功；603 算失敗"
    );
    assert_eq!(report.agent_dials[2].fail_codes.get(&603), Some(&1));
}
