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

### [x] 11. 座席端 codec 與 SDP 不一致，以及音檔相關問題（正確性）

原本座席回覆的 SDP 固定宣告 PCMA，RTP 卻照 `cfg.codec` 送；兩端也沒有依對方 SDP 協商。另外：選 PCMU 時音檔內容仍是 A-law、只把 PT 標成 0（對方聽到失真）；GUI 選檔視窗列出不支援的 `.mp3` / `.pcm`、卻沒列 `.ul` / `.al` 等；音檔載入失敗時默默改送靜音。
已修正：
- 新增 `core/src/sip/sdp.rs` 處理協商。民眾端 offer 把偏好的 codec 放第一個，依 200 OK 的 answer 決定送出的 PT；座席端依 offer 選 codec，answer 只宣告選中的那一個（offer 有 101 時一併接受），沒有 G.711 回 488；RE-INVITE 依是否帶 SDP 回 answer 或 offer。
- 音檔快取維持 A-law，協商出 PCMU 時送出前逐 frame 轉成 μ-law（`alaw_to_ulaw` 查表）。
- GUI 選檔視窗改為實際支援的副檔名；兩端引擎與 GUI 的 `start_test` 在開始前先載入音檔，失敗就回報原因。
- 測試：`core/tests/codec_negotiation.rs`（PCMU offer / answer、保留偏好、488、音檔不支援），`sip::sdp` 與 `alaw_to_ulaw` 的單元測試。

### [x] 12. 座席每通結束都重新註冊（正確性）

原本每通掛斷後立即送一次 REGISTER（`trigger_re_register`），原 commit 沒有說明原因；高話務時會對 SIPX 多送大量 REGISTER，干擾測試結果。
已修正：預設只在 Expires 到期前刷新；保留原行為為選項 `Config.agent_reregister_after_call`（CLI `--reregister-after-call`、GUI 座席區塊「通話後重新註冊」，預設關閉），供交換機確實要求通話後重新註冊的環境使用。測試：`core/tests/agent_behavior.rs` 的 `call_end_does_not_reregister_by_default` / `reregister_after_call_is_opt_in`（對方 BYE 與座席主動掛斷兩條路徑）。

### [x] 13. 民眾端 INVITE 不支援 401 / 407 認證（正確性，視環境）

原本 REGISTER 有 Digest 認證、INVITE 沒有，SIPX 入口要求撥號認證時所有通話都會失敗。
已修正：設定撥號認證密碼後（`Config.caller_auth_username` / `caller_auth_password`；CLI `--auth-user` / `--auth-pass`；GUI 民眾端「撥號認證帳號 / 密碼」），INVITE 收到 401 / 407 會以同一個 Call-ID、CSeq + 1、新 branch 帶 `Authorization` / `Proxy-Authorization` 重送一次；帶認證仍被拒才算失敗。帳號未填時用該通的主叫號碼。舊 transaction 重傳的 401 以 CSeq 判斷並忽略。GUI 的 CLI 預覽以 `"<密碼>"` 代替真實密碼。
同時修正：開啟 `--from-seq` 時 ACK / CANCEL / BYE / RE-INVITE 回應的 From 仍用基本號碼，與該通 INVITE 不一致（主叫號碼改存在 dialog）。
測試：`core/tests/invite_auth.rs`（401、407、未設密碼、密碼錯誤只重試一次，mock 會驗證 Digest 雜湊並重傳 401）。

### [x] 14. IPv6 位址解析錯誤（正確性）

原本多處用 `split(':')` 取 IP，IPv6 位址會被截斷；另外 Request-URI、SDP、綁定位址也都假設 IPv4。
已修正：新增 `core/src/net.rs`（`host` / `uri_host` / `join` / `is_ipv6` / `unspecified_for`），兩端引擎、SDP、SIP 訊息、RTP、註冊伺服器與 GUI 改用它：
- SIP URI 中的 IPv6 加中括號（`sip:2001@[2001:db8::1]:5060`）；`ip:port` 一律由 `join` 組成。
- SDP 依位址類型寫 `IN IP6` / `IN IP4`；解析對方 SDP 的 IPv6 `c=` 也正確。
- 連往 IPv6 伺服器時以 `[::]:0` 綁定；`--local ::1` 這類只給 IPv6 IP 的寫法可用。
測試：`net`、`sdp`、`message`、`transport` 的單元測試；`core/tests/ipv6.rs`（民眾端撥 IPv6 伺服器含 RTP、座席向 IPv6 伺服器註冊）。開發容器沒有 IPv6，此檔在無法綁定 `[::1]` 時會略過並印出原因，實際在 CI 執行。

### [ ] 15. 在真正的 SIPX 驗證端到端對照（驗證）

