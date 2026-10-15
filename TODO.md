# sipress 待辦清單

給人與 AI agent 共用的工作清單。開始做之前先讀「協作規則」，做完一項就更新這個檔案。

## 協作規則

- **狀態標記**：`[ ]` 待辦　`[~]` 進行中　`[x]` 完成
- **認領**：開始前把狀態改成 `[~]`，並寫上分支名稱，避免兩個 agent 做同一件事。
- **完成**：改成 `[x]`，附上 PR 編號；把實作中發現的新問題加到清單最後。
- **一項一個分支、一個 PR**：分支從最新的 `main` 開，命名 `feat/…` 或 `fix/…`。
- **先寫測試**：bug 先寫會失敗的測試重現；功能要有測試覆蓋。
- **不要直接推 `main`**：走 PR，CI（Linux / macOS / Windows）全綠才合併；合併由使用者執行。
- 測試分層、契約 fixtures 與 CI 說明見 [TESTING.md](TESTING.md)；計算方式見 [METRICS.md](METRICS.md)。

### 本機開發注意事項

| 項目 | 說明 |
|---|---|
| 前端測試 | `npm test`；Node 25+ 的內建 Web Storage 已在 `vitest.config.ts` 關掉（第 7 項）；CI 用 Node 22 |
| Rust 測試 | 本機沒有 `cargo nextest` 時改用 `cargo test -p sipress-core -p sipress`；GUI 後端在 `gui/src-tauri` 下 `cargo test` |
| Lint | `cargo clippy … -- -D warnings`（core/cli 與 gui 各跑一次），警告即失敗 |
| `cargo fmt` | CI 會檢查（`cargo fmt --all -- --check`），commit 前先跑 `cargo fmt --all` |
| 契約 fixtures | 改了前端送出的設定或 Rust 的 snapshot/report 欄位時，依 TESTING.md 重新產生並一起 commit |
| 發佈 | 改 7 個檔案的版本號（`Cargo.toml` ×3、`package.json`、`package-lock.json`、`tauri.conf.json`、`Cargo.lock`），並寫繁體中文發佈說明 `docs/release-notes/vX.Y.Z.md`（沒有時改用自動產生的英文說明）；commit `Release vX.Y.Z`，合併後推 tag `vX.Y.Z` |

---

## 清單

### [x] 1. GUI「本機 Port」設定沒有作用 — PR #8

`SharedUdpSocket::new` 只取 IP、port 一律綁 0。已修正：`IP:port` 會綁定指定 port，被佔用時報清楚的錯誤；GUI 本地埠只套用在民眾端，0 = 自動。

### [x] 2. 座席註冊狀態回報到即時快照 — PR #9

快照新增 `agents_registered` / `agents_total`（每帳號由 `RegisteredGauge` 追蹤，drop 時自動取消）。座席端檢視的指標列以 REG n/總數 取代 PDD；雙端啟動改為等座席全部註冊才撥號（最多 10 秒，部分成功繼續並警告，全部失敗不撥號）。
「超過 Expires 未刷新 → 取消」只有單元測試（刷新間隔最少 60 秒）。

### [x] 3. 端到端對照報告

用 Call-ID 或主叫號碼把民眾端每通電話和座席端接到的配對，算出：
- 漏接率：民眾端接通、但沒有座席接到的通數
- 排隊與分配延遲：民眾端送出 INVITE 到座席收到 INVITE 的時間
- 雙向音質：兩個方向的 MOS 對照

產出一份合併的 HTML 報告。需要先確認 SIPX 轉送時是否保留可配對的欄位（Call-ID 通常會被 B2BUA 改掉，可能要用主叫號碼加時間窗或自訂 header）。

