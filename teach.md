# sipress 程式碼完整講解

本文件適合想深入理解 sipress 整體架構、各模組邏輯與關鍵注意事項的開發者。讀者需具備基本 Rust 知識。

## 目錄

1. [整體架構](#1-整體架構)
2. [Workspace 結構與編譯單元](#2-workspace-結構與編譯單元)
3. [core — 設定層 config.rs](#3-core--設定層-configrs)
4. [core — 指標層 stats.rs](#4-core--指標層-statsrs)
5. [core — SIP 訊息 sip/message.rs](#5-core--sip-訊息-sipmessagers)
6. [core — SIP 對話狀態機 sip/dialog.rs](#6-core--sip-對話狀態機-sipdialogrs)
7. [core — SIP 解析 sip/parser.rs & transport.rs](#7-core--sip-解析-sipparserrs--transportrs)
8. [core — 壓測主引擎 engine.rs](#8-core--壓測主引擎-enginers)
9. [core — RTP 音訊 rtp/audio.rs](#9-core--rtp-音訊-rtpaudiors)
10. [core — RTP 封包 rtp/packet.rs](#10-core--rtp-封包-rtppacketrs)
11. [core — RTP Session rtp/session.rs](#11-core--rtp-session-rtpsessionrs)
12. [core — RTP 品質統計 rtp/stats.rs](#12-core--rtp-品質統計-rtpstatsrs)
13. [core — 報告輸出 reporter.rs & html_reporter.rs](#13-core--報告輸出-reporterrs--html_reporterrs)
14. [core — SIP 日誌 sip_logger.rs](#14-core--sip-日誌-sip_loggerrs)
15. [core — REGISTER / Digest auth sip/register.rs](#15-core--register--digest-auth-sipregisterrs)
16. [core — 一次性註冊器 registrar.rs](#16-core--一次性註冊器-registrarrs)
17. [core — 座席引擎 agent_engine.rs](#17-core--座席引擎-agent_enginers)
18. [cli — 命令列介面](#18-cli--命令列介面)
19. [gui — Tauri 後端 commands.rs & lib.rs](#19-gui--tauri-後端-commandsrs--librs)
20. [gui — 前端狀態管理 testStore.ts](#20-gui--前端狀態管理-teststorets)
21. [gui — Vue 元件樹](#21-gui--vue-元件樹)
22. [建置系統 build.ps1 / build.sh](#22-建置系統-buildps1--buildsh)
23. [關鍵設計決策與注意事項](#23-關鍵設計決策與注意事項)

---

## 1. 整體架構

```
使用者
  │
  ├─ GUI (Tauri) ──────────────── Rust process
  │    ├── Vue 3 + TypeScript       │
  │    │   (WebView)                │
  │    │   invoke('start_test')     │
  │    └──────────────────────────▶│ commands.rs
  │                                 │   Engine::run()
  ├─ CLI / TUI ─────────────────▶──┤
  │    args.rs + clap               │ core/engine.rs
  │                                 │   ├── UDP socket (SIP)
  └─ (未來：API / CI mode)          │   ├── UDP socket (RTP/per-call)
                                    │   └── StatsSnapshot → callback
```

sipress 遵循「一個 core library，多個 entry point」的設計：
- **`sipress-core`**：純 library crate，包含所有業務邏輯（SIP、RTP、統計、報告）
- **`sipress`（cli）**：二進位 crate，`main.rs` 解析 CLI 參數後建構 `Config` 呼叫 core
- **`sipress-gui`（gui）**：Tauri 桌面應用，`commands.rs` 作為 frontend/backend 橋接

這樣的分層確保 core 可被單獨測試，且 CLI 與 GUI 共用完全相同的引擎邏輯。

---

## 2. Workspace 結構與編譯單元

```toml
# Cargo.toml（workspace root）
[workspace]
members = ["core", "cli", "gui/src-tauri"]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
```

**重點**：
- `resolver = "2"` 是 Rust 2021 edition 的要求，解決 feature 衝突問題
- workspace 共享 `tokio` 版本，避免多版本共存的連結問題
- Tauri 編譯的 `target/` 在 workspace root，不在 `gui/src-tauri/target/`
- 因此 build script 應從 `target/release/` 複製產出，而非 `gui/src-tauri/target/release/`

---

## 3. core — 設定層 config.rs

`Config` 是引擎的輸入資料結構，`Serialize/Deserialize` 讓它可以：
1. 從 CLI (`clap`) 組裝後直接傳入
2. 從 GUI 前端 JSON（透過 Tauri `invoke`）反序列化傳入

```rust
pub enum Mode { Caller, Agent }

pub struct AgentAccount {
    pub extension: String,
    pub username:  String,
    pub password:  String,
    pub domain:    String,
}

pub struct Config {
    pub server_addr:          String,       // "ip:port"
    pub cps:                  f64,          // calls per second
    pub max_concurrent_calls: usize,
    pub duration_secs:        u64,          // 0 = unlimited
    pub call_duration_secs:   u64,          // 0 = no auto-BYE
    pub invite_timeout_secs:  u64,          // 8s default
    pub max_total_calls:      Option<u64>,  // None = unlimited
    pub enable_rtp:           bool,
    pub audio_file:           Option<PathBuf>,
    pub mode:                 Mode,         // Caller / Agent
    pub agent_accounts:       Vec<AgentAccount>,
    // ...
}
```

**注意事項**：
- `duration_secs = 0` 表示「不限時間」；引擎內部以 `u64::MAX / 2` 作為遠期 deadline
- `max_total_calls = None` 表示不限通數，搭配 `duration_secs` 決定結束時機
- 兩者皆為「不限」時，測試必須手動停止（GUI: Stop 按鈕；CLI: Ctrl-C）
- `call_duration_secs = 0` 表示不主動送 BYE，通話由伺服器端掛斷
- `mode` 由 GUI 在 Sidebar 分頁切換；Caller 使用 `Engine`、Agent 使用 `AgentEngine`
- `agent_accounts` 在 Caller 模式被忽略；Agent 模式下若為空 engine 會直接 bail
- `Mode` 與 `AgentAccount` 都標 `#[serde(default)]`，舊版 JSON 不含這兩欄也能反序列化

---

## 4. core — 指標層 stats.rs

### LiveStats（無鎖計數）

```rust
pub struct LiveStats {
    pub calls_initiated: AtomicU64,   // 已送出 INVITE 數
    pub calls_answered:  AtomicU64,   // 已收到 200 OK for INVITE 數
    pub calls_completed: AtomicU64,   // 已收到 200 OK for BYE 數（正常結束）
    pub calls_failed:    AtomicU64,   // 已收到 4xx/5xx/6xx 數
    pub calls_timeout:   AtomicU64,   // 已逾時（INVITE 無回應）數
    pub calls_active:    AtomicI64,   // 目前進行中通話數（有借有還）
}
```

`calls_active` 是 `AtomicI64`（有號整數）而非 `AtomicU64`，因為理論上在某些 race condition 下可能瞬間為負，用有號整數避免 underflow panic；取快照時用 `.max(0) as u64`。

**生命週期計數**：
- `on_invite()` → `calls_initiated++`, `calls_active++`
- `on_answered()` → `calls_answered++`（不動 active，通話仍在進行）
- `on_completed()` → `calls_completed++`, `calls_active--`
- `on_failed()` → `calls_failed++`, `calls_active--`
- `on_timeout()` → `calls_timeout++`, `calls_active--`

所有計數均使用 `Ordering::Relaxed`，因為只需最終一致性，不需要跨執行緒的 happens-before 保證。

### StatsSnapshot（可序列化快照）

```rust
pub struct StatsSnapshot {
    pub calls_concurrent: u64,   // = calls_active.max(0)
    pub asr:              f64,   // calls_answered / calls_initiated × 100
    pub error_rate:       f64,   // (calls_failed + calls_timeout) / calls_initiated × 100

    // ── 即時 RTP 品質（Engine 進度 task 每秒聚合所有活躍 RtpSession） ──
    pub rtp_mos:           Option<f64>,   // 平均 MOS（None = 尚無 RTP session）
    pub rtp_loss_pct:      Option<f64>,   // 平均掉包率 %
    pub rtp_jitter_ms:     Option<f64>,   // 平均 Jitter ms
    pub rtp_sent_packets:  Option<u64>,   // 累計送出封包數
    pub rtp_recv_packets:  Option<u64>,   // 累計收到封包數

    // ── 引擎完成旗標 ──
    pub finished: bool,   // Engine/AgentEngine 主迴圈結束後設為 true
}
```

`QUEUED`（佇列中）在前端計算：`calls_initiated - calls_answered - calls_failed - calls_timeout`，等價於「已發出但未接通也未失敗的 INVITE 數量」，近似於 `calls_active`。

**即時 RTP 欄位**：Engine 的進度 task 每秒遍歷 `rtp_sessions: HashMap<call_id, RtpSession>`，對所有活躍 session 呼叫 `stats.snapshot()` 聚合平均 MOS / loss / jitter 與累計封包數，填入 `StatsSnapshot`。前端 `applySnapshot()` 讀取這些欄位後即時更新 RightPanel 的 MOS / 掉包 / Jitter 長條圖。

**`finished` 旗標**：解決 `duration=0 + max_total_calls` 場景下前端不知道引擎已結束的問題。Engine/AgentEngine 在主迴圈結束後、回傳 `FinalReport` 前，透過 `engine_finished: Arc<AtomicBool>` 設為 true，進度 task 將此值帶入 snapshot。前端 `pollTimer` 每秒檢查 `snap.finished`，為 true 時呼叫 `_finishTest()` 取回報告。

### DetailedStats（直方圖，需 Mutex）

使用 `hdrhistogram::Histogram<u64>` 儲存 PDD / 通話建立時間 / 通話持續時間的分佈。

HDR Histogram 的特性：
- `new_with_bounds(1, max, 3)` — 最小 1µs、3 位有效數字；PDD / 建立時間最大 60s（60M µs），通話時長最大 24h（86,400M µs）
- 精確記錄延遲百分位（P50/P95/P99/MAX）而不失精度
- 因 Histogram 本身不是 Sync，需包在 `Mutex` 中

**重要注意事項**：`std::sync::Mutex` guard 不能跨 `.await` 存活（`MutexGuard<T>` 不實作 `Send`）。engine.rs 中取得 histogram 數據的程式碼必須在所有 `.await` 完成後才取 lock：

```rust
// ✅ 正確：先完成所有 async 操作，再取 std::sync::MutexGuard
let rtp_agg = ... .await;  // 先做完所有 async
let pdd_p50 = {
    let pdd_h = detail.pdd_hist.lock().unwrap();  // 再取 Mutex
    us_to_ms(&pdd_h, 0.50)
};

// ❌ 錯誤：不能在 lock 之後有 .await
let pdd_h = detail.pdd_hist.lock().unwrap();
do_something().await;  // compile error: MutexGuard is not Send
```

---

## 5. core — SIP 訊息 sip/message.rs

sipress 不依賴任何 SIP 框架，手工構建 RFC 3261 格式字串。

### INVITE 構建

```
INVITE sip:{to}@{server} SIP/2.0
Via: SIP/2.0/UDP {local};branch={branch};rport
Max-Forwards: 70
From: <sip:{from}@{domain}>;tag={from_tag}
To: <sip:{to}@{server}>
Call-ID: {call_id}
CSeq: 1 INVITE
Contact: <sip:{from}@{local};transport=udp>
Content-Type: application/sdp
Content-Length: {len}
User-Agent: sipress/0.1
Allow: INVITE,ACK,BYE,CANCEL,OPTIONS

v=0
o=sipress 1000 1000 IN IP4 {local_ip}
s=sipress
c=IN IP4 {local_ip}
t=0 0
m=audio {rtp_port} RTP/AVP 8
a=rtpmap:8 PCMA/8000
a=ptime:20
a=sendrecv
```

**SDP 中的 RTP port**：
- 若啟用 RTP：填入預先分配好的本機偶數 port（例如 `10000`）
- 若未啟用 RTP：填入 `9`（SDP RFC 規定 port=9 表示媒體流被停用）

**SDP 為何只 offer PCMA（PT=8），不 offer PCMU（PT=0）？**
過去版本 offer `RTP/AVP 0 8`（同時提供 μ-law 與 A-law），按 SDP RFC 第一個 codec 為主，伺服器通常會選 PCMU。但實際對部分 SIP 軟交換機（要求 PCMA）會造成音訊不正確。改成只 offer PCMA 後，audio.rs 端也統一把所有音檔轉成 A-law（見 §9）。

### 對伺服器主動請求的 200 OK 鏡射

引擎收到伺服器發來的 `RE-INVITE`（Session-Expires 保活）或 `BYE` 時，必須鏡射對方的 Via / From / To / Call-ID / CSeq 標頭回 200 OK。`SipMessage` 提供：

```rust
SipMessage::ok_for_server_bye(raw_request) -> String
SipMessage::ok_for_server_reinvite(raw_request, local_addr, rtp_port) -> String
```

私有 helper `extract_request_headers()` 從 raw 訊息逐行抓取上述標頭（多行 Via 用 `\r\n` 串接保留）。RE-INVITE 的版本會額外附 SDP，sip body 同 INVITE 規格（PCMA-only）。

### SDP 解析的關鍵修正：sdp_rtp_addr()

過去 `SipResponse::sdp_rtp_port()` 只取 `m=audio PORT`，把 RTP 對端 IP **誤用 SIP server IP**。實際上交換機可能把媒體流路由到不同 IP（媒體閘道 / SBC），必須讀 SDP 的 `c=` 行：

```rust
pub fn sdp_rtp_addr(raw: &str, fallback_ip: &str) -> Option<String> {
    // 1. 掃 session-level c= IN IP4 <ip>
    // 2. 進入 m=audio 區塊後，再用 media-level c= 覆蓋
    // 3. 取 m=audio PORT
    // 4. 回傳 "ip:port"，若 c= 缺失或為 0.0.0.0 則用 fallback_ip
}
```

舊版 `sdp_rtp_port()` 仍保留供向下相容。

### 唯一識別碼生成

```rust
pub fn new_branch() -> String {
    format!("z9hG4bK-{}", Uuid::new_v4().simple())
}
```

RFC 3261 要求 branch 以 `z9hG4bK` 開頭（magic cookie），用以區別 RFC 2543 的舊格式。

```rust
pub fn new_call_id(domain: &str) -> String {
    format!("{}@{}", Uuid::new_v4().simple(), domain)
}
```

Call-ID 格式：`<uuid>@<local_domain>`，保證全局唯一。

### ACK 注意事項

ACK 的 CSeq 序號必須與對應的 INVITE **相同**（不是 +1）。這是 RFC 3261 §17.1.1.3 的規定，sipress 使用 `dialog.cseq`（初始值為 1）送 ACK，BYE 時才 `cseq + 1`。

---

## 6. core — SIP 對話狀態機 sip/dialog.rs

每一通通話對應一個 `Dialog` 實例，儲存在引擎的 `HashMap<String, Dialog>`（鍵為 Call-ID）。

### 狀態轉移圖

```
Calling ──(100)──▶ Trying
Calling/Trying ──(180)──▶ Ringing        [記錄 ringing_at，計算 PDD]
Calling/Trying/Ringing ──(200 INVITE)──▶ Connected  [記錄 answered_at，觸發 ACK + RTP]
Connected ──(通話時間到/BYE sent)──▶ Terminating
Terminating ──(200 BYE)──▶ Completed     [記錄 ended_at，計算 ACD]

Calling/Trying/Ringing ──(invite_timeout)──▶ TimedOut
Calling/Trying/Ringing/Connected ──(4xx/5xx/6xx)──▶ Failed(code)
```

### 計時點

| 指標 | 起始 | 結束 |
|------|------|------|
| PDD | `invite_sent_at` | `ringing_at` |
| Setup Time | `invite_sent_at` | `answered_at` |
| Call Duration（ACD）| `answered_at` | `ended_at`（BYE 200 OK） |

### Dialog 清理

引擎主迴圈每輪掃描後，`retain` 只保留「活躍」狀態：

```rust
dialogs.retain(|_, d| matches!(
    d.state,
    DialogState::Calling | DialogState::Trying |
    DialogState::Ringing | DialogState::Connected |
    DialogState::Terminating
));
```

`Completed`、`Failed`、`TimedOut` 的 dialog 在統計記錄後即被清除，避免 HashMap 無限增長。

---

## 7. core — SIP 解析 sip/parser.rs & transport.rs

### SipParser — Compact Form 支援

sipress 使用 `SipParser` struct（`core/src/sip/parser.rs`）統一解析所有 SIP header，**完整支援 RFC 3261 §7.3.3 Compact Form**。部分 SIP 軟交換機（尤其高負載時）會發送縮寫 header，若不支援則無法正確識別通話：

| Compact | 標準名稱 | SipParser 方法 |
|---------|----------|---------------|
| `i:` | `Call-ID:` | `call_id()` |
| `t:` | `To:` | `to_tag()` |
| `f:` | `From:` | `from_tag()` |
| `m:` | `Contact:` | `contact()` / `contact_uri()` |
| `v:` | `Via:` | `via()` |
| `l:` | `Content-Length:` | `content_length()` |

核心輔助方法 `header_value(raw, names: &[&str])` 接受多個別名一次查找：

```rust
pub fn call_id(raw: &str) -> Option<String> {
    Self::header_value(raw, &["call-id", "i"])  // 同時匹配長短形式
}
```

**重要**：解析函數全部是 `Option<T>` 回傳，不 panic。原始 UDP 封包可能損毀或截斷，必須容錯。

### Contact URI 解析

```rust
pub fn contact_uri(raw: &str) -> Option<String> {
    let val = Self::header_value(raw, &["contact", "m"])?;
    // <sip:user@host:port;params> → sip:user@host:port（剝除角括號與 URI 參數）
    if let Some(start) = val.find('<') {
        if let Some(end_rel) = val[start..].find('>') {
            return Some(val[start + 1..start + end_rel].to_string());
        }
    }
    Some(val.split(';').next().unwrap_or(&val).trim().to_string())
}
```

`contact_uri()` 從 200 OK 的 Contact header 取出純 SIP URI，引擎將其儲存在 `dialog.remote_contact`，ACK 與 BYE 使用該 URI 作為 Request-URI（RFC 3261 §12.2.1.1）。

### CSeq Method 解析

```rust
pub fn cseq_method(raw: &str) -> Option<String> {
    let val = Self::header_value(raw, &["cseq"])?;
    val.split_whitespace().nth(1).map(|s| s.to_uppercase())
    // "1 INVITE" → "INVITE"
}
```

為什麼需要 CSeq Method？因為 200 OK 可能是對 INVITE 或 BYE 的回應，兩者處理邏輯完全不同。

### SDP RTP Port 解析

```rust
pub fn sdp_rtp_port(raw: &str) -> Option<u16> {
    // 找到 SIP/SDP 分界（空行 \r\n\r\n 或 \n\n）
    let body_start = raw.find("\r\n\r\n").map(|i| i + 4)
        .or_else(|| raw.find("\n\n").map(|i| i + 2))?;
    // m=audio 16384 RTP/AVP 0 8  → 取第二個 token "16384"
    for line in raw[body_start..].lines() {
        if line.starts_with("m=") {
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            if let Ok(port) = parts.get(1)?.parse::<u16>() {
                return Some(port);
            }
        }
    }
    None
}
```

### transport.rs — SharedUdpSocket

所有 SIP 訊息（INVITE、ACK、BYE、CANCEL）共用同一個 UDP socket，以 `Arc` 共享給各個 tokio task：

```rust
pub struct SharedUdpSocket {
    pub socket:     UdpSocket,
    pub server:     SocketAddr,
    pub local_addr: String,  // "ip:port"（填入 SIP Via header）
}
```

`UdpSocket::connect()` 設定預設的對端地址，後續 `send()` 不需再指定目標。

---

## 8. core — 壓測主引擎 engine.rs

引擎是 sipress 最複雜的部分，採用 **Tokio 單執行緒事件迴圈**模型。

### 並發架構

```
tokio::spawn ─── Task 1：接收 UDP 封包
                    │
                    │  mpsc::unbounded_channel
                    ▼
tokio::spawn ─── Task 2：進度回報（每秒）
                    │
                    ▼
主控迴圈（非同步，但不 spawn 新 task）
  ①  drain ev_rx（處理 SIP 事件）
  ②  掃描逾時 & 送 BYE
  ③  發起新通話（INVITE）
  ④  判斷是否結束
  ⑤  sleep 500µs
```

**為什麼不用多執行緒？**
- SIP 狀態管理需要頻繁修改 `HashMap<String, Dialog>`
- 用 `tokio::Mutex` 而非 `std::sync::Mutex`，在 `.await` 期間可以釋放 lock
- 單一主控迴圈避免了 race condition

### SipEvent 內部事件

引擎用 `SipEvent` 從接收 task 傳遞解析結果給主控迴圈：

```rust
enum SipEvent {
    /// SIP 回應（UAC 收到的 1xx/2xx/4xx/5xx/6xx）
    Response {
        call_id:         String,          // 支援 "i:" compact form
        code:            u16,             // 狀態碼
        to_tag:          Option<String>,  // To-tag（200 OK 建立 dialog）
        method:          Option<String>,  // CSeq method（區分 INVITE/BYE）
        remote_rtp_addr: Option<String>,  // SDP c=+m= 解析出的對端 RTP "ip:port"
        contact_uri:     Option<String>,  // Contact header 純 URI（ACK/BYE 使用）
    },
    /// 伺服器主動發來的 SIP 請求（RE-INVITE 刷新 session / 伺服器 BYE 掛斷）
    IncomingRequest {
        call_id: String,
        method:  String,
        raw:     String,  // 原始訊息（建構 200 OK 回應，鏡射 Via/From/To/CSeq）
    },
}
```

`contact_uri` 從 200 OK 的 Contact header 提取（由 `SipParser::contact_uri()` 解析），收到後儲存在 `dialog.remote_contact`，後續 ACK 和 BYE 作為 Request-URI，符合 RFC 3261 §12.2.1.1。

### Task 1：接收迴圈（區分回應與請求）

```rust
tokio::spawn(async move {
    let mut buf = vec![0u8; 65536];
    loop {
        let n = udp_recv.socket.recv(&mut buf).await?;
        let raw = String::from_utf8_lossy(&buf[..n]).into_owned();

        // 使用 SipParser（支援 compact form，i: = Call-ID, m: = Contact…）
        let call_id = SipParser::call_id(&raw);   // 支援 "i:" compact form

        if raw.starts_with("SIP/2.0") {
            // SIP 回應 → SipEvent::Response
            let code            = SipParser::status_code(&raw);
            let to_tag          = SipParser::to_tag(&raw);
            let method          = SipParser::cseq_method(&raw);
            let remote_rtp_addr = if code == Some(200) { SipParser::sdp_rtp_addr(&raw) } else { None };
            let contact_uri     = if code == Some(200) { SipParser::contact_uri(&raw) } else { None };
            if let (Some(cid), Some(code)) = (call_id, code) {
                let _ = ev_tx2.send(SipEvent::Response { call_id: cid, code, to_tag, method, remote_rtp_addr, contact_uri });
            }
        } else {
            // SIP 請求（伺服器主動：RE-INVITE / BYE）→ SipEvent::IncomingRequest
            let req_method = raw.lines().next()
                .and_then(|l| l.split_whitespace().next()).unwrap_or("").to_string();
            if (req_method == "INVITE" || req_method == "BYE") {
                if let Some(cid) = call_id {
                    let _ = ev_tx2.send(SipEvent::IncomingRequest { call_id: cid, method: req_method, raw });
                }
            }
        }
    }
});
```

接收 task 只做最少的工作（解析狀態碼、Call-ID），不修改任何共享狀態，所有狀態更新都透過 channel 傳回主控迴圈。

### SipEvent 的兩個變體

```rust
enum SipEvent {
    Response { call_id, code, to_tag, method, remote_rtp_addr },
    IncomingRequest { call_id, method, raw },  // 伺服器主動發來的請求
}
```

主控迴圈用 `match ev { ... }` 分派：
- `Response` 走原本的狀態機（100/180/200/4xx-6xx）
- `IncomingRequest` 對 `INVITE` 回 `200 OK + SDP`（保活）並解析 SDP 中新的 RTP 地址、呼叫 `RtpSession::update_remote()` 切換送出目標；對 `BYE` 回 `200 OK` 並把 dialog 標記為 Completed

**為什麼一定要回 RE-INVITE？** 軟交換機常用 SIP Session-Expires (RFC 4028) 機制定期送 RE-INVITE 確認對方還活著。過去版本的 sipress 完全忽略這些請求，導致 ~9 秒後伺服器砍掉通話 → 接通通話被縮短，錄音檔只有開頭。

### Task 2：進度回報

```rust
let unlimited = cfg.duration_secs == 0;
tokio::spawn(async move {
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let elapsed = start.elapsed().as_secs_f64();
        let progress = if unlimited { 0.0 } else { (elapsed / duration).min(1.0) };
        cb(live.snapshot(), progress);
        if !unlimited && elapsed >= duration { break; }
    }
});
```

當 `duration_secs = 0` 時：
- `progress` 固定回報 `0.0`（前端顯示 0% 進度）
- 迴圈不自動 break，等主控迴圈結束後 task 被 drop

### CPS 節流機制

```rust
let cps_interval = Duration::from_secs_f64(1.0 / cfg.cps);
let mut next_call = Instant::now();

// 主控迴圈內：
if !total_limit_reached && now >= next_call && now < deadline {
    if concurrent < cfg.max_concurrent_calls {
        // 送 INVITE
        next_call = now + cps_interval;
    }
}
```

這是一個簡單的令牌桶機制：每隔 `1/CPS` 秒才能發起一通新通話。若並發上限已滿則跳過這個時槽，但 `next_call` 不推進（下一輪立即再試）。

**注意**：若 CPS = 10 且並發滿了，新通話會等到有空間才送出，實際 CPS 可能低於設定值。這是正確行為。

### 結束條件邏輯

```rust
let unlimited_time = cfg.duration_secs == 0;
let deadline = if unlimited_time {
    start + Duration::from_secs(u64::MAX / 2)  // 遠期（約 2900 億年）
} else {
    start + cfg.duration()
};

// 每輪主控迴圈結束前：
let total_limit_reached = cfg.max_total_calls
    .map_or(false, |max| live.calls_initiated.load(...) >= max);

let all_done = total_limit_reached
    && live.calls_active.load(...) <= 0;

if now >= deadline || all_done {
    time::sleep(Duration::from_secs(2)).await;  // 等待最後幾個通話結束
    break;
}
```

| 結束條件 | 說明 |
|---------|------|
| `now >= deadline` | 時間到（duration > 0） |
| `total_limit_reached && calls_active <= 0` | 達通數上限且沒有進行中的通話 |
| `stopped`（`watch::Receiver` 收到訊號）| GUI Stop → `stop_handle().send(true)` → 引擎停止新通話、對所有進行中通話發 BYE/CANCEL、等待 drain 後正常產生 FinalReport |

### ACK 與 BYE 的 Request-URI（RFC 3261 合規）

RFC 3261 §12.2.1.1 規定：**2xx ACK 的 Request-URI 必須使用 Contact header 中的 URI**，而非原始的 To header URI。這確保後續請求能路由到正確的 UA（對方可能在 NAT 後方）。

```rust
// 收到 200 OK for INVITE 時：
dialog.remote_contact = contact_uri.clone();  // 儲存 Contact URI

// 送 ACK 時：
let ack = SipMessage::ack(
    ...
    dialog.remote_contact.as_deref(),  // 傳入 Contact URI（若有）
);
// SipMessage::ack() 內部：若 contact_uri.is_some()，以之作為 Request-URI
```

**Non-2xx ACK（4xx/5xx/6xx）**：遵循 RFC 3261 §17.1.1.3，使用原始 INVITE 的 branch，且 Request-URI 保持與原 INVITE 相同，**不**使用 Contact URI：

```rust
400..=699 if method.as_deref() != Some("BYE") => {
    // non-2xx ACK 使用原始 INVITE branch，Request-URI 不變
    let ack_err = SipMessage::ack(..., None);  // contact_uri = None → 用原始 To URI
}
```

### 主迴圈睡眠時間

```rust
time::sleep(Duration::from_micros(500)).await;
```

500µs 的睡眠足以支援最高 2000 CPS（1/2000s = 500µs）。若需要更高 CPS，應縮短睡眠時間或改為 busy-wait。

---

## 9. core — RTP 音訊 rtp/audio.rs

`AudioSource` 負責**自動把任意輸入音檔轉成 G.711A（PCMA, PT=8）**並切成 160-byte frames（20ms @ 8kHz）：

```
.wav format=1  PCM16  → linear_to_alaw()        ──┐
.wav format=6  A-law  → 直接讀取                  ├─→ 160-byte PCMA frames
.wav format=7  μ-law  → ulaw_to_linear → A-law    │   ─→ 循環播放
.al / .alaw    raw    → 直接讀取                  │
.ul / .ulaw    raw    → ulaw_bytes_to_alaw()    ──┘
```

`AudioSource` 結構：

```rust
pub struct AudioSource {
    frames: Vec<Vec<u8>>,    // 預先切好的 160-byte PCMA frames
    cursor: usize,
    pub looping: bool,
    pub payload_type: u8,    // 固定 8（PCMA），供 RtpSession 使用
}
```

**WAV 解析的注意點**：A-law 的 fmt chunk 通常是 18 bytes（不是 PCM 的 16 bytes）；解析必須用 chunk-scan 邏輯（找到 `fmt ` chunk 後讀其長度），不能用固定偏移。

**循環播放**：`next_frame()` 到達結尾後回到頭部，確保長時間測試持續有音訊輸出。

**靜音模式**：`AudioSource::silence()` 回傳填滿 `0xD5`（A-law 零位準）的 frames，PT=8。仍然送出 RTP 封包以維持媒體流時序。

**為何全部統一 PCMA？** 見 §5 中的 SDP offer 章節。簡言之：SDP 只 offer PCMA，那麼傳送端也必須是 PCMA，否則 RTP 內容與 SDP 不一致。

---

## 10. core — RTP 封包 rtp/packet.rs

### RTP 標頭格式（RFC 3550）

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|V=2|P|X|  CC   |M|     PT      |       Sequence Number         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                           Timestamp                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                             SSRC                              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                             Payload                           |
```

sipress 使用：
- `V=2`（版本 2）
- `P=0, X=0, CC=0`（無 padding、無擴展、無 CSRC）
- `M=0`（非語音開始）
- `PT=8`（PCMA, G.711 A-law）— **改自舊版 PT=0 (PCMU)**，因部分軟交換機要求 A-law
- `seq`：每封包 +1，初始值隨機（防止預測）
- `ts`：每封包 +160（20ms × 8kHz），初始值隨機
- `SSRC`：每個 RTP session 隨機產生

**Payload Type 是怎麼決定的？** `RtpSession` 不再硬編碼 `PT_PCMU = 0`，而是從 `AudioSource::payload_type` 讀取（永遠是 8）。這層抽象讓未來支援其他 codec（G.722 / Opus）時不需要動 RTP 封包層。

---

## 11. core — RTP Session rtp/session.rs

### Port 預分配機制（TOCTOU 競態消除）

這是 sipress 中最重要的設計之一：**RTP port 在 INVITE 送出前就必須分配完成**，才能寫入 SDP。

早期實作的 `allocate_port()` 只回傳 port 號，bind 後立即釋放 socket。`RtpSession::start()` 收到 200 OK 後再重新 bind，中間存在「bind → release → rebind」的 **TOCTOU（Time-of-Check-Time-of-Use）競態**：在高 CPS 環境下，兩通 call 可能在微秒內爭搶同一個 port。

**改正後**：`allocate_port()` 回傳 `(u16, UdpSocket)`，socket **持續綁定** 直到傳入 `RtpSession::start()`：

```rust
// allocate_port：bind 後保持 socket，回傳 (port, 已綁定 socket)
pub async fn allocate_port(
    counter:  &Arc<Mutex<u16>>,
    local_ip: &str,
) -> Result<(u16, UdpSocket)> {
    let mut guard = counter.lock().await;
    for offset in (0u16..4000).step_by(2) {
        let port = start.wrapping_add(offset);
        if let Ok(socket) = UdpSocket::bind(&addr).await {
            *guard = port.wrapping_add(2);
            return Ok((port, socket));  // ← socket 保持綁定，不釋放
        }
    }
}

// engine.rs 送 INVITE 前：
let (rtp_port, rtp_socket) = RtpSession::allocate_port(&counter, local_ip).await?;
pre_bound_rtp.insert(call_id.clone(), rtp_socket);   // ← 暫存，port 持續佔用

// engine.rs 收到 200 OK 後：
let pre_socket = pre_bound_rtp.remove(&call_id);     // ← 取出已綁定的 socket
RtpSession::start(rtp_cfg, counter, pre_socket).await // ← 傳入 pre_bound
```

`RtpSession::start()` 的 `pre_bound: Option<UdpSocket>` 參數：

```rust
pub async fn start(
    config:    RtpSessionConfig,
    counter:   Arc<Mutex<u16>>,
    pre_bound: Option<UdpSocket>,   // Some = 直接使用（TOCTOU 安全），None = 動態分配
) -> Result<Self> {
    let (socket, local_port) = if let Some(sock) = pre_bound {
        let port = sock.local_addr()?.port();
        sock.connect(&config.remote_addr).await?;   // 只需 connect，不需再 bind
        (Arc::new(sock), port)
    } else {
        // fallback：重新動態分配（低 CPS 場景仍可用）
        let (port, sock) = Self::allocate_port(&counter, &config.local_ip).await?;
        sock.connect(&config.remote_addr).await?;
        (Arc::new(sock), port)
    };
    ...
}
```

### 兩個並發 Task

`RtpSession::start()` spawn 兩個獨立 task：

**Task A（傳送）**：每 20ms tick 一次，讀取音訊 frame，構建 RTP 封包，送出 UDP：

```rust
let mut interval = time::interval(Duration::from_millis(20));
loop {
    interval.tick().await;
    if stop.load(Ordering::Relaxed) { break; }
    // ...送封包
    seq = seq.wrapping_add(1);
    ts  = ts.wrapping_add(160);
}
```

**Task B（接收）**：非阻塞接收，有 50ms timeout，定期檢查 stop_flag：

```rust
match time::timeout(Duration::from_millis(50), socket.recv(&mut buf)).await {
    Ok(Ok(n)) => { /* 計算 jitter/loss */ }
    Err(_) => {} // timeout，繼續迴圈以便檢查 stop_flag
}
```

### 停止機制

```rust
pub fn stop(&self) -> RtpStatsSnapshot {
    self.stop_flag.store(true, Ordering::Relaxed);
    self.stats.snapshot()
}
```

`stop()` 是同步函數（不 await），設置 flag 後立即回傳快照。兩個 task 會在下次迴圈迭代時檢測到 flag 並退出。

### RE-INVITE RTP 目標更新

`RtpSession` 持有 `socket: Arc<UdpSocket>` 引用，提供 `update_remote(new_addr)` 方法：

```rust
pub async fn update_remote(&self, new_addr: &str) -> Result<()> {
    self.socket.connect(new_addr).await?;
    Ok(())
}
```

當 Engine 收到 RE-INVITE 且 SDP 中的 RTP 地址與原先不同時，呼叫此方法對 connected UDP socket 重新 `connect()`，後續 `send()` 即自動送往新地址。這解決了交換機 RE-INVITE 換 port 後音訊仍送到舊 port 的問題。

---

## 12. core — RTP 品質統計 rtp/stats.rs

### Jitter 計算（RFC 3550 §A.8）

RFC 3550 定義的 jitter 是**相鄰封包到達間隔的差異的指數加權移動平均**：

```
D(i,j) = |(Rj - Ri) - (Sj - Si)|    （均以 8kHz clock 計）

J(i) = J(i-1) + ( |D| - J(i-1) ) / 16
```

sipress 實作：

```rust
let recv_diff_8k = (recv_us_delta as f64 / 1_000_000.0 * 8000.0) as i64;
let rtp_diff     = rtp_ts_j.wrapping_sub(rtp_ts_i) as i64;
let d            = (recv_diff_8k - rtp_diff).unsigned_abs() as u64;

// J += (|D| - J) / 16  ← 整數近似，乘以 16 累積
let j     = jitter_x16_us.load(Relaxed);
let new_j = j.saturating_add(d).saturating_sub(j / 16);
jitter_x16_us.store(new_j, Relaxed);
```

最終輸出：`jitter_x16_us / 16 / 8 ms`（8kHz clock 單位 → ms）

### 掉包率計算（RFC 3550 §A.3 標準實作）

早期實作以「傳送數 vs 接收數」計算掉包率，缺點是假設傳送端計數可靠（接收端根本看不到傳送端的計數）。

**RFC 3550 §A.3 正確做法**：以**接收端序號空間**計算 expected 封包數：

```
expected = (max_seq - first_seq + 1) + seq_cycles × 65536
lost     = expected - received
loss_rate = lost / expected
```

sipress 追蹤三個欄位：

```rust
pub struct RtpStats {
    first_seq:  Mutex<Option<u16>>,  // 收到的第一個序號
    max_seq:    Mutex<Option<u16>>,  // 目前看到的最高序號
    seq_cycles: AtomicU64,           // u16 序號 wrap-around 次數（每回繞 +1）
    ...
}
```

`on_recv()` 中的 wrap-around 偵測：

```rust
if seq < prev_max && diff < 0x8000 {
    // 序號從 65535 回繞到小數值（forward wrap），cycles 計數 +1
    self.seq_cycles.fetch_add(1, Ordering::Relaxed);
}
```

計算 expected 時加回 `cycles × 65536`，確保長時通話（序號多次繞回）仍能正確統計。

**為什麼不用傳送計數？**
- 接收端看不到傳送端的計數器狀態
- 傳送端計數器只在同一個程序內可用，無法反映真實網路掉包

### 零收包的 Fallback（sent-based loss）

當完全沒有收到對端 RTP（`first_seq = None`），RFC 3550 的序號空間計算無法使用。但如果本端已成功送出封包（`sent > 0`），代表通話已建立卻收不到回程音訊 → 100% loss：

```rust
let (_expected, lost, loss_rate) = if seq_expected > 0 {
    // 正常：用接收端 seq 空間（RFC 3550 §A.3）
    let lost = seq_expected.saturating_sub(recv);
    (seq_expected, lost, lost as f64 / seq_expected as f64)
} else if sent > 0 {
    // 有送無收 → 100% loss
    (sent, sent, 1.0)
} else {
    // RTP 未啟動
    (0, 0, 0.0)
};
```

**修正前的問題**：零收包 → `expected = 0` → `loss = 0%` → `MOS ≈ 4.4（優良）`，完全誤導。修正後 loss = 100% → MOS ≈ 1.0（劣）。

### SSRC 變更追蹤（RE-INVITE 場景）

RE-INVITE 後對端可能更換媒體伺服器，導致 RTP 的 SSRC（Synchronization Source）改變。新 SSRC 的序號從全新的起點開始，與舊 SSRC 的序號空間不連續。

**修正前的問題**：舊 SSRC 最後序號 36232，新 SSRC 起始序號 61771 → `expected = 61771 - 35983 + 1 = 25789`，但實際只收到 ~614 包 → 掉包率 93.72%、MOS 1.08。完全是假象。

**解法**：在 `RtpStats` 新增三個欄位追蹤跨 SSRC 的累積統計：

```rust
pub struct RtpStats {
    current_ssrc:   Mutex<Option<u32>>,  // 目前的 SSRC
    prior_expected: AtomicU64,           // 前一個（或多個）SSRC 期間的 expected 累計
    prior_received: AtomicU64,           // 前一個（或多個）SSRC 期間的 received 累計
    ...
}
```

`on_recv()` 接收 SSRC 參數，偵測到 SSRC 變更時：

```rust
// 1. 累積舊 SSRC 期間的統計
prior_expected += (max_seq - first_seq + 1) + cycles × 65536
prior_received += received

// 2. 重設序號追蹤
first_seq = new_seq
max_seq   = new_seq
seq_cycles = 0
received   = 1   （本包）
current_ssrc = new_ssrc
```

`snapshot()` 和 `packet_loss_rate()` 合併所有 SSRC 期間：

```rust
total_expected = prior_expected + current_ssrc_expected
total_received = prior_received + current_ssrc_received
loss_rate      = (total_expected - total_received) / total_expected
```

這樣無論 RE-INVITE 換了幾次 SSRC，每個 SSRC 期間都獨立計算後加總，不會因為序號空間跳躍而產生假掉包。

### on_send 計數時機

`on_send()` 僅在 `socket.send()` 成功後呼叫，避免送失敗的封包被計為已送出：

```rust
match socket.send(&bytes).await {
    Ok(_)  => stats.on_send(frame.len()),
    Err(e) => { break; }
}
```

### MOS 估算（ITU-T E-Model 簡化版）

```
R = 93.2 - Ie_eff - Id

Ie_eff = 0 + (95 - 0) × ppl / (ppl + 4.3)    ← G.711 Ie=0, Bpl=4.3
Id     = max(0, jitter_ms - 150) × 0.1

MOS = 1 + 0.035R + R(R-60)(100-R) × 7×10⁻⁶
```

**基礎 R 值**：G.711 PCMU 在完美條件下 R = 93.2，對應 MOS ≈ 4.4（接近「優秀」）。

**實際限制**：
- 公式對 jitter < 150ms 不計分（電信業常見標準），超過才開始扣 Id
- 掉包率扣分的曲線在 5% 左右開始明顯（ITU-T G.113 Appendix I）
- 輸出限制在 [1.0, 5.0] 避免極端值

---

## 13. core — 報告輸出 reporter.rs & html_reporter.rs

### 文字報告（reporter.rs）

支援三種輸出格式：
- `table`：帶邊框的 Unicode 框線表格（終端機顯示）
- `json`：標準 JSON，欄位名與 `FinalReport` struct 一致
- `csv`：用逗號分隔，第一行為標頭

### HTML 報告（html_reporter.rs）

內嵌所有 CSS 和 SVG，產生單一自包含 `.html` 檔案，可用瀏覽器離線開啟。

```rust
// CLI 呼叫（儲存至 reports/ 目錄）
HtmlReporter::save(&report, &args.report_dir, &ts, &args.server)?;

// GUI 呼叫（回傳字串，由前端下載）
HtmlReporter::render(&report, &timestamp, &server_addr)
```

`render()` 接受 `server_addr: &str` 並顯示在報告標頭，GUI 透過前端傳入（`config.value.server`），CLI 透過 `args.server` 傳入。HTML 模板使用 Rust `format!` 巨集插值，無模板引擎依賴。包含：
- SVG 環形圖（ASR / CCR）
- SVG 長條圖（PDD / Setup Time 百分位數）
- RTP 品質區塊（有 RTP 時才顯示）
- SIP 錯誤碼明細表（4xx / 5xx / 6xx / 逾時）

---

## 14. core — SIP 日誌 sip_logger.rs

每次壓測產生一個帶時間戳記的 log 檔案：

```
logs/YYYYMMDD_HHMMSS_caller.sip.log   ← 民眾端
logs/YYYYMMDD_HHMMSS_agent.sip.log    ← 座席端
```

格式範例：

```
[00:00:01.023] ──▶ INVITE sip:29876@192.168.1.100:5060 SIP/2.0
[00:00:01.145] ◀── SIP/2.0 100 Trying
[00:00:01.243] ◀── SIP/2.0 180 Ringing
[00:00:01.501] ◀── SIP/2.0 200 OK
[00:00:01.502] ──▶ ACK sip:29876@192.168.1.100:5060 SIP/2.0
[00:00:31.502] ──▶ BYE sip:29876@192.168.1.100:5060 SIP/2.0
[00:00:31.612] ◀── SIP/2.0 200 OK
```

**注意**：SipLogger 使用 `std::fs::File` + `BufWriter`，寫入操作在 `.log_message()` 呼叫時同步進行（非 async），不會阻塞 tokio runtime 太久（檔案寫入通常極快）。高 CPS 場景下可考慮改為 async 寫入。

---

## 15. core — REGISTER / Digest auth sip/register.rs

座席模式必備的兩塊功能：建構 REGISTER 訊息、處理 401/407 Digest challenge。

### RegisterMessage::build()

```rust
pub fn build(
    username, _domain, server, local_addr,
    cseq, branch, from_tag, call_id,
    transport, expires,
    auth_header: Option<&str>,   // 第一次 None；401 後 Some("Digest username=...")
) -> String
```

**與 SIPp 對齊的格式**（修正前有多處差異導致部分 PBX 不認帳號）：

| 項目 | 修正前 | 修正後（對齊 SIPp） |
|------|--------|---------------------|
| From/To domain | `sip:user@{ip}`（不含 port） | `sip:user@{ip:port}`（含 port） |
| Contact | `<sip:user@addr;transport=udp>;expires=N` | `<sip:user@addr>`（簡潔格式） |
| Allow | 6 個方法 | 14 個方法（含 INFO, PRACK, NOTIFY, SUBSCRIBE 等） |
| Allow-Events | 缺少 | `talk,hold,conference,refer,check-sync` |
| Expires | 600 | 240（與 SIPp 一致） |

`_domain` 參數保留但不使用（避免改動所有呼叫端），From/To 統一用 `server`（含 ip:port）作為 domain。

### DigestChallenge

```rust
pub struct DigestChallenge {
    pub realm:     String,
    pub nonce:     String,
    pub algorithm: String,           // 通常 "MD5"
    pub qop:       Option<String>,   // "auth" 或 None（RFC 2069 fallback）
    pub opaque:    Option<String>,
}
```

`parse(raw)` 從 401 / 407 回應的 `WWW-Authenticate` 或 `Proxy-Authenticate` 標頭擷取（含多行延續處理）。

`build_authorization(user, pass, "REGISTER", uri)` 計算 MD5 response：

```
HA1 = MD5(username:realm:password)
HA2 = MD5(method:uri)

# qop=auth：RFC 2617 路徑
response = MD5(HA1 : nonce : nc : cnonce : qop : HA2)

# 無 qop：RFC 2069 fallback
response = MD5(HA1 : nonce : HA2)
```

### split_quoted_csv()

Digest header 內的 key=value 用逗號分隔，但 value 可能含引號包住的逗號（例：`qop="auth, auth-int"`）。簡易 state machine：遇到 `"` 切換 in_quote 狀態，僅在 `!in_quote` 時把 `,` 視為分隔符。

### 單元測試

`cargo test -p sipress-core` 覆蓋兩個案例：
- `parse_digest_challenge`：從含 `WWW-Authenticate: Digest realm=..., nonce=..., qop="auth"` 的 raw response 解析回 `DigestChallenge`
- `build_auth_response`：對給定 challenge 生出 Authorization 標頭，斷言含 `username="alice"` 與 `response="..."`

---

## 16. core — 一次性註冊器 registrar.rs

`register_once()` 是給 GUI **新增帳號時即時驗證**用的：開短期 UDP socket → REGISTER → 收 401 → 重送帶 Authorization → 收 200 OK 或失敗 → 關 socket → 回傳結果。

### 介面

```rust
pub async fn register_once(
    server_addr, domain, username, password,
    expires, transport,
) -> Result<RegisterResult>

pub struct RegisterResult {
    pub status:       RegisterStatus,   // Registered / AuthFailed / Rejected / Timeout / NetworkError
    pub message:      String,           // 給人看的描述
    pub expires_secs: Option<u32>,      // 成功時伺服器同意的 Expires
    pub sip_code:     Option<u16>,
}
```

### recv_with_timeout 的細節

```rust
async fn recv_with_timeout(sock, dur) -> Result<String> {
    loop {
        let n = timeout(remaining, sock.recv(&mut buf)).await??;
        let raw = ...;
        let code = parse_status_code(&raw);
        if (100..200).contains(&code) { continue; }   // 跳過 100/180 中繼回應
        return Ok(raw);
    }
}
```

**為什麼要跳過 1xx？** 部分伺服器在處理 REGISTER 時也會回 100 Trying。若直接拿來解析狀態碼會誤判為失敗。這段邏輯確保只取最終回應。

### 為何不直接重用引擎的 socket？

新增帳號是 GUI 一次性操作；開引擎需要 Config + tokio runtime + 統計結構，太重。短期 socket 用完即關，乾淨利落。代價是：壓測時 `AgentEngine` 會再開一條新 socket 重新 REGISTER（不共用註冊狀態）。

---

## 17. core — 座席引擎 agent_engine.rs

座席模式的核心，每個 `AgentAccount` 一個獨立 tokio task + 自帶 UDP socket。

### 架構

```
AgentEngine::run()
   │
   ├── 建立共享 rtp_port_counter + rtp_sessions 列表
   ├── 啟動進度回報 task（聚合即時 RTP 品質）
   │
   ├── 為每個 account spawn task：account_runner()
   │         │
   │         ├── 開 UdpSocket + connect(server)
   │         ├── 第一次 REGISTER（無認證）
   │         │
   │         └── tokio::select! 主迴圈：
   │               ├── sock.recv() ─→ handle_response() / handle_request()
   │               ├── re-register 計時器 ─→ 沿用快取 challenge 重送 REGISTER
   │               ├── 通話 timeout ─→ ACK(10s)/BYE(150s) 未收到 → 主動 BYE
   │               └── stop.changed() ─→ REGISTER Expires=0 → break
   │
   ├── time::sleep(duration) 或 stop.changed()
   ├── stop_tx.send(true)     // 通知所有 runner 解除註冊
   ├── time::sleep(2 秒)      // 給 deregister + RTP 停止一些時間
   └── 全部 abort + 收集 RTP 統計 + 產生 FinalReport
```

### RegState（per-account）

```rust
struct RegState {
    cseq: u32,                          // REGISTER CSeq 累加
    challenge: Option<DigestChallenge>, // 401 後快取，re-REGISTER 直接用不需再被 challenge
}
```

`Arc<Mutex<RegState>>` 在主迴圈與計時 branch 共享。每次 REGISTER 都 `cseq += 1`。

### handle_response：只關心 REGISTER 回應

```rust
match cseq_method {
    Some("REGISTER") => match code {
        200 => { registered = true; last_register_at = now; current_expires = parse_expires(...); }
        401 | 407 => { 解析 challenge 並重送 REGISTER }
        _ => { registered = false; log }
    }
    _ => return,  // BYE/INVITE 的回應在 caller 模式才需要，這裡是 UAS 不會主動發那些 dialog
}
```

### handle_request：所有伺服器主動的請求

| 方法 | 處理 |
|------|------|
| `INVITE`（新通話） | `live.on_invite()` → 100 Trying → 180 Ringing → 分配 RTP port → 200 OK + SDP → 啟動 `RtpSession`（若 enable_rtp）→ 在 dialogs 表中記錄 |
| `INVITE`（已有 dialog） | RE-INVITE，回 200 OK + SDP；解析 SDP 更新 RTP 目標（`update_remote()`） |
| `ACK` | 標記 `ack_received = true`（10s 未收到 → 主動 BYE） |
| `BYE` | 200 OK + 停止 RTP + `live.on_completed()` + 從 dialogs 表移除 |
| `CANCEL` | 200 OK（CANCEL）+ 487 Request Terminated（INVITE）+ `live.on_failed()` |
| `OPTIONS` | 200 OK（健康檢查）|

### build_response_with_sdp

200 OK for INVITE 必須含 SDP 才能完成媒體協商：

```rust
fn build_response_with_sdp(raw_request, status_line, to_tag, local_addr, rtp_port) -> String {
    let (via, from, to, call_id, cseq) = extract_request_headers_for_response(raw_request);
    let to_with_tag = inject_to_tag_if_missing(&to, to_tag);
    let sdp = "...m=audio {port} RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n...";
    // Contact 帶我們的本機 socket 地址，否則對方後續送 BYE 會找不到我們
    format!("SIP/2.0 {status}\r\n{via}\r\n{from}\r\n{to_with_tag}\r\n...{sdp}")
}
```

`inject_to_tag_if_missing` 處理一個 corner case：若伺服器送來的 INVITE 中 To 標頭沒有 tag（這是合法的，初次 INVITE 時 callee 還沒被指派 tag），我們得補上自己生成的 to_tag。

### 真實 RTP 收發

當 `enable_rtp = true` 時，座席模式使用與民眾模式相同的 `RtpSession` 基礎設施：

```rust
// 接聽 INVITE 時分配真實 RTP port
let (rtp_port, pre_bound) = RtpSession::allocate_port(&port_counter, local_ip).await?;
// 200 OK SDP 中宣告真實 port
let ok = build_response_with_sdp(raw, "200 OK", &to_tag, local_addr, rtp_port);
// 解析 INVITE SDP 中對端 RTP 地址
let remote_rtp_addr = SipResponse::sdp_rtp_addr(raw, sip_ip);
// 啟動 RTP session
let session = RtpSession::start(rtp_cfg, port_counter, pre_bound).await?;
rtp_sessions.lock().await.push(Arc::clone(&session.stats)); // 加入即時聚合列表
```

`rtp_sessions: Vec<Arc<RtpStats>>` 被進度 task 每秒遍歷，聚合所有活躍 session 的平均 MOS / loss / jitter，填入 `StatsSnapshot` 供前端即時顯示。

當 `enable_rtp = false` 時，退回到 `pick_dummy_rtp_port()` 隨機數字模式（純 SIP 信令測試）。

### ACK / BYE timeout 主動掛斷

```rust
// DialogCtx 追蹤 ACK 狀態
struct DialogCtx {
    ack_received: bool,   // 收到 ACK 後設為 true
    answered_at:  Instant, // 200 OK 送出時間
    rtp_session:  Option<RtpSession>,
    // ...
}

// tokio::select! 主迴圈中的 timeout branch：
// - !ack_received && elapsed > 10s → ACK timeout，主動 BYE
// - ack_received && elapsed > 150s → BYE timeout，主動 BYE
```

timeout 後座席建構 BYE（From/To 交換，因座席是 UAS）並送出，停止 RTP，記錄 calls_completed。開啟 `agent_reregister_after_call` 時會再立即 re-REGISTER（預設關閉）。

### 通話結束後 re-REGISTER

```rust
async fn trigger_re_register(...) {
    // 沿用快取的 Digest challenge，立即重送 REGISTER
    // 確保交換機不因 REGISTER 過期而不再分配來電給此座席
}
```

### Stop 訊號與優雅退出

```rust
// AgentEngine 持有 watch::Sender<bool>
let (stop_tx, _) = watch::channel(false);

// 外部呼叫 stop_handle().send(true) 觸發停止
// 或時間到時 engine 內部自行 send(true)
let _ = self.stop_tx.send(true);
time::sleep(1 秒).await;
for h in handles { h.abort(); }
```

改用 `watch::channel` 取代先前的 `Notify`，原因是 `Notify::notify_waiters()` 只會喚醒**已經在 await 的 task**，如果 runner 剛好在處理其他事情（例如正在收 SIP 訊息），通知會被丟失。`watch` 儲存最新值，`changed().await` 不會錯過已發生的狀態變更。

`stop_handle()` 回傳 `watch::Sender<bool>` clone，讓 GUI 的 `stop_test` command 可以從外部觸發 graceful stop（REGISTER Expires=0）。

### 為何用單 socket？

每個帳號一個 UDP socket，比起「全部帳號共用一個 socket，按 To header 路由」簡單得多。代價是 N 個帳號就 N 個 socket，但對 100~1000 個座席而言完全沒問題（系統 fd 限制通常 1024+）。

---

## 18. cli — 命令列介面

### args.rs（clap 參數）

```rust
#[derive(Parser)]
struct Args {
    #[arg(short = 's', long, default_value = "127.0.0.1:5060")]
    server: String,
    
    #[arg(short = 'd', long, default_value_t = 60)]
    duration: u64,  // 0 = unlimited
    
    #[arg(long, default_value = None)]
    max_calls: Option<u64>,
    // ...
}
```

### main.rs 流程

```
1. 解析 clap Args
2. 組裝 Config
3. 若 --tui：spawn TUI dashboard task
4. Engine::run(on_progress) .await
5. 輸出報告（table/json/csv）
6. 若 --html：產生 HTML 報告
```

### TUI Dashboard（tui/dashboard.rs）

使用 `ratatui` 繪製即時儀表板：
- 按 `q` 或 `Esc` 退出 TUI 顯示（引擎繼續在背景執行到完成）
- 每秒從 `on_progress` callback 收到 `StatsSnapshot` 更新畫面

---

## 19. gui — Tauri 後端 commands.rs & lib.rs

### lib.rs — 應用入口

```rust
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(AppState::default()))   // ← 必須
        .invoke_handler(tauri::generate_handler![
            start_test, stop_test, get_snapshot, get_report, get_html_report,
            register_account,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

**關鍵：`.manage()` 必須在 `lib.rs` 中呼叫**。若 `main.rs` 有獨立的 `Builder::default()` 且沒有 `.manage()`，會導致 runtime 錯誤：`state not managed for field 'state' on command 'start_test'`。

`main.rs` 唯一的職責是呼叫 `lib::run()`：

```rust
fn main() {
    sipress_gui_lib::run();
}
```

### AppState — 跨 Command 共享狀態

```rust
pub struct AppState {
    pub snapshot: Mutex<Option<StatsSnapshot>>,  // 最新快照（前端輪詢）
    pub report:   Mutex<Option<FinalReport>>,    // 最終報告（完成後）
    pub stop_tx:  Mutex<Option<mpsc::Sender<()>>>,  // 停止信號發送端
}
```

以 `Arc<AppState>` 包裝後 `.manage()`，讓每個 command 透過 `State<'_, Arc<AppState>>` 取得。

### start_test Command（依 mode 分派）

```rust
pub async fn start_test(
    state:  State<'_, Arc<AppState>>,
    config: Config,  // Tauri 自動從 JSON 反序列化
) -> Result<String, String> {
    let state = Arc::clone(&state);
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    *state.stop_tx.lock().unwrap() = Some(stop_tx);

    let on_progress: ProgressCallback = Arc::new(move |snap, _| {
        *state_cb.snapshot.lock().unwrap() = Some(snap);
    });

    let mode = config.mode.clone();
    tokio::spawn(async move {
        let result = match mode {
            Mode::Caller => {
                let engine      = Engine::new(config);
                let stop_handle = engine.stop_handle();
                // 將前端的 stop 訊號轉發成 watch::Sender(true)
                // 讓 Engine 走完 graceful stop（BYE/CANCEL + drain）
                let forward = tokio::spawn(async move {
                    if stop_rx.recv().await.is_some() {
                        let _ = stop_handle.send(true);
                    }
                });
                let r = engine.run(Some(on_progress)).await;
                forward.abort();
                r
            }
            Mode::Agent => {
                let engine      = AgentEngine::new(config);
                let stop_handle = engine.stop_handle();
                let forward = tokio::spawn(async move {
                    if stop_rx.recv().await.is_some() {
                        let _ = stop_handle.send(true);
                    }
                });
                let r = engine.run(Some(on_progress)).await;
                forward.abort();
                r
            }
        };
        match result {
            Ok(report) => *state.report.lock().unwrap() = Some(report),
            Err(e)     => tracing::error!("Engine 錯誤: {}", e),
        }
    });

    Ok("started".to_string())  // 立即回傳，不等待引擎完成
}
```

`start_test` 立即回傳 `"started"`，引擎在背景 task 中執行。前端收到回傳後開始輪詢 `get_snapshot`。

**Graceful Stop 機制**：不再用 `tokio::select!` 直接中斷引擎。改為將前端 `stop_rx` 透過一個 `forward` task 轉發成 `stop_handle().send(true)`，引擎收到後：
- **Caller 模式**：停止發起新通話 → 對所有 `Connected` 通話發 BYE、`Calling/Trying/Ringing` 通話發 CANCEL → 等待 BYE 回應 → 正常回傳 `FinalReport`
- **Agent 模式**：通知所有 runner 發 `REGISTER Expires=0` → 等待 deregister → 正常回傳 `FinalReport`

這確保交換機端在 Stop 後立即釋放通道，不需等 session timer 超時。

### register_account Command

座席模式新增帳號時即時驗證用：

```rust
#[tauri::command]
pub async fn register_account(
    server, domain, username, password,
    expires: Option<u32>,
    transport: Option<String>,
) -> Result<RegisterResult, String> {
    // domain 為空時取 server IP 作 fallback
    register_once(&server, &dom, &username, &password,
                  expires.unwrap_or(3600), &transport.unwrap_or("UDP".into()))
        .await.map_err(|e| e.to_string())
}
```

回傳的 `RegisterResult` 含 status / message / expires_secs / sip_code，前端依 status 切換 badge 顏色。

### stop_test 的 MutexGuard 注意事項

```rust
pub async fn stop_test(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    // ✅ 取出 sender 後立即 drop guard，再 .await
    let tx = state.stop_tx.lock().unwrap().take();
    if let Some(tx) = tx {
        let _ = tx.send(()).await;  // guard 已被 drop
        Ok("stopped".to_string())
    } else {
        Err("沒有正在執行的壓測".to_string())
    }
}
```

若直接 `state.stop_tx.lock().unwrap().as_ref()?.send().await`，`MutexGuard` 會跨越 `.await`，導致 `!Send` 編譯錯誤。

### Tauri 2 Capabilities

Tauri 2 採用明確權限模型，每個 window API 都必須在 `capabilities/default.json` 宣告：

```json
{
  "permissions": [
    "core:default",
    "core:window:allow-close",
    "core:window:allow-minimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-start-dragging",
    "opener:default",
    "dialog:allow-open"
  ]
}
```

未宣告的操作（如視窗拖曳）會靜默失敗，不會有錯誤訊息，難以排查。

---

## 20. gui — 前端狀態管理 testStore.ts

### Pinia Store 架構

```typescript
export const useTestStore = defineStore('test', () => {
    const config = ref<TestConfig>({...})        // 使用者輸入
    const metrics = ref<Metrics>({...})          // 即時指標
    const series = ref<SeriesData>({...})        // 圖表資料
    const status = ref<TestStatus>('idle')       // 'idle' | 'running' | 'finished'
    const logs = ref<LogEntry[]>([])             // SIP 事件日誌
    const elapsedSec = ref(0)                    // 計時器秒數
    // ...
})
```

### CallerProfile — 新增 callDuration

`CallerProfile` 介面新增 `callDuration` 欄位，對應 Rust Config 的 `call_duration_secs`：

```typescript
export interface CallerProfile {
    ...
    callDuration: number   // per-call duration seconds（0 = 不主動 BYE，等對方掛斷）
}
```

預設值 30 秒，對應 `call_duration_secs: 30`。

### Config 轉換（frontend → Rust）

前端 `TestConfig` 的欄位名與 Rust `Config` struct 的欄位名必須一致（Tauri 自動 camelCase ↔ snake_case 轉換）。`buildRustConfig()` 包含 `call_duration_secs`：

```typescript
function buildRustConfig() {
    const c = config.value
    return {
        server_addr:          c.server,
        duration_secs:        c.duration,
        max_total_calls:      c.caller.totalCalls > 0 ? c.caller.totalCalls : null,
        call_duration_secs:   c.caller.callDuration,   // ← 新增，對應 --call-duration
        // ...
    }
}
```

**注意**：`max_total_calls: 0` 在 Rust 端是 `Option<u64>` 的 `None`（不限），前端傳 `null` 對應 Rust `None`，傳 `0` 對應 `Some(0)`（立即停止）。因此前端必須明確將 0 轉換為 `null`。

`cliCommand` computed 對應加入 `--call-duration` 選項：

```typescript
const callDur = c.caller.callDuration > 0 ? ` --call-duration ${c.caller.callDuration}` : ''
// callDuration = 0 不輸出 --call-duration（保持 Rust 端預設）
```

### respCodes — 即時與最終兩階段更新

`respCodes` 是 `Record<string, number>`，在兩個不同時機更新：

**即時（`applySnapshot()`）**：每秒從快照計數器更新 200/408 的近似值：
```typescript
respCodes.value['200'] = snap.calls_answered   // 即時接通數
respCodes.value['408'] = snap.calls_timeout    // 即時逾時數
```

**最終（`_tryFetchReport()`）**：測試完成後從 `FinalReport.fail_codes` 取得精確的 4xx/5xx/6xx 分佈，再補上 200/408：
```typescript
// fail_codes: Record<string, number> — Rust 端統計的各錯誤碼數量
if (report.fail_codes) {
    for (const [code, count] of Object.entries(report.fail_codes)) {
        respCodes.value[code] = count   // 486, 404, 503 等
    }
}
// 200 OK 和 408 Timeout 由 summary 計數器補充
respCodes.value['200'] = report.calls_answered
respCodes.value['408'] = report.calls_timeout
```

`RustReport` 介面包含 `fail_codes: Record<string, number>` 欄位，由 Rust `FinalReport.fail_codes` 序列化而來。

### 輪詢機制

```typescript
async function startPolling() {
    pollingTimer = setInterval(async () => {
        const snap = await invoke<RustSnapshot | null>('get_snapshot');
        if (snap) {
            metrics.value.succeeded = snap.calls_answered;
            metrics.value.failed    = snap.calls_failed + snap.calls_timeout;
            metrics.value.queued    = Math.max(0,
                snap.calls_initiated - snap.calls_answered
                - snap.calls_failed - snap.calls_timeout);
            // ...更新圖表資料
        }
    }, 1000);
}
```

前端每 1 秒呼叫 `get_snapshot`（同步 Rust command，直接從 `AppState.snapshot` 讀取，不做任何計算），效率極高。

### 計時器與 duration=0 的處理

```typescript
clockTimer = setInterval(() => {
    elapsedSec.value++;
    // duration>0 時依計時器自動停止
    if (config.value.duration > 0 && elapsedSec.value >= config.value.duration) {
        _finishTest();
    }
}, 1000);

// pollTimer 每秒輪詢 snapshot，檢查引擎完成旗標
pollTimer = setInterval(async () => {
    const snap = await invoke<RustSnapshot>('get_snapshot');
    if (snap) {
        applySnapshot(snap);
        if (snap.finished) { _finishTest(); }  // ← 引擎回報完成
    }
}, 1000);
```

**自動停止機制**（雙重保障）：
1. **`duration > 0`**：前端 `clockTimer` 計時到達後呼叫 `_finishTest()`
2. **`snap.finished`**：後端引擎主迴圈結束後設 `finished = true`，前端 `pollTimer` 每秒檢查

這解決了 `duration=0 + max_total_calls` 的場景：引擎達到總通數上限後自動結束，透過 `finished` 旗標通知前端，不再依賴計時器。使用者點擊 Stop 按鈕仍可隨時手動停止。

**即時 RTP 品質更新**：`applySnapshot()` 除了更新 SIP 指標外，也讀取 `snap.rtp_mos` 等欄位即時更新 `rtpMetrics`，讓 RightPanel 的 MOS / 掉包率 / Jitter 長條圖在測試期間即時刷新（不再只在測試結束後顯示）。

### HTML 報告匯出（exportHtml）

```typescript
async function exportHtml() {
    const ts   = /* YYYYMMDD_HHMMSS */
    const html = await invoke<string>('get_html_report', {
        serverAddr: config.value.server,  // 顯示在報告標頭
        timestamp:  ts,
    })
    // 與 JSON/CSV 相同的下載流程
    const blob = new Blob([html], { type: 'text/html' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `sipress_${ts}.html`
    a.click()
}
```

**設計選擇**：HTML 由 Rust 產生字串後傳回前端下載，而非在 Rust 端寫入磁碟並開啟。原因：
1. 不需要 Rust 端的檔案系統路徑 dialog（Tauri 的 blocking save dialog 不適合 async context）
2. 與 JSON/CSV 匯出行為一致，前端統一管理下載邏輯
3. 前端傳入 `timestamp` 確保 Rust 不需要依賴 `chrono` crate

---

## 21. gui — Vue 元件樹

```
App.vue
└── MainLayout.vue
    ├── TitleBar.vue          進度條 + Start/Stop 按鈕 + ↓ JSON / ↓ CSV / ↓ HTML 匯出
    ├── MetricStrip.vue       8 欄即時指標（CPS/CONCUR/SUCCESS/FAILED/QUEUED/ASR/ERR%/PDD）
    └── ContentArea.vue
        ├── Sidebar.vue       左側設定面板
        ├── ChartPanel.vue    中間折線圖（多個標籤頁）
        ├── RightPanel.vue    右側詳細資訊
        │   ├── CallStatus.vue    通話狀態分佈
        │   ├── RtpQuality.vue    MOS/掉包/Jitter
        │   ├── ResponseCodes.vue 回應碼統計
        │   └── SipFlow.vue       最後通話時序圖
        └── LogPanel.vue      底部 SIP 日誌
```

### Sidebar.vue 關鍵設計

- `duration` 欄位 `min="0"`，設為 0 時顯示「不限時間」提示
- `totalCalls` 欄位設為 0 時顯示「不限（依測試時長）」提示
- 音檔選擇器使用 `@tauri-apps/plugin-dialog` 的 `open()` 函數（非 `<input type="file">`，因為 Tauri WebView 的 file input 在某些平台需要特殊 capability）

**新增：單通時長（callDuration）滑桿**

在民眾端設定（總通數上限下方）新增 `callDuration` 控制：

```html
<div class="field">
  <label>單通時長 (s) <span class="tag">--call-duration</span></label>
  <div class="slider-row">
    <input type="range" min="0" max="300" v-model.number="store.config.caller.callDuration">
    <input type="number" min="0" v-model.number="store.config.caller.callDuration" class="num-input">
  </div>
  <div class="field-hint">
    {{ store.config.caller.callDuration === 0 ? '0 = 不限（直到對方掛斷）' : `接通後約 ${store.config.caller.callDuration} 秒掛斷` }}
  </div>
</div>
```

`v-model.number` 確保雙向綁定為數字型別（不是字串），max=300 限制最大值。

### ChartPanel.vue — 折線圖

使用 Chart.js / Vue-ChartJS，每次從 store 的 `series` 讀取最新資料點更新圖表。圖表有最大資料點數限制（通常 300 點），超過後自動移除最舊的點。

---

## 22. 建置系統 build.ps1 / build.sh

### 輸出路徑

```powershell
# Windows（build.ps1）
$GUI_RELEASE = "target\release"   # workspace root 下的 target
```

```bash
# Linux/macOS（build.sh）
GUI_TARGET_DIR="target/release"   # workspace root 下的 target
```

**陷阱**：若誤用 `gui\src-tauri\target\release`，Tauri 跨 workspace 編譯時找不到產出物。

### GUI Bundle 輸出路徑

Tauri 打包產出的安裝檔（.msi/.deb/.dmg）位於：

```
target/release/bundle/msi/     ← Windows MSI 安裝版
target/release/bundle/nsis/    ← Windows NSIS 安裝版
target/release/bundle/deb/     ← Linux .deb 安裝版
target/release/bundle/appimage/ ← Linux AppImage 免安裝版
target/release/bundle/dmg/     ← macOS .dmg 安裝版
```

build script 從這些路徑複製到 `dist/`。

---

## 23. 關鍵設計決策與注意事項

### A. 為什麼用 UDP 而非 TCP for SIP？

預設使用 UDP，因為大多數中國電信軟交換機（SoftX3000、S8500 等）預設接受 UDP SIP。TCP 雖然更可靠，但 TCP 連線管理增加了壓測工具的複雜度，且在高 CPS 場景下 TCP 的 SYN 握手開銷更明顯。

### B. SIP 認證（401/407）的實作策略

從 v0.2 起，**座席模式**完整實作 RFC 2617 Digest 認證（含 qop=auth 與 RFC 2069 fallback），因為座席必須帶帳密註冊到交換機。實作位於：
- `core/src/sip/register.rs` — challenge 解析 + response 計算
- `core/src/registrar.rs` — 一次性 REGISTER（GUI 新增帳號用）
- `core/src/agent_engine.rs` — 持續性 REGISTER（壓測時每帳號一個 task）

**民眾模式（Caller）依然不帶 INVITE 認證**。原因：壓測場景下，測試 SIP 伺服器端通常會將測試來源 IP 加入白名單；INVITE 帶 Digest 會大幅增加每通通話的訊息往返（每通要多 2 條訊息），影響最高 CPS。需要時可在 engine.rs 加入「收到 401/407 自動重送 INVITE 帶 Authorization」的邏輯，沿用 `register.rs` 中的 `DigestChallenge`。

### C. AtomicU64 vs Mutex 的選擇

- 頻繁讀寫的計數器（calls_initiated 等）→ `AtomicU64`（無鎖，最快）
- 需要條件讀寫或複合操作的狀態（snapshot、stop_tx）→ `Mutex`
- 需要可變借用且不能 Clone 的資料（Histogram）→ `Mutex`

### D. tokio::Mutex vs std::sync::Mutex

- `tokio::Mutex`：跨 await 持有 lock 時使用（engine 中的 `dialogs`、`rtp_sessions`）
- `std::sync::Mutex`：不跨 await 的快速操作（commands.rs 中的 `AppState`）

`std::sync::Mutex` 性能更好，但 guard 不能跨 `.await`。誤用會導致 `Future is not Send` 編譯錯誤。

### E. 為什麼不使用 SRTP / TLS？

sipress 目前不支援 SRTP（加密 RTP）或 SIP over TLS，原因：
1. 企業內網壓測不需要加密
2. 加密解密增加 CPU 開銷，影響最高 CPS 測量
3. TLS 憑證管理增加部署複雜度

若有需求，TLS 支援可透過在 `Transport::Tls` 分支加入 `tokio-rustls` 實作。

### F. 大量並發通話的記憶體估算

每個 `Dialog` 約佔 200-400 bytes（包含 `String` 欄位）。
10,000 並發通話 ≈ 4 MB。在現代機器上不成問題。

RTP 每個 Session 有兩個 tokio task stack（預設 2MB 每 task），但 tokio 使用動態 stack，實際開銷遠小於此。

### G. SIP Dialog 的 HashMap 競爭

引擎主控迴圈中有三個地方鎖定 `dialogs`：
1. 事件處理（①）
2. 逾時掃描（②）
3. 發起新通話（③）

這三步是**順序執行**（不是並發），每次都 `lock().await` 然後立即 drop。接收 Task 透過 channel 傳遞事件，不直接存取 `dialogs`，所以不存在真正的競爭。

### H. TOCTOU 競態消除的設計決策

**問題**：早期 `allocate_port()` bind 成功後立即釋放 socket，再由 `RtpSession::start()` 重新 bind。在高 CPS 壓測下，兩通通話的 port 分配幾乎同時發生，中間的極短 race window（通常 < 1µs）足以讓另一通 call 的 bind 搶先佔用同一個 port，導致 EADDRINUSE 錯誤。

**解法**：`allocate_port()` 改為回傳 `(u16, UdpSocket)`，socket 保持綁定狀態存入 `pre_bound_rtp` HashMap，直到 200 OK 時才移交給 `RtpSession::start(pre_bound: Some(socket))`。從 bind 成功到 connect 對端，port 從未被釋放，競態完全消除。

**代價**：每通進行中的 call 額外持有一個已綁定但未 connect 的 UdpSocket（約 200 bytes kernel 資源）。10,000 並發下約 2MB kernel socket buffer，可接受。

### I. RFC 3261 合規：ACK 的 Request-URI

**問題**：sipress 早期的 ACK Request-URI 使用原始 To header 中的 URI（INVITE 的 Request-URI）。RFC 3261 §12.2.1.1 明確規定 **2xx ACK 必須使用 Contact URI**，因為 200 OK 的 Contact 可能反映 UAS 在 NAT 後的真實地址（與 To URI 不同）。

**解法**：
1. `SipParser::contact_uri()` 從 200 OK 解析 Contact header，剝除角括號與 URI 參數
2. 引擎將其存入 `dialog.remote_contact: Option<String>`
3. `SipMessage::ack()` 接受 `contact_uri: Option<&str>` 參數；有值時作為 Request-URI

**Non-2xx ACK**（RFC 3261 §17.1.1.3）：收到 4xx/5xx/6xx 的 ACK 是事務層行為，Request-URI 保持與 INVITE 相同，**不**使用 Contact URI（傳入 `None`）。

### H. 壓測工具本身的性能上限

在 3GHz 單核的機器上，sipress 能穩定達到約：
- 純 SIP 信令：~2000 CPS
- SIP + RTP（靜音）：~500 CPS
- SIP + RTP（音檔）：~200 CPS（受音檔解碼速度限制）

瓶頸主要在：
1. 主控迴圈的 500µs 睡眠（2000 CPS 上限）
2. UDP 封包的系統呼叫開銷
3. RTP 音訊解碼（啟用音檔時）

### I. 座席模式為何每帳號獨立 socket？

替代方案是「全部帳號共用一個 SIP socket，依 To header 的 username 把進來的 INVITE 路由到對應的 dialog」。但：

- **REGISTER 與後續的 INVITE Contact 標頭一致性**：每個帳號 REGISTER 時的 Contact 必須是「我的可達地址」。共用 socket 的話所有帳號共用同一個 `ip:port`，理論上可行，但大部分 SBC / 軟交換機會懷疑這種行為。
- **dialog 清晰度**：每帳號獨立 socket = 每帳號獨立的 SIP UA，內部狀態彼此完全隔離，debug 容易。
- **代價**：N 個帳號 = N 個 fd。Linux 預設 1024 fd，1000 個座席仍在範圍內；超過時可 `ulimit -n` 提升。

### J. 座席模式的 RTP 實作

座席模式現已支援真實 RTP 收發，與民眾模式共用 `RtpSession` 基礎設施。主要差異：

1. **port 分配時機不同**：民眾模式在 INVITE 前預分配（寫入 SDP offer），座席模式在收到 INVITE 後分配（寫入 200 OK SDP answer）
2. **遠端地址來源**：民眾模式從 200 OK SDP 解析，座席模式從 INVITE SDP 解析
3. **共用 `rtp_port_counter`**：所有 runner 共用同一個 port 計數器，避免 port 衝突
4. 當 `enable_rtp = false` 時，退回到 `pick_dummy_rtp_port()` 隨機數字模式（純 SIP 信令壓測，不佔 port 資源）

---

## 附錄：常見錯誤與排查

| 錯誤訊息 | 原因 | 解法 |
|---------|------|------|
| `state not managed for field 'state'` | `main.rs` 有獨立 Builder 不含 `.manage()` | 讓 `main.rs` 只呼叫 `lib::run()` |
| `MutexGuard is not Send` | std Mutex guard 跨 `.await` | 在 `.await` 前 drop guard |
| `Future is not Send` | tokio::spawn 的 async block 持有非 Send 型態 | 改用 tokio::Mutex 或縮短 lock 範圍 |
| `無法找到可用的 RTP port` | 系統 RTP port 耗盡（>4000 個） | 降低並發數或增加 rtp_base_port 範圍 |
| `SIP log 建立失敗` | logs/ 目錄不存在或無寫入權限 | 程式已 fallback 到 temp 目錄，檢查終端機輸出 |
| GUI 顯示空白 | Tauri WebView 設定問題 | 刪除 `%APPDATA%\com.leozh.sipress` 重啟 |
| 視窗無法拖曳/最小化 | Tauri capabilities 缺少 window 權限 | 確認 `capabilities/default.json` 有 `core:window:allow-*` |
| 所有通話 408 Timeout | 伺服器不可達（這是正確行為） | 確認 SIP 伺服器地址正確且防火牆開放 |
| 座席帳號註冊一直 ERR (401) | 密碼錯 / Digest 計算對不上 | 確認密碼大小寫；查 SIP log 中 `WWW-Authenticate` 格式是否標準 |
| 接通的通話幾秒被砍 | 過去版本忽略 RE-INVITE 導致伺服器砍 dialog | 已修補；查 SIP log 確認有 `回應 200 OK` 的 RE-INVITE 紀錄 |
| 民眾模式錄音檔只有開頭 | 同上（Session-Expires 沒回應）| 已修補；如還發生請貼 SIP log 上來 |
| 座席模式按開始跑成民眾模式 | Config 沒帶 `mode` 欄位 | 已修補；確認 `buildRustConfig()` 有 `mode: c.mode` |
| RE-INVITE 後 RTP 送到舊 port | Engine 收到 RE-INVITE 時只回 200 OK，沒更新 RTP 目標 | 已修補；RE-INVITE handler 現在解析 SDP 並呼叫 `RtpSession::update_remote()` |
| 按 Stop 後交換機不馬上掛線 | Caller 模式按 Stop 直接 drop Engine，沒發 BYE/CANCEL | 已修補；改用 `stop_handle()` graceful stop，會對所有通話發 BYE/CANCEL |
| RTP 完全沒收到但 MOS 顯示優良 | 零收包時 `expected=0` → `loss=0%` → `MOS≈4.4` | 已修補；有送無收時以 sent 為基準回報 100% loss |
| RE-INVITE 後 RTP 掉包率異常高（>90%） | RE-INVITE 換了媒體伺服器，SSRC 改變導致序號空間不連續，被誤判為大量掉包 | 已修補；`RtpStats` 追蹤 SSRC 變更，每個 SSRC 期間獨立統計後合併 |

---

如有疑問或發現文件有誤，歡迎在 Repository 開 Issue。
