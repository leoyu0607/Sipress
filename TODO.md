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
| 發佈 | 改 7 個檔案的版本號（`Cargo.toml` ×3、`package.json`、`package-lock.json`、`tauri.conf.json`、`Cargo.lock`），commit `Release vX.Y.Z`，推 tag `vX.Y.Z` |

---

## 清單

### [x] 1. GUI「本機 Port」設定沒有作用 — PR #8

`SharedUdpSocket::new` 只取 IP、port 一律綁 0。已修正：`IP:port` 會綁定指定 port，被佔用時報清楚的錯誤；GUI 本地埠只套用在民眾端，0 = 自動。

### [x] 2. 座席註冊狀態回報到即時快照 — PR #9

快照新增 `agents_registered` / `agents_total`（每帳號由 `RegisteredGauge` 追蹤，drop 時自動取消）。座席端檢視的指標列以 REG n/總數 取代 PDD；雙端啟動改為等座席全部註冊才撥號（最多 10 秒，部分成功繼續並警告，全部失敗不撥號）。
「超過 Expires 未刷新 → 取消」只有單元測試（刷新間隔最少 60 秒）。

### [ ] 3. 端到端對照報告（前置的第 2 項已完成）

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

### [x] 4. 座席行為模擬

`Config.agent_behavior`（`AgentBehavior`）：響鈴延遲區間、拒接 %（486）、不接 %（響到 CANCEL → 487，120 秒上限回 480）、接聽後主動掛斷區間、wrap-up 休息秒數（期間來電 486）。座席 runner 新增「響鈴中」狀態，響鈴中的 CANCEL 回 487。CLI 參數 `--ring` / `--talk`（`N` 或 `MIN-MAX`）、`--reject-pct`、`--no-answer-pct`、`--wrap-up`；GUI 座席區塊有對應欄位與驗證，CLI 預覽只帶非預設值。流程測試在 `core/tests/agent_behavior.rs`。
目前座席同時可接多通（通話中來電仍會接聽），沒有模擬「通話中忙線」。

### [x] 5. CLI 支援座席模式

新增 `--mode agent`（不再隱藏）、`--accounts <檔案>`、`--account-domain`、`--ack-timeout`、`--bye-timeout`。帳號檔格式與 GUI 匯入相同（解析在 core 的 `AgentAccount::parse_list`，格式錯誤 / 分機重複會指出行號）。CLI 加上 Ctrl-C 與 TUI `q` 的 graceful stop（座席端會先解除註冊）。GUI 的 CLI 預覽在座席模式產生對應指令（帳號檔 = 「匯出 CSV」的 `sipress_accounts.csv`），契約 fixture `cli-agent.txt` 由 CLI e2e 驗證。

### [x] 6. 持續時間分兩端設定

持續時間從頂層 `duration` 移到 `caller.duration` / `agent.duration`（座席端預設 0 = 不限）；Sidebar 的欄位跟著模式分頁切換。舊版 localStorage 的共用值會同時套用到兩端。雙端啟動時座席端比民眾端早結束會在事件記錄警告。SIP server、Transport 仍為共用。

### [x] 7. 修好 Node 26 的前端測試環境

Node 26 內建的 `localStorage` 蓋掉 happy-dom 的，`freshPinia()` 呼叫 `localStorage.clear()` 失敗。已在 `vitest.config.ts` 用 `test.execArgv` 對測試 worker 加 `--no-experimental-webstorage`（Node 22 與 26 都認得；不認得的舊版不加），Node 22 / 26 下 `npm test` 皆通過。

### [x] 8. 決定 `cargo fmt` 的政策

採 rustfmt 預設設定：一次格式化整個 workspace（純格式化 commit 列在 `.git-blame-ignore-revs`），CI 的 Lint job 加上 `cargo fmt --all -- --check`。

### [ ] 9. 座席「通話中忙線」模擬

第 4 項的行為模擬只在 wrap-up 期間拒接；座席通話中收到新來電仍會接聽（一個帳號可同時多通）。真實話機通常一次一通，可加一個選項讓通話中的來電回 486，ACD 才會改派給其他座席。