**SIPX log 分析（acd_twlife_1029.log，2025-10-29 19:38–20:17，16 通，皆為座席 101001 外撥 0966551552 經 Avaya trunk）：**
- `Call-ID`：每一段都換新（B2BUA），不能用來配對。
- `P-CIN-CallId`（例：`C202510291938500A01F29500004082`）：同一通電話在 SIPX 內部所有 INVITE 段都保留，送往外部 trunk 的 INVITE 也帶著；SIPX 送出的 180 / 183 / 200 多數也帶。是最有希望的配對鍵。
- 打給座席的那段（`INVITE sip:000001168168101001@…`）`From` 是對方號碼（0966551552），所以「主叫號碼 + 時間窗」可以當備案。
**SIPX log 分析（mohw_sipx_20260915：acd.log + pcap，民眾 15900000002 撥 168168999 → 分配給 WebRTC 座席 0000021681689999）：**
- 送到座席話機的 INVITE（WSS，`INVITE sip:ejd2lsf9@….invalid;transport=ws`）帶 `P-CIN-CallId: C20260915170613CB4162AB020087F9`，與民眾端那段相同 ✔
- SIPX 回給主叫的 `180 Ringing` 與 `200 OK` 也帶同一個 `P-CIN-CallId` ✔
- 座席段的 `From` 保留主叫號碼（`"15900000002" <sip:15900000002@…>`）✔，Call-ID 照例全部不同。
- 剩下的不確定：這次的主叫是上游 CINcc 節點（203.65.98.171），INVITE 進來時已經帶 `P-CIN-CallId`（ID 內的節點碼也是上游的）。像 sipress 這種不帶此 header 的外部主叫，入口節點應該會自己產生，但 log 中沒有直接證據 → 實作時必須有主叫號碼備案。
- 實作方向：兩端各自記錄每通的 `P-CIN-CallId`（民眾端取自 18x/200，座席端取自 INVITE）、主叫號碼與時間戳；合併時先用 `P-CIN-CallId` 配對，配不到的再用「主叫號碼 + 時間窗」。民眾端可選擇每通使用不同主叫號碼（尾數遞增），讓備案也能一對一配對。

**已完成：** 兩端引擎記錄每通的 `CallTrace`（`core/src/trace.rs`），`core/src/correlate.rs` 先以 `P-CIN-CallId`、再以主叫號碼 + 時間窗配對，算出漏接率、分配 / 接起延遲、ACD 改派、雙向 MOS；`core/src/correlation_html.rs` 產生合併的 HTML 報告。民眾端新增「每通不同主叫號碼」（`caller_number_seq` / `--from-seq`）。CLI：`--trace-file` 寫出一端的紀錄、`--correlate A B` 合併。GUI：TitleBar 的 `↓ 對照`。流程測試 `core/tests/correlation_flow.rs` 以 mock B2BUA 驗證兩種配對。
仍待實測確認：sipress 民眾端直接撥 SIPX 時，入口節點是否會自行產生 `P-CIN-CallId` 並回在 18x / 200（報告的「民眾端 CIN %」會顯示）。

### [x] 4. 座席行為模擬

`Config.agent_behavior`（`AgentBehavior`）：響鈴延遲區間、拒接 %（486）、不接 %（響到 CANCEL → 487，120 秒上限回 480）、接聽後主動掛斷區間、wrap-up 休息秒數（期間來電 486）。座席 runner 新增「響鈴中」狀態，響鈴中的 CANCEL 回 487。CLI 參數 `--ring` / `--talk`（`N` 或 `MIN-MAX`）、`--reject-pct`、`--no-answer-pct`、`--wrap-up`；GUI 座席區塊有對應欄位與驗證，CLI 預覽只帶非預設值。流程測試在 `core/tests/agent_behavior.rs`。
通話中忙線見第 9 項。

### [x] 5. CLI 支援座席模式

新增 `--mode agent`（不再隱藏）、`--accounts <檔案>`、`--account-domain`、`--ack-timeout`、`--bye-timeout`。帳號檔格式與 GUI 匯入相同（解析在 core 的 `AgentAccount::parse_list`，格式錯誤 / 分機重複會指出行號）。CLI 加上 Ctrl-C 與 TUI `q` 的 graceful stop（座席端會先解除註冊）。GUI 的 CLI 預覽在座席模式產生對應指令（帳號檔 = 「匯出 CSV」的 `sipress_accounts.csv`），契約 fixture `cli-agent.txt` 由 CLI e2e 驗證。