確認 sipress 民眾端直接撥入時，SIPX 入口是否產生 `P-CIN-CallId` 並回在 18x / 200（報告的「民眾端 CIN %」）。拿不到時改用 `--from-seq` 配對，並把結果補進第 3 項的紀錄。

### [x] 16. TCP 傳輸（功能）

原本 GUI 與 CLI 都能選 TCP，但一執行就報「尚未實作」。
已實作：`core/src/sip/transport.rs` 的 `SipSocket` 把 UDP / TCP 包成同一個介面，民眾端、座席端與 GUI 的「測試註冊」都改用它：
- TCP 時民眾端整個測試、每個座席帳號各用一條連線；依 `Content-Length`（含 compact `l:`）分框，略過 keep-alive CRLF，單則訊息上限 64 KB。
- Via 為 `SIP/2.0/TCP`、Contact 帶 `;transport=tcp`（REGISTER 也帶，否則伺服器會用 UDP 找座席）；座席的回應與主動 BYE 依來電 Via 的協定。
- TCP 不做 SIP 層重傳（INVITE / BYE），由傳輸層保證送達。
- 連線失敗或伺服器斷線會直接回報錯誤。CLI `--transport` 只接受 udp / tcp（原本 `tls` 會被默默當成 UDP）。
- 限制：伺服器送給座席的請求必須走座席註冊的那條連線（SIPX 等 proxy 的一般做法）；sipress 不另外監聽 TCP port。RTP 仍為 UDP。TLS / SRTP 視需求另列。
測試：`take_frame` 分框與 `via_transport` 單元測試、`SipSocket` TCP 收送；`core/tests/tcp_transport.rs`（民眾端單一連線完成兩通、回應延遲超過 T1 也不重傳、兩則回應黏在同一次寫入；座席端 TCP 註冊與接聽；連線被拒立即失敗）。

### [x] 17. CLI 補上 GUI 才有的設定（功能）

原本 CPS 模式（漸增 / 階梯 / 尖峰）、DTMF、codec 只有 GUI 能設定，CLI 預覽標示「不支援」。
已完成：CLI 新增 `--cps-ramp START-END`、`--cps-steps CPS:秒,...`、`--cps-spike 基準,尖峰,開始%,秒數`（三者擇一）、`--codec pcma|pcmu`、`--dtmf 序列`（需 `--rtp`）。GUI 的 CLI 預覽改為輸出這些參數，移除「CLI 不支援」提示。
- core 的 `Config::validate` 新增 CPS 模式與 DTMF 格式檢查（CLI、GUI 共用）；GUI 啟動前也檢查 DTMF 格式。原本 `123#` 這種不加逗號的寫法只會送出第一個按鍵，現在會直接提示格式錯誤。
- 測試：CLI 參數解析與錯誤值、`CpsPattern::validate`、DTMF 格式；`gui_cli_preview_reproduces_gui_config` 把 `gui/contract/cli-*.txt` 交給 CLI 解析後，逐欄比對 GUI 實際送出的 `config-*.json`（帳號檔、密碼佔位字、未使用的欄位除外）。以 mock SIPX 實跑：階梯 2:3,8:3 → 每秒 INVITE `2,2,2,8,8,8`；漸增 1-9 → `1,3,4,6,7,8`；尖峰 2,10,50,2 → `2,2,2,10,10,2`；`--codec pcmu` offer `0 8 101`；`--dtmf` 送出 PT 101。

### [x] 18. 即時指標的準確度（指標）

原本 `METRICS.md` 第 11 節列的已知限制，已修正：
- 即時 CPS：改由引擎的進度 task 每秒計算（`RateMeter`：計數增加量 ÷ 實際經過秒數）放進快照的 `cps`，GUI 直接使用，不再用輪詢差值（不會出現 0 與兩倍交替）。
- 即時 ASR / ERR%：分母改為「已有結果」的通話（接通 + 失敗 + 逾時），不含還在響鈴的通話；最終報告仍以全部發起為分母（民眾端兩者一致）。
- PDD：快照新增 `pdd_p50_ms`，GUI 測試中與結束後都顯示 p50。
- 同時修正：時間到後引擎收尾（CANCEL / BYE、座席解除註冊）期間，進度回報原本就停了，GUI 停在最後一筆；現在持續回報到引擎結束（保留寬限時間避免引擎異常時 task 不停）。
測試：`stats` 單元測試（即時 / 最終 ASR、`RateMeter`、PDD p50 與報告一致）、`core/tests/live_metrics.rs`（20 CPS 實跑，每秒 CPS 15–25、響鈴中的通話不拉低 ASR、最後一筆快照為已結束）、GUI vitest 改為採用引擎的 `cps` 與 `pdd_p50_ms`。

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

