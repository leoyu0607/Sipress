# sipress — SIP 軟體交換機壓測工具

> 高效能、跨平台的 SIP UAC 壓測工具，提供視窗 GUI、終端機 TUI 與 CLI 三種操作介面，支援真實 RTP 音訊傳送、聲音品質分析與 HTML 報告產出。

[![Rust](https://img.shields.io/badge/Rust-2021-orange)](https://www.rust-lang.org)
[![Tauri](https://img.shields.io/badge/Tauri-2-blue)](https://tauri.app)
[![License: MIT](https://img.shields.io/badge/License-MIT-green)](LICENSE)

## 目錄

- [功能特色](#功能特色)
- [取得執行檔](#取得執行檔)
- [專案結構](#專案結構)
- [介面說明](#介面說明)
- [SIP 通話流程](#sip-通話流程)
- [座席模式（Agent Mode）](#座席模式agent-mode)
- [RTP 音訊流程](#rtp-音訊流程)
- [關鍵指標](#關鍵指標)
- [CLI 快速開始](#cli-快速開始)
- [CLI 參數完整說明](#cli-參數完整說明)
- [輸出格式](#輸出格式)
- [SIP Log](#sip-log)
- [效能](#效能)
- [建置說明](#建置說明)
- [模組說明](#模組說明)

---

## 功能特色

| 功能 | 說明 |
|------|------|
| **視窗 GUI** | Tauri 桌面應用，無邊框深色主題，即時圖表與 SIP 日誌顯示 |
| **終端機 TUI** | ratatui 儀表板，顯示即時 ASR、CPS、並發數（`--tui`） |
| **雙模式壓測** | **民眾端**（主動撥出）/ **座席端**（REGISTER 後等待來電並自動接聽） |
| **SIP 信令** | INVITE / 100 / 180 / 200 / ACK / BYE / CANCEL（RFC 3261）；可處理伺服器主動 RE-INVITE（Session-Expires）與 BYE |
| **Digest 認證** | 座席 REGISTER 與民眾端 INVITE 都支援 RFC 2617 Digest（401 → `Authorization`、407 → `Proxy-Authorization`）；REGISTER 自動刷新 |
| **真實 RTP** | 每 20ms 傳送 G.711 **PCMA**（A-law）封包，支援 WAV 音檔自動轉檔（PCM16/A-law/μ-law → PCMA）|
| **聲音品質分析** | MOS 估算（ITU-T E-Model G.107）、掉包率、Jitter（RFC 3550） |
| **總通數上限** | 民眾模式可設定 `--max-calls N`，達上限後自動停止（不依時長）；GUI 前端自動偵測引擎完成並停止 |
| **即時計數** | 儀表板同步顯示成功通數、失敗通數、佇列通數、Error Rate |
| **即時 RTP 品質** | 測試期間即時顯示 MOS / 掉包率 / Jitter / 封包統計，不需等測試結束 |
| **HTML 報告** | 含環形指標圖、延遲分位數長條圖、RTP 品質區塊，可離線檢視；GUI 與 CLI 均支援 |
| **GUI 匯出** | TitleBar 一鍵匯出 JSON / CSV / **HTML** 三種格式 |
| **SIP Log** | 每次壓測自動產生帶時間戳記的完整 SIP 訊息 log |
| **靜態編譯** | cargo-zigbuild 交叉編譯，無需 Docker，支援 Linux / Windows / macOS |
| **多輸出格式** | Table / JSON / CSV，可 pipe 給其他工具 |

---

## 取得執行檔

### 直接下載（推薦）

前往 [Releases](../../releases) 下載對應平台的版本，所有檔案均在 `dist/` 目錄：

| 平台 | 安裝版 | 免安裝版 |
|------|--------|---------|
| Windows | `sipress-gui-windows-x86_64-installer.msi` | `sipress-gui-windows-x86_64-portable.exe` |
| Windows | `sipress-gui-windows-x86_64-setup.exe`（NSIS） | — |
| Linux | `sipress-gui-linux-x86_64-installer.deb` | `sipress-gui-linux-x86_64-portable.AppImage` |
| macOS ARM | `sipress-gui-macos-arm64-installer.dmg` | `sipress-gui-macos-arm64-portable` |

> **CLI 執行檔**（無 GUI）：
> - Windows：`sipress-windows-x86_64-native.exe`（MSVC）或 `sipress-windows-x86_64.exe`（GNU zigbuild）
> - Linux：`sipress-linux-x86_64`
> - macOS：`sipress-macos-arm64`

### 自行建置

請參閱 [建置說明](#建置說明)。

---

## 專案結構

```
sipress/
├── Cargo.toml                    ← workspace（core + cli + gui）
├── build.sh                      ← Linux/macOS 建置腳本
├── build.ps1                     ← Windows PowerShell 建置腳本
├── build.bat                     ← Windows 命令提示字元入口
│
├── core/                         ← 核心 library（無 UI，CLI 與 GUI 共用）
│   └── src/
│       ├── config.rs             ← 設定結構（Config / Transport / Mode / AgentAccount）
│       ├── engine.rs             ← 民眾端壓測主引擎（並發通話控制）
│       ├── agent_engine/         ← 座席端壓測引擎（REGISTER + 等待來電 + 自動接聽）
│       │   ├── mod.rs            ← AgentEngine：啟動每個帳號的 runner、進度回報、最終報告
│       │   ├── runner.rs         ← 單一帳號的狀態（socket / 註冊 / 通話 / 響鈴中來電）與事件處理
│       │   ├── register.rs       ← 註冊狀態、REGISTER 重傳、已註冊計數
│       │   └── call.rs           ← 通話狀態、座席行為的隨機決定、SIP 訊息組裝
│       ├── registrar.rs          ← 一次性 REGISTER 握手（Phase 1：新增帳號立即驗證）
│       ├── stats.rs              ← 指標收集（ASR/ACD/PDD/延遲/即時 RTP 品質/finished 旗標）
│       ├── reporter.rs           ← 終端機輸出（Table / JSON / CSV）
│       ├── html_reporter.rs      ← HTML 報告產生器
│       ├── sip_logger.rs         ← SIP 完整訊息 log 記錄器
│       ├── sip/
│       │   ├── message.rs        ← SIP 訊息建構（INVITE/ACK/BYE/CANCEL/200 OK 鏡射回應）
│       │   ├── register.rs       ← REGISTER 訊息 + Digest auth (RFC 2617) 解析與計算
│       │   ├── dialog.rs         ← SIP 對話狀態機
│       │   ├── parser.rs         ← SIP 回應解析
│       │   └── transport.rs      ← UDP / TCP 傳輸層（TCP 依 Content-Length 分框）
│       └── rtp/
│           ├── audio.rs          ← WAV → PCMA（G.711 A-law）自動轉檔；支援 PCM16 / A-law / μ-law
│           ├── packet.rs         ← RTP 封包建構與解析（RFC 3550）
│           ├── session.rs        ← Per-call RTP session（port 分配、收發）
│           └── stats.rs          ← Jitter / 掉包率 / MOS 計算
│
├── cli/                          ← 終端機 CLI + TUI 儀表板
│   └── src/
│       ├── main.rs
│       ├── args.rs               ← clap 參數定義
│       └── tui/dashboard.rs      ← ratatui 即時儀表板
│
└── gui/                          ← Tauri 桌面 GUI
    ├── src-tauri/
    │   ├── Cargo.toml
    │   ├── tauri.conf.json       ← 無邊框視窗、bundle 設定
    │   └── src/
    │       ├── lib.rs            ← Tauri 應用入口
    │       └── commands.rs       ← start_test / stop_test / get_snapshot / get_report
    └── src/                      ← Vue 3 + TypeScript 前端
        ├── components/           ← TitleBar / Sidebar / ChartPanel / LogPanel…
        └── stores/testStore.ts   ← 狀態管理 + Tauri invoke 橋接
```

---

## 介面說明

### GUI（視窗模式）

執行 `sipress-gui-*-portable.exe` 或安裝後開啟，可看到：

- **左側 Sidebar**：填寫伺服器位址、CPS、並發數、**總通數上限**、持續時間（**0 = 不限時間**）、音檔路徑
- **頂部 TitleBar**：顯示狀態、進度條，▶ Start / ■ Stop 按鈕，及 **↓ JSON / ↓ CSV / ↓ HTML** 匯出按鈕（HTML 需測試完成後才啟用）
- **中間 MetricStrip**：即時顯示 CPS、CONCUR、**SUCCESS（成功通數）**、**FAILED（失敗通數）**、**TOTAL（累計發起）**、ASR、**ERR%（Error Rate）**、PDD
- **圖表區**：折線圖（CPS / ASR / CCR / ERR 趨勢）
- **右側面板**：回應碼統計、**即時** RTP 品質（**MOS / 掉包率 / Jitter / 封包統計**，測試中即時更新）、SIP flow 時序
- **底部 LogPanel**：即時 SIP 事件日誌（color-coded）

> 詳細使用步驟請參閱 [howtouse.md](howtouse.md)

### TUI（終端機圖形介面）

```bash
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60 --tui
```

按 `q` 或 `Esc` 退出（壓測仍在背景執行，結束後輸出報告）。

### CLI（純文字輸出）

```bash
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60
```

---

## SIP 通話流程

### 民眾端（Caller mode，主動撥出）

```
UAC (sipress)              UAS (軟交換機)
     │                          │
     │──── INVITE ─────────────▶│  本機 RTP port 預分配並寫入 SDP m= 行
     │◀─── 100 Trying ──────────│
     │◀─── 180 Ringing ─────────│  PDD 計時結束
     │◀─── 200 OK ──────────────│  通話建立，解析 SDP c=+m= RTP 地址 + Contact URI
     │──── ACK ────────────────▶│  Request-URI = Contact URI（RFC 3261 §12.2.1.1）
     │                          │
     │═══ RTP G.711 音訊流 ════▶│  每 20ms 一個 160-byte PCMU/PCMA frame
     │◀══ RTP G.711 音訊流 ═════│  計算 Jitter / 掉包 / MOS（RFC 3550 §A.3/§A.8）
     │                          │
     │◀─── RE-INVITE ───────────│  Session-Expires 會話刷新（可能變更 RTP port）
     │──── 200 OK + SDP ───────▶│  自動回應保活 + 解析新 SDP 更新 RTP 目標地址
     │                          │
     │──── BYE ────────────────▶│  通話持續時間計時結束，停止 RTP
     │◀─── 200 OK ──────────────│
     │
     │  （或：伺服器主動掛斷時）
     │◀─── BYE ─────────────────│
     │──── 200 OK ─────────────▶│  自動回應，記錄通話結束
     │
     │  （INVITE 逾時未收到回應時）
     │──── CANCEL ─────────────▶│  RFC 3261 §9
     │
     │  （收到 4xx/5xx/6xx 時）
     │──── ACK ────────────────▶│  使用原始 INVITE branch（RFC 3261 §17.1.1.3）
```

---

## 座席模式（Agent Mode）

座席模式模擬一群 SIP 話機（座席分機）：對交換機 REGISTER 後等待來電，收到 INVITE 自動接聽，是壓測「座席端負載」的工具。

### 流程概觀

```
[GUI 切到「座席端」]
       │
       ├── 在 Sidebar 「快速生成帳號」或拖曳 CSV 匯入
       │       │
       │       ▼
       │   每個帳號獨立呼叫 register_account（一次性驗證）
       │   ─→ status 變成 'registered' / 'failed'
       │
[按「開始測試」]
       │
       ▼
   AgentEngine::run()
       │
       ├── 為每個 AgentAccount spawn 一個 task：
       │       1. REGISTER（含 Digest 重送）
       │       2. 等待 INVITE → 100 Trying → 180 Ringing → 200 OK + SDP（PCMA）
       │       3. 等待 ACK（10s timeout → 主動 BYE）
       │       4. 啟動真實 RTP 收發（若 enable_rtp）
       │       5. 處理 RE-INVITE（更新 RTP 目標）/ BYE / CANCEL / OPTIONS
       │       6. BYE timeout（150s → 主動 BYE）
       │       7. 在 Expires/2 計時器到期時 re-REGISTER（最少每 60 秒）
       │       8. （選項 --reregister-after-call）通話結束後立即 re-REGISTER
       │
       └── 測試時間到 → 通知所有 task 發 REGISTER Expires=0 → 結束
```

### 帳號清單格式

GUI 可拖曳 CSV / TXT 檔，支援以下三種格式（首行若是標頭會自動跳過）：

```csv
extension,username,password,domain
1001,user1001,pass1001,sip.example.com
1002,user1002,pass1002,sip.example.com
```

```
1001:pass1001:sip.example.com
1002:pass1002:sip.example.com
```

```
1001 user1001 pass1001 sip.example.com
1002 user1002 pass1002 sip.example.com
```

### 帳號狀態

| 狀態（badge）| 含義 |
|---|---|
| `—` | idle，剛建立尚未註冊 |
| `…` | registering，REGISTER 進行中 |
| `REG`（綠）| 已註冊成功 |
| `ERR`（紅）| 認證失敗 / 拒絕 / 逾時，Event Log 會顯示具體 SIP 狀態碼 |

按 Sidebar 的「重新註冊」可對所有非 registered 帳號再試一次。

### 自動處理的 SIP 訊息

座席模式下，sipress 會處理以下伺服器主動發來的訊息：

| 收到的請求 | 自動回應 |
|---|---|
| `INVITE` 新通話 | `100 Trying` → `180 Ringing` →（響鈴延遲）→ `200 OK` + SDP（PCMA）；啟動真實 RTP（若 enable_rtp）。依行為模擬也可能回 `486`（拒接 / 休息中）或一直響鈴不接 |
| `INVITE` （已建立 dialog） | RE-INVITE 視為保活，回 `200 OK` + SDP；更新 RTP 目標地址 |
| `ACK` | 標記通話已建立（10s 未收到 → 座席主動 BYE） |
| `BYE` | `200 OK`，停止 RTP，計入 calls_completed |
| （BYE timeout） | 通話 150s 未收到 BYE → 座席主動送 BYE 掛斷 |
| `CANCEL` | 響鈴中：`200 OK`（CANCEL）+ `487 Request Terminated`（INVITE）；已接聽：`200 OK` 且不影響通話（等 BYE） |
| （主動掛斷） | 設定 `--talk` 時，接聽後隨機秒數由座席送 BYE |
| `OPTIONS` | `200 OK`（健康檢查） |

### 座席端 RTP

座席模式現已支援真實 RTP 收發（與民眾模式共用 `RtpSession` 基礎設施）：
- 當 `enable_rtp = true` 時，接聽來電時分配真實 RTP port、啟動 G.711A 音訊傳送與品質統計
- 進度 task 每秒聚合所有活躍 session 的 MOS / 掉包率 / Jitter，前端即時顯示
- RE-INVITE 時自動更新 RTP 目標地址
- 註冊只在 Expires 到期前刷新；交換機要求通話後重新註冊才會繼續派話時，開啟 `--reregister-after-call`（GUI「通話後重新註冊」）

### 注意事項

- 測試前用 GUI 「快速生成帳號」做的 `register_account` 是短期 socket，與壓測時 `AgentEngine` 持有的 socket 是兩條線；同帳號雙路 Contact 在嚴格的交換機上可能會被拒。建議：壓測前先確認帳號可註冊（看 badge 變綠），開始測試後不要再按「重新註冊」。



```
sipress (UAC for REGISTER, UAS for INVITE)        軟交換機
     │                                                  │
     │──── REGISTER ───────────────────────────────────▶│
     │◀─── 401 Unauthorized + WWW-Authenticate ─────────│  Digest challenge
     │──── REGISTER + Authorization (Digest MD5) ──────▶│
     │◀─── 200 OK (Expires=N) ──────────────────────────│  註冊成功，Expires/2 後 re-REGISTER
     │                                                  │
     │           （等待外部撥入）                       │
     │                                                  │
     │◀─── INVITE (sip:agent@host) ─────────────────────│  外部使用者來電
     │──── 100 Trying ─────────────────────────────────▶│
     │──── 180 Ringing ────────────────────────────────▶│
     │──── 200 OK + SDP (PCMA, 真實 RTP port) ────────▶│  自動接聽
     │◀─── ACK ─────────────────────────────────────────│  （10s 未收到 → 主動 BYE）
     │                                                  │
     │═══ RTP G.711 音訊流（若 enable_rtp）════════════▶│
     │◀══ RTP G.711 音訊流 ═════════════════════════════│
     │                                                  │
     │◀─── RE-INVITE ──────────────────────────────────│  Session-Expires 保活
     │──── 200 OK + SDP ───────────────────────────────▶│  更新 RTP 目標地址
     │                                                  │
     │◀─── BYE ─────────────────────────────────────────│  對方掛斷（或 150s timeout → 主動 BYE）
     │──── 200 OK ─────────────────────────────────────▶│  停止 RTP
     │                                                  │
     │──── REGISTER (re-register) ─────────────────────▶│  Expires/2 到期時刷新（開啟選項時通話後也會送）
     │◀─── 200 OK ──────────────────────────────────────│
     │                                                  │
     │           （測試結束）                           │
     │──── REGISTER (Expires=0) ───────────────────────▶│  解除註冊
     │◀─── 200 OK ──────────────────────────────────────│
```

---

## RTP 音訊流程

```
音檔（.wav / .al / .ul / .raw）
       │
       ▼
   AudioSource::from_file()    自動偵測格式並轉換為 G.711A (PCMA)
   ┌────────────────────────────────────────────────────┐
   │  WAV format=1 (PCM16)  → linear_to_alaw()          │
   │  WAV format=6 (A-law)  → 直接讀取                  │
   │  WAV format=7 (μ-law)  → ulaw_to_linear → A-law    │
   │  raw .al / .alaw       → 直接讀取                  │
   │  raw .ul / .ulaw       → μ-law → A-law             │
   │  → 切成 160-byte frames（20ms @ 8kHz）             │
   │  → 循環播放（解碼結果快取，所有通話共用）           │
   └────────────────────────────────────────────────────┘
       │
       ▼
   協商出 PCMU 時逐 frame 轉成 μ-law（alaw_to_ulaw 查表）
       │
       ▼
   RtpPacket::encode()  PT=協商結果（8 / 0）  seq++  ts+=160  SSRC=random
       │
       ▼
   UdpSocket::send()    傳送至 SDP c= IP + m= port

接收端（同時執行）：
   UdpSocket::recv() → RtpPacket::decode()
       → RtpStats::on_recv()
           → Jitter（RFC 3550 §A.8）
           → 掉包率 = 1 − (recv / expected)
           → MOS（ITU-T E-Model G.107 簡化版）
```

### 支援音檔格式

| 格式 | 說明 |
|------|------|
| `.wav` (PCM16) | 16-bit mono / stereo，自動轉成 G.711A |
| `.wav` (A-law)  | format=6（PCMA），直接使用 |
| `.wav` (μ-law)  | format=7（PCMU），自動轉成 PCMA |
| `.al` / `.alaw` / `.pcma` | 原始 G.711 A-law，8kHz mono |
| `.ul` / `.ulaw` / `.pcmu` / `.raw` | 原始 G.711 μ-law，自動轉成 A-law |

音檔解碼後快取為 G.711A；實際送出的 codec 由 SDP 協商決定，選到 PCMU 時送出前轉成 μ-law，封包的 payload type 與內容一致。
音檔格式不支援或檔案損壞時，壓測在開始前就會失敗並顯示原因（不會改送靜音）。

### Codec 協商

| 角色 | 行為 |
|---|---|
| 民眾端（offer） | `m=audio PORT RTP/AVP <偏好> <另一個 G.711> 101`，依 200 OK 的 answer 決定送 PT 8 或 0 |
| 座席端（answer） | offer 有偏好的 codec 就用它，否則用 offer 中第一個 G.711；answer 只宣告選中的 codec（offer 有 101 時一併接受）；offer 完全沒有 G.711 → `488 Not Acceptable Here` |
| RE-INVITE | 帶 SDP 時只回目前的 codec；不帶 SDP 時 200 OK 以 offer 格式列出所有 codec |

偏好的 codec 即 GUI 的 Codec 設定（CLI `--codec pcma|pcmu`，預設 PCMA）。

---

## 關鍵指標

> 每個指標的完整計算邏輯（觸發事件、公式、聚合方式、即時值與最終報告的差異、已知限制）請見 **[METRICS.md](METRICS.md)**。

### SIP 指標

| 指標 | 說明 | 計算方式 |
|------|------|---------|
| **CPS** | Calls Per Second，每秒發起通話數 | 即時：每秒 `calls_initiated` 差值；報告：`calls_initiated / 測試時長` |
| **CONCUR** | 目前進行中通話數 | `calls_active`（INVITE +1，完成 / 失敗 / 逾時 −1） |
| **SUCCESS** | 已接通通數 | `calls_answered`（累計） |
| **FAILED** | 失敗通數 | `calls_failed + calls_timeout`（累計） |
| **TOTAL** | 累計發起通數 | `calls_initiated` |
| **ASR** | Answer Seizure Ratio，接通率 | `calls_answered / calls_initiated × 100%` |
| **CCR** | Call Completion Rate，完成率 | `calls_completed / calls_initiated × 100%` |
| **ERR%** | Error Rate，失敗率 | `(calls_failed + calls_timeout) / calls_initiated × 100%` |
| **PDD** | Post Dial Delay，撥號後延遲 | INVITE 送出 → 收到 180 / 183（ms）；即時顯示平均、報告顯示 p50/p95/p99/max |
| **Setup Time** | 通話建立時間 | INVITE 送出 → 收到 200 OK（ms） |
| **ACD** | Average Call Duration，平均通話時長 | 200 OK → 通話結束的平均（秒） |

### RTP / 聲音品質指標

| 指標 | 說明 | 標準 |
|------|------|------|
| **MOS** | Mean Opinion Score，1.0 ~ 5.0（本公式最高約 4.41） | ≥ 4.0 優、≥ 3.0 普通、< 3.0 差 |
| **掉包率** | (Expected − Received) / Expected，依序號空間計算（RFC 3550 §A.3） | 建議 < 1%，可接受 < 3% |
| **Jitter** | 封包到達時間抖動（ms，RFC 3550 §A.8） | 建議 < 30ms |

### MOS 估算公式（ITU-T E-Model 簡化）

```
Ie_eff = 0 + (95 − 0) × Ppl / (Ppl + 4.3)    ← 掉包影響（G.711 Bpl=4.3）
Id     = max(0, jitter_ms − 150) × 0.1         ← Jitter 影響（>150ms 才算）
R      = 93.2 − Ie_eff − Id
MOS    = 1 + 0.035R + R(R−60)(100−R) × 7×10⁻⁶  ← ITU-T G.107 §B.4
```

---

## CLI 快速開始

```bash
# 最簡單：對 192.168.1.100:5060 發起 100 並發、10 CPS、持續 60 秒
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60

# 帶 TUI 即時儀表板
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60 --tui

# 啟用 RTP + 播放音檔
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60 \
  --rtp --audio /path/to/sample.wav

# 輸出 HTML 報告
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60 --html

# JSON 輸出（適合 CI/CD 整合）
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60 --format json

# CSV 輸出（匯入試算表）
./sipress -s 192.168.1.100:5060 -c 100 --cps 10 -d 60 --format csv > result.csv

# 座席端：用帳號檔註冊 N 支分機並自動接聽，-d 0 = 直到 Ctrl-C（結束時解除註冊）
./sipress -s 192.168.1.100:5060 --mode agent --accounts accounts.csv -d 0 --tui
```

---

## CLI 參數完整說明

### SIP 連線

| 參數 | 簡短 | 預設 | 說明 |
|------|------|------|------|
| `--server` | `-s` | `127.0.0.1:5060` | 目標 SIP 伺服器（`ip:port`） |
| `--local` | — | 自動 | 本機綁定 IP 或 IP:port（例如 `0.0.0.0:5070`、IPv6 為 `::1` 或 `[::1]:5070`）；只給 IP 時 port 由系統分配 |
| `--domain` | — | 自動 | 本機 SIP domain（用於 From header） |

### 通話控制

| 參數 | 簡短 | 預設 | 說明 |
|------|------|------|------|
| `--concurrent` | `-c` | `100` | 最大並發通話數 |
| `--cps` | — | `10.0` | 每秒發起通話數 |
| `--cps-ramp` | — | — | CPS 漸增 `START-END`：在 `--duration` 內從 START 線性升到 END（取代 `--cps`，需 `-d` > 0） |
| `--cps-steps` | — | — | CPS 階梯 `CPS:秒,CPS:秒,...`：依序維持各階，最後一階維持到結束 |
| `--cps-spike` | — | — | CPS 尖峰 `基準,尖峰,開始%,尖峰秒數`，例如 `10,100,50,10`（需 `-d` > 0）；三種模式擇一 |
| `--duration` | `-d` | `60` | 壓測持續時間（秒）；**設為 `0` = 不限時間**，需搭配 `--max-calls` 或手動停止 |
| `--max-calls` | — | 不限 | 總通話上限，達到且所有進行中通話結束後自動停止 |
| `--call-duration` | — | `30` | 單通通話持續時間（秒，`0` = 不主動 BYE） |
| `--invite-timeout` | — | `8` | INVITE 逾時秒數 |

### 座席模式

| 參數 | 預設 | 說明 |
|------|------|------|
| `--mode` | `caller` | `caller`（民眾端，主動撥出）/ `agent`（座席端，註冊後等待來電並自動接聽） |
| `--accounts` | — | 座席帳號檔（agent 模式必填），一行一個帳號：`extension,username,password[,domain]`（可有標題列）、`extension:password[:domain]` 或空白分隔；GUI 帳號清單的「匯出 CSV」可直接使用 |
| `--account-domain` | SIP server IP | 帳號未填 domain 時使用的預設值 |
| `--ack-timeout` | `10` | 接聽後等待 ACK 的逾時秒數（逾時主動 BYE） |
| `--bye-timeout` | `150` | 通話中等待對方 BYE 的逾時秒數（逾時主動 BYE） |
| `--ring` | `0` | 響鈴幾秒後才接聽，`N` 或區間 `MIN-MAX`（隨機） |
| `--talk` | `0` | 接聽幾秒後座席主動掛斷，`N` 或 `MIN-MAX`；0 = 等對方 BYE |
| `--reject-pct` | `0` | 拒接比例 %（響鈴後回 486 Busy Here） |
| `--no-answer-pct` | `0` | 不接比例 %（一直響鈴，等 SIPX 逾時 CANCEL 改派；120 秒未取消回 480） |
| `--wrap-up` | `0` | 兩通之間休息秒數，期間來電回 486 |
| `--busy-in-call` | 關閉 | 通話中忙線：已有通話（含響鈴中）時新來電回 486，讓 ACD 改派；關閉時同一帳號可同時接多通 |
| `--reregister-after-call` | 關閉 | 每通結束後立即重新 REGISTER；預設只在 Expires 到期前刷新，高話務下開啟會對交換機多送大量 REGISTER |

### 端到端對照

| 參數 | 預設 | 說明 |
|------|------|------|
| `--from-seq` | 關閉 | 民眾端每通使用不同主叫號碼（從 `--from` 遞增、保留位數），讓主叫號碼配對能一對一 |
| `--trace-file` | — | 結束後把每通的追蹤紀錄（`P-CIN-CallId`、主叫號碼、時間、MOS）寫成 JSON；兩端各存一份 |
| `--correlate A B` | — | 讀兩端的追蹤紀錄（順序不拘）輸出合併報告，不執行壓測；搭配 `--format`、`--html`、`--report-dir` |
| `--correlate-window` | `120` | 以主叫號碼配對時，座席收到來電最晚可在民眾端撥出後幾秒 |

配對先用 SIPX 的 `P-CIN-CallId`（同一通電話的每一段都相同；SIP Call-ID 每段都不同），拿不到時改用主叫號碼 + 時間窗。報告包含漏接率、排隊與分配延遲、ACD 改派與雙向 MOS，GUI 的 `↓ 對照` 按鈕產生同一份報告。

agent 模式下 `--concurrent`、`--cps`、號碼設定等民眾端參數會被忽略；Ctrl-C 或 TUI 按 `q` 會先送 `REGISTER Expires=0` 解除註冊再結束。

### 號碼設定

| 參數 | 預設 | 說明 |
|------|------|------|
| `--from` | `1000` | 主叫號碼 |
| `--to-prefix` | `2` | 被叫號碼前綴 |
| `--to-range` | `9999` | 被叫尾數最大值（隨機） |
| `--auth-user` | 主叫號碼 | 撥號認證帳號（SIPX 對 INVITE 回 401 / 407 時使用） |
| `--auth-pass` | 不認證 | 撥號認證密碼；設定後收到 401 / 407 會帶 Digest 認證（`Authorization` / `Proxy-Authorization`）重送 INVITE 一次，仍被拒才算失敗 |

### RTP 音訊

| 參數 | 預設 | 說明 |
|------|------|------|
| `--rtp` | 關閉 | 啟用 RTP 音訊傳送與接收 |
| `--rtp-port` | `10000` | RTP 起始 port（偶數，每通 call +2） |
| `--audio` | 無（靜音） | 音檔路徑（`.wav` / `.raw` G.711 μ-law） |
| `--codec` | `pcma` | 偏好的 codec（`pcma` / `pcmu`）；實際使用的以 SDP 協商結果為準 |
| `--dtmf` | — | 民眾端接通後經 RTP（RFC 4733）送出的 DTMF，例如 `1,w2000,2,#`（`wN` = 等待 N 毫秒）；需搭配 `--rtp` |

### 輸出與記錄

| 參數 | 預設 | 說明 |
|------|------|------|
| `--tui` | 關閉 | 啟用 ratatui 即時儀表板 |
| `--format` | `table` | 輸出格式：`table` / `json` / `csv` |
| `--html` | 關閉 | 產生 HTML 視覺化報告 |
| `--report-dir` | `reports/` | HTML 報告輸出目錄 |
| `--interval` | `5m` | 分時段統計的時段長度（`300`、`30s`、`5m`、`1h`；`0` = 不分段）。報告附上每段的 CPS、ASR、PDD、MOS、失敗碼；HTML 報告畫成趨勢圖 |
| `--timeline-csv` | — | 結束後把分時段統計寫成 CSV（每段一列） |
| `--logs-dir` | `logs/` | SIP log 輸出目錄 |
| `--verbose` | 關閉 | 顯示詳細 debug log |

---

## 輸出格式

### Table（預設）

```
╔══════════════════════════════════════════════════╗
║              sipress 壓測報告                    ║
╠══════════════════════════════════════════════════╣
║  測試時長          60.0 s                        ║
╠══════════════════════════════════════════════════╣
║  發起通話           600                          ║
║  接通通話           522    ASR    87.00 %         ║
║  完成通話           498    CCR    83.00 %         ║
║  失敗通話            48    CPS     9.87           ║
║  逾時通話            30    ACD    28.40 s         ║
╠══════════════════════════════════════════════════╣
║  PDD p50  142ms   p95  380ms   p99  510ms        ║
╚══════════════════════════════════════════════════╝
```

### HTML 報告

壓測完成後產生於 `reports/YYYYMMDD_HHMMSS_report.html`，包含：

- 核心 KPI 卡片（ASR、CCR、CPS、ACD）
- 環形接通率 SVG 圖
- 延遲分位數長條圖（PDD P50/P95/P99/MAX）
- RTP 聲音品質區塊（MOS、掉包率、Jitter）
- SIP 錯誤碼明細表（4xx / 5xx / 6xx）

---

## SIP Log

每次壓測自動在 `logs/` 目錄建立 `YYYYMMDD_HHMMSS_caller.sip.log`（民眾端）或 `YYYYMMDD_HHMMSS_agent.sip.log`（座席端），記錄完整 SIP 訊息（帶方向箭頭與毫秒時間戳記），壓測結束自動寫入統計摘要。

---

## 效能

本機 loopback 量測（4 vCPU）：沒有 RTP 時 5000 通並發只用約 16%（民眾端）/ 1%（座席端）的單核 CPU；RTP 每路每秒 50 個封包，5000 路約需 3 個核心，建議每核心約 1500–2000 路。方法、完整數據與 `load_bench` 用法見 [`docs/performance.md`](docs/performance.md)。

## 建置說明

> 測試分層、本機執行方式、前端 ↔ Rust 契約 fixtures 與 CI 流程請見 **[TESTING.md](TESTING.md)**。

### 前置需求

| 工具 | 用途 | 安裝 |
|------|------|------|
| Rust + Cargo | 編譯核心與 CLI | `rustup.rs` |
| cargo-zigbuild | 跨平台靜態編譯 | `cargo install cargo-zigbuild` |
| zig / ziglang | cargo-zigbuild linker | `pip install ziglang` |
| Node.js + npm | 建置 Tauri GUI 前端 | `nodejs.org` |

### 建置腳本（一鍵建置）

**Windows：**
```bat
build.bat gui              ← 只建 GUI
build.bat windows-native   ← 只建 CLI
build.bat all              ← 全部（CLI 所有平台 + GUI）
```

**Linux / macOS：**
```bash
bash build.sh gui          # 只建 GUI
bash build.sh linux-x86    # 只建 Linux CLI
bash build.sh all          # 全部
```

### 所有輸出都在 `dist/`

> 不同平台執行 `all` 產生的檔案不同：GUI bundle 只會在當前平台建置，CLI cross-compile 產出固定。

```
dist/
│
│  ── GUI（建置平台決定） ──
├── sipress-gui-windows-x86_64-installer.msi    ← GUI Windows 安裝版（Windows 建置）
├── sipress-gui-windows-x86_64-setup.exe        ← GUI Windows 安裝版 NSIS（Windows 建置）
├── sipress-gui-windows-x86_64-portable.exe     ← GUI Windows 免安裝版（Windows 建置）
├── sipress-gui-linux-x86_64-installer.deb      ← GUI Linux 安裝版（Linux 建置）
├── sipress-gui-linux-x86_64-portable.AppImage  ← GUI Linux 免安裝版（Linux 建置）
├── sipress-gui-macos-arm64-installer.dmg       ← GUI macOS 安裝版（macOS 建置）
├── sipress-gui-macos-arm64-portable            ← GUI macOS 免安裝版（macOS 建置）
│
│  ── CLI（zigbuild 交叉編譯，任何平台皆可產生） ──
├── sipress-linux-x86_64                        ← CLI Linux x86_64 靜態二進位
├── sipress-linux-arm64                         ← CLI Linux ARM64 靜態二進位
├── sipress-windows-x86_64.exe                  ← CLI Windows x86_64（GNU zigbuild）
│
│  ── CLI（native，建置平台決定） ──
├── sipress-windows-x86_64-native.exe           ← CLI Windows MSVC（Windows 建置）
├── sipress-macos-arm64                         ← CLI macOS ARM64（macOS 建置）
└── sipress-linux-x86_64                        ← CLI Linux x86_64（Linux 建置，與 zigbuild 同名）
```

### 僅建置 CLI

```bash
# Debug
cargo build -p sipress

# Release
cargo build -p sipress --release
```

### 僅建置 GUI（開發模式）

```bash
cd gui
npm install
npm run tauri dev   # 開啟視窗，hot-reload 前端
```

---

## 模組說明

### `core/src/engine.rs`

壓測主引擎（Tokio 非同步）：
- **接收 task**：單一 UDP recv 迴圈，以 `SipParser`（支援 compact form）**區分 SIP 回應與請求**；回應（`Response`）與伺服器主動請求（`IncomingRequest`）分別送至 channel
- **進度 task**：每秒觸發 `on_progress` callback（TUI / GUI 更新）
- **主控迴圈**：處理 SIP 事件 → 掃描逾時 → 依 CPS 發起新通話 → 判斷結束
- **Graceful Stop**：透過 `stop_handle()` 回傳 `watch::Sender<bool>`，外部呼叫 `.send(true)` 後引擎停止發起新通話，對所有 `Connected` 通話發 BYE、`Calling/Trying/Ringing` 通話發 CANCEL，等待 BYE 回應後才產生最終報告
- RTP session 在收到 200 OK 後啟動（傳入 `pre_bound` 預分配 socket），BYE 後停止並收集統計
- 進度 task 每秒聚合所有活躍 RtpSession 的 MOS/loss/jitter 填入 `StatsSnapshot`，前端即時顯示
- 主迴圈結束後設定 `engine_finished` AtomicBool → `StatsSnapshot.finished = true`，前端偵測後自動停止
- 2xx ACK 使用 Contact URI 作為 Request-URI；non-2xx ACK 使用原始 INVITE branch
- 收到伺服器 RE-INVITE（Session-Expires）→ 自動 200 OK 保活，並解析 SDP 更新 RTP 目標地址（避免對端換 port 後音訊送錯）；收到伺服器 BYE → 自動 200 OK

### `core/src/sip/message.rs`

手刻 SIP 訊息格式（不依賴外部 crate）：
- `SipMessage::invite()` — 含 SDP，`m=audio PORT RTP/AVP 0 8` 使用預分配的本機 port（同時列出 PCMU/PCMA）
- `SipMessage::ack()` — 2xx ACK 使用 `contact_uri` 作為 Request-URI（RFC 3261 §12.2.1.1）；non-2xx ACK 使用原始 INVITE branch（RFC 3261 §17.1.1.3）
- `SipMessage::cancel()` — RFC 3261 §9 CANCEL
- `SipMessage::ok_for_server_bye(raw_request)` — 對伺服器 BYE 鏡射標頭回 200 OK
- `SipMessage::ok_for_server_reinvite(raw_request, local_addr, rtp_port)` — 對 RE-INVITE 回 200 OK + SDP

### `core/src/sip/parser.rs`

`SipParser` struct 提供所有 SIP header 解析，**完整支援 RFC 3261 Compact Form**（縮寫 header 名稱）：

| Compact | 標準 | 用途 |
|---------|------|------|
| `i:` | `Call-ID:` | 識別對話 |
| `t:` | `To:` | 被叫方（含 to-tag） |
| `f:` | `From:` | 主叫方（含 from-tag） |
| `m:` | `Contact:` | 對端聯絡地址 |
| `v:` | `Via:` | 傳輸路徑 |
| `l:` | `Content-Length:` | Body 長度 |

主要方法：
- `call_id()` — 支援 `i:` 縮寫，確保高負載下能正確識別每通通話
- `contact_uri()` — 從 Contact header 剝除角括號與參數，取出純 SIP URI（供 ACK/BYE 作為 Request-URI）
- `sdp_rtp_port()` — 從 200 OK SDP body 解析對端 RTP port（`m=audio PORT RTP/AVP ...`）

### `core/src/rtp/session.rs`

**TOCTOU 競態消除**：RTP port 在送出 INVITE 前就以 `UdpSocket::bind()` 預佔並保持綁定，收到 200 OK 後直接移交給 `RtpSession::start()`，完全避免「bind → release → rebind」之間的 race window：

```
engine 主控迴圈（送 INVITE 前）:
  (port, socket) = RtpSession::allocate_port(counter, local_ip).await
  pre_bound_rtp.insert(call_id, socket)    ← socket 持續綁定

engine 主控迴圈（收到 200 OK 後）:
  socket = pre_bound_rtp.remove(call_id)   ← 取出已綁定 socket
  RtpSession::start(cfg, counter, Some(socket))  ← 傳入 pre_bound，直接 connect 對端
```

`allocate_port()` 回傳 `(u16, UdpSocket)`（port 號 + 已綁定 socket），確保從分配到啟動期間 port 不被系統回收。

`update_remote(new_addr)` 方法可在 RE-INVITE 後動態切換 RTP 送出目標（對 connected UDP socket 重新 `connect()`），確保對端換 port 後音訊不會送到舊地址。

### `core/src/rtp/stats.rs`

RTP 品質統計，所有計算均符合 RFC 3550 標準：
- **Jitter**（RFC 3550 §A.8）：EWMA 指數加權移動平均，8kHz clock 單位
- **掉包率**（RFC 3550 §A.3）：以序號空間計算，追蹤 `first_seq`、`max_seq`、`seq_cycles`（wrap-around 計數），確保長時通話中序號回繞仍能正確統計 `expected` 封包數。**當完全未收到對端 RTP 但已成功送出封包時，以 sent 為基準回報 100% loss**（避免零收包被誤判為「無損 → MOS 優良」）
- **SSRC 切換追蹤**：RE-INVITE 後對端可能更換 SSRC，序號跳到完全不同的範圍。`on_recv()` 偵測到 SSRC 變更時，將舊 SSRC 的 (expected, received) 累積到 `prior_expected` / `prior_received`，重置序號追蹤後以新 SSRC 重新開始，避免跨 SSRC 序號空間造成假掉包
- **MOS**（ITU-T E-Model G.107）：由掉包率與 Jitter 估算 G.711 通話品質分數（1.0 ~ 5.0）
- **on_send 計數**：僅在 `socket.send()` 成功後才計入，避免送失敗的封包被計為已送出

**TOCTOU 修正**：`allocate_port()` 現在回傳已綁定的 `(port, UdpSocket)` 而非只有 port number，消除「測試 bind → 釋放 → RtpSession 再 bind」之間的 race window。Engine 在 INVITE 前呼叫 `allocate_port()` 並將 socket 暫存於 `pre_bound_rtp: HashMap<call_id, UdpSocket>`；收到 200 OK 後直接移交給 `RtpSession::start(pre_bound)` 使用。

### `gui/src-tauri/src/commands.rs`

六個 Tauri command，前端透過 `invoke()` 呼叫：

| Command | 說明 |
|---------|------|
| `start_test(config)` | 啟動壓測（背景非同步，立即回傳）。依 `config.mode` 自動分派到 `Engine`（Caller）或 `AgentEngine`（Agent） |
| `stop_test()` | 手動停止（graceful）；民眾模式會對所有進行中通話發 BYE / 未接通的發 CANCEL；座席模式會發 `REGISTER Expires=0` 解除註冊 |
| `get_snapshot()` | 取得即時 `StatsSnapshot`（前端每秒輪詢），含即時 RTP 品質與 `finished` 旗標 |
| `get_report()` | 取得最終 `FinalReport`（壓測完成後） |
| `get_html_report(server_addr, timestamp)` | 產生 HTML 報告字串（前端下載為 `.html` 檔）；`server_addr` 會顯示在報告標頭 |
| `register_account(server, domain, username, password, expires, transport)` | 對單一帳號發起一次性 REGISTER（含 401 Digest 重送），用於座席模式新增帳號時的即時驗證 |

### `core/src/agent_engine/`

座席端壓測引擎，每個 `AgentAccount` 一個獨立 tokio task（`runner::AccountRunner`）+ 自帶 socket。每個帳號的狀態只由自己的 task 存取，不需上鎖；主迴圈等待「收到 SIP 訊息 / 註冊刷新 / REGISTER 重傳 / 響鈴與通話計時器 / 停止訊號」之一，交給對應的方法處理：

1. **REGISTER**：初次無認證 → 收 401 解析 `WWW-Authenticate: Digest` → 用 MD5 計算 response → 重送
2. **保持註冊**：在 `Expires/2` 計時器到期時 re-REGISTER（沿用快取的 challenge）
3. **接受來電**：收到 INVITE → 100 Trying → 180 Ringing → 200 OK + SDP（PCMA, 真實 RTP port）
4. **等待 ACK**：10s 未收到 ACK → 座席主動 BYE
5. **真實 RTP**：啟動 `RtpSession` 收發 G.711A 音訊（若 `enable_rtp`），收集 MOS / 掉包 / Jitter
6. **保活**：收到 RE-INVITE（Session-Expires）→ 200 OK + SDP + 更新 RTP 目標地址
7. **掛斷**：收到 BYE → 200 OK → 停止 RTP；150s timeout → 主動 BYE
8. **通話後 re-REGISTER（選項）**：`agent_reregister_after_call` 開啟時，每次通話結束（伺服器 BYE、座席主動掛斷或逾時）都立即 re-REGISTER；預設關閉，只靠第 2 點的定期刷新
9. **CANCEL / OPTIONS**：自動處理（487 / 200 OK）
10. **結束**：收到外部停止訊號 → REGISTER Expires=0 → exit
11. **完成通知**：主迴圈結束後設定 `engine_finished` → `StatsSnapshot.finished = true`，前端自動停止

### `core/src/sip/register.rs`

REGISTER 訊息建構 + RFC 2617 Digest auth：
- `RegisterMessage::build()` — REGISTER 訊息，可選夾帶 `Authorization`。From/To domain 使用完整 `server:port`（與 SIPp 一致），Contact 為簡潔 `<sip:user@ip:port>`（不帶 transport 參數），包含完整 `Allow`（14 個方法）與 `Allow-Events: talk,hold,conference,refer,check-sync`
- `DigestChallenge::parse()` — 從 401/407 回應解析 `realm/nonce/algorithm/qop/opaque`
- `DigestChallenge::build_authorization()` — 計算 MD5 response 並產生 `Authorization` header

支援 `qop=auth`（含 `nc` / `cnonce`）與 RFC 2069 fallback（無 qop）。

### `core/src/registrar.rs`

`register_once()`：對單一帳號發起完整一次 REGISTER 握手（用於 GUI 新增帳號時的即時驗證）：
- 短期 UDP socket，握手結束即關閉
- 自動處理 401 → Digest 重送
- 回傳 `RegisterResult { status, message, expires_secs, sip_code }`
