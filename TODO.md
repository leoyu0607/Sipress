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
| 前端測試 | 本機若是 Node 26，需 `NODE_OPTIONS=--no-webstorage npm test`（見第 7 項）；CI 用 Node 22 |
| Rust 測試 | 本機沒有 `cargo nextest` 時改用 `cargo test -p sipress-core -p sipress`；GUI 後端在 `gui/src-tauri` 下 `cargo test` |
| Lint | `cargo clippy … -- -D warnings`（core/cli 與 gui 各跑一次），警告即失敗 |
| `cargo fmt` | 目前沒有強制，既有檔案本來就不符合（見第 8 項），不要整檔重排 |
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

### [ ] 4. 座席行為模擬

目前座席收到來電一律立即接聽，測不到 ACD 的重新分配。新增參數：
- 響鈴幾秒後才接
- 一定比例拒接（486 Busy）或不接（讓 SIPX 逾時重派）
- 通話中由座席主動掛斷
- 兩通之間的休息時間（wrap-up time）

### [ ] 5. CLI 支援座席模式

`AgentEngine` 已在 core，CLI 需補參數（例如帳號檔路徑）。目前 `cli/src/args.rs` 的 `--mode` 是隱藏參數且只支援 caller；GUI 的 CLI 預覽在座席模式只顯示註解（`testStore.ts` 的 `cliCommand`）。

### [ ] 6. 持續時間分兩端設定

目前 SIP server、Transport、持續時間為兩端共用。常見需求是座席端先開、比民眾端晚關，持續時間應可分開設定。

### [ ] 7. 修好 Node 26 的前端測試環境

Node 26 內建的 `localStorage` 蓋掉 happy-dom 的，`freshPinia()` 呼叫 `localStorage.clear()` 失敗。在 vitest 設定處理，讓 `npm test` 不需要 `NODE_OPTIONS`。

### [ ] 8. 決定 `cargo fmt` 的政策

既有程式碼採對齊排版，不符合 rustfmt 預設。二選一：一次格式化全部並加進 CI；或加 `rustfmt.toml` / 文件說明不使用 rustfmt。