### [x] 6. 持續時間分兩端設定

持續時間從頂層 `duration` 移到 `caller.duration` / `agent.duration`（座席端預設 0 = 不限）；Sidebar 的欄位跟著模式分頁切換。舊版 localStorage 的共用值會同時套用到兩端。雙端啟動時座席端比民眾端早結束會在事件記錄警告。SIP server、Transport 仍為共用。

### [x] 7. 修好 Node 26 的前端測試環境

Node 26 內建的 `localStorage` 蓋掉 happy-dom 的，`freshPinia()` 呼叫 `localStorage.clear()` 失敗。已在 `vitest.config.ts` 用 `test.execArgv` 對測試 worker 加 `--no-experimental-webstorage`（Node 22 與 26 都認得；不認得的舊版不加），Node 22 / 26 下 `npm test` 皆通過。

### [x] 8. 決定 `cargo fmt` 的政策

採 rustfmt 預設設定：一次格式化整個 workspace（純格式化 commit 列在 `.git-blame-ignore-revs`），CI 的 Lint job 加上 `cargo fmt --all -- --check`。

### [x] 9. 座席「通話中忙線」模擬

`AgentBehavior.busy_in_call`（CLI `--busy-in-call`、GUI 座席區塊「通話中忙線」）：座席已有通話或有來電響鈴中時，新來電直接回 486 Busy Here（計為失敗通話），ACD 才會改派給其他座席。預設關閉，維持一個帳號可同時接多通的舊行為。測試在 `core/tests/agent_behavior.rs`（通話中、響鈴中、關閉時照常接聽）。

### [x] 10. 回歸測試發現並修正的問題

以 CLI 實際程序 + 模擬 SIPX（Digest 認證、B2BUA、ACD 改派）、Chromium 操作前端（invoke 轉接真引擎）、Xvfb 中實際操作 Tauri 程式做全功能回歸時發現：
- **雙端執行中無法切換民眾端 / 座席端分頁**（自加入雙端模式起）：分頁在設定鎖定的 fieldset 內，檢視中的一端一執行就被停用。分頁移出鎖定範圍，新增元件測試 `gui/src/__tests__/sidebar.spec.ts`。
- **兩端 SIP log 檔名相同**：民眾端也寫 `*_agent.sip.log`，雙端同一秒啟動會寫進同一個檔。民眾端改為 `*_caller.sip.log`。
- **事件記錄每秒重複 WARN**：只要失敗過一次，之後每秒都寫一次累計失敗。改為只在失敗數增加時記錄。
- 版面：座席行為的區間欄位在側欄寬度下數字被截斷；「測試執行中」橫幅半透明導致捲動時文字重疊；帳號清單「n / n 已註冊」被擠成直排。
- CLI 對照摘要在已使用 `--from-seq` 時仍建議加 `--from-seq`（改為與 HTML 相同，只在配對不確定時提示）。

---

以下依重要程度排序：先修會讓測試結果失真或無法使用的正確性問題，再補功能、指標與效能，最後是整理。

### [ ] 11. 座席端 codec 與 SDP 不一致（正確性）

座席回覆的 SDP 固定宣告 PCMA（`core/src/agent_engine.rs` 的 `build_response_with_sdp`），RTP 卻照 `cfg.codec` 送出（GUI 會把民眾端選的 codec 一起帶給座席端）。選 PCMU 時，座席宣告 PCMA、實際送 PCMU，對端可能無聲或 MOS 失真。兩端也都沒有依照對方的 SDP 協商：民眾端 offer `8 0 101`，但不論對方 answer 什麼都用 `cfg.codec` 送。
要做：座席依 INVITE 的 offer 選 codec 並在 answer 宣告同一個；民眾端依 200 OK 的 answer 決定送哪個 payload type。

### [ ] 12. 座席每通結束都重新註冊（正確性）

每通掛斷後立即送一次 REGISTER（`trigger_re_register`）。高話務時會對 SIPX 多送大量 REGISTER，壓測工具本身干擾了測試結果。
要做：只在接近 Expires 時刷新；若實際環境確實需要通話後重新註冊，改成選項（預設關閉）。

### [ ] 13. 民眾端 INVITE 不支援 401 / 407 認證（正確性，視環境）

REGISTER 有 Digest 認證，INVITE 沒有；SIPX 入口若要求撥號認證，所有通話都會被當成失敗。需先確認實際環境是否要求。
要做：收到 401 / 407 時帶 `Authorization` / `Proxy-Authorization` 重送 INVITE（CSeq 遞增、同一個 Call-ID），帳密由設定提供；GUI 與 CLI 加上對應欄位。

### [ ] 14. IPv6 位址解析錯誤（正確性）

有 4 處用 `split(':').next()` 取 IP，IPv6 位址會被截斷：`core/src/agent_engine.rs`（3 處）、`core/src/engine.rs`（1 處）。
要做：改用 `SocketAddr` / `IpAddr` 解析，補 IPv6 的單元測試。

### [ ] 15. 在真正的 SIPX 驗證端到端對照（驗證）

確認 sipress 民眾端直接撥入時，SIPX 入口是否產生 `P-CIN-CallId` 並回在 18x / 200（報告的「民眾端 CIN %」）。拿不到時改用 `--from-seq` 配對，並把結果補進第 3 項的紀錄。

### [ ] 16. TCP 傳輸（功能）

GUI 與 CLI 都能選 TCP，但一執行就報「尚未實作」。要嘛實作（連線管理、訊息分框、`Content-Length` 解析），要嘛在介面上先移除這個選項。TLS / SRTP 視需求另列。

### [ ] 17. CLI 補上 GUI 才有的設定（功能）

CPS 模式（漸增 / 階梯 / 尖峰）、DTMF、codec 目前只有 GUI 能設定，CLI 預覽會標示「不支援」（`testStore.ts` 的 `cliUnsupported`）。補齊後 GUI 的 CLI 預覽即可完整重現設定。

### [ ] 18. 即時指標的準確度（指標）

`METRICS.md` 第 11 節列的已知限制：
- 即時 CPS 由 GUI 輪詢差值計算，與引擎計時器不同步，會出現 0 與兩倍交替 → 改由引擎每秒統計並放進快照。
- 即時 ASR 的分母包含仍在響鈴的通話，數值偏低。
- PDD 測試中顯示平均、結束改為 P50，數字會跳動 → 統一口徑。

### [ ] 19. 長時間耐久測試的分時段統計（功能）

報告只有整體結果，看不出第幾小時開始變差。加上每 N 分鐘一段的 ASR、PDD、MOS、失敗碼統計，HTML 報告畫成時序圖，CLI 可輸出 CSV。

### [ ] 20. 大量並發的效能基準（效能）

座席端每個帳號各有一個 socket 與 task、民眾端每 10ms 掃描全部通話、每通 RTP 各自一個 20ms 計時器。上千並發時 CPU 負擔未知。
要做：先加 1,000 / 5,000 並發的基準測試（本機 mock），量出瓶頸再優化，避免盲目重寫。

### [ ] 21. 拆分 `agent_engine.rs`（維護）

約 1,500 行；`handle_request` 有 15 個參數，建構 REGISTER 的程式碼重複 4 處。整理成每個帳號一個狀態結構（socket、註冊狀態、dialog、響鈴中來電、追蹤紀錄），方法各自處理一種事件。

### [ ] 22. 清理專案（整理）

- 移除 `gui/src.zip`、根目錄只印 Hello world 的 `src/main.rs`。
- 更新或移除內容過時的 `gui/CHANGES.md`。
- v0.5.0 發佈說明的下載表漏寫 macOS 免安裝檔（`sipress-gui-macos-*-portable`），GitHub 上的 release 說明一併補上。
- CI 的 `actions/checkout@v4` 等 action 出現 Node 20 停用警告，升級到新版。

