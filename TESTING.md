# sipress — 測試與 CI

## 測試分層

| 層級 | 位置 | 內容 |
|---|---|---|
| Rust 單元測試 | `core/src/**`（`#[cfg(test)]`） | SIP 解析 / 組裝、Dialog 狀態機、RTP 統計（掉包、Jitter、MOS、聚合）、CPS 模式、設定驗證、指標計算 |
| 引擎流程測試 | `core/tests/*.rs` | loopback mock SIP server：INVITE / 重傳 / CANCEL / BYE / 認證 / 座席註冊與接聽、RTP latching 與 port 釋放 |
| CLI 端到端測試 | `cli/tests/cli_e2e.rs` | 執行編譯好的 `sipress` 執行檔對 mock UAS 壓測，驗證參數解析 → 引擎 → JSON / CSV / HTML 報告 |
| 前端單元測試 | `gui/src/__tests__/testStore.spec.ts` | store 的驗證、輪詢 snapshot → 指標 / 圖表、最終報告、停止流程、帳號匯入 / 生成、設定保存、匯出 |
| 前端 ↔ Rust 契約 | `gui/contract/` | 前端與後端交換的 JSON 格式，雙向驗證（見下節） |
| GUI 後端測試 | `gui/src-tauri/src/commands.rs` | 執行狀態管理、契約 fixtures 驗證 |

## 前端 ↔ Rust 契約（`gui/contract/`）

GUI 與 Rust 之間只靠 JSON 溝通，任一邊改了欄位名稱都不會有編譯錯誤，所以用 fixtures 雙向檢查：

| 檔案 | 產生者 | 驗證者 | 檢查內容 |
|---|---|---|---|
| `config-*.json` | vitest（`contract.spec.ts`） | `cargo test -p sipress-gui` | 前端送給 `start_test` 的設定能被 `Config` 解析、通過 `validate()`，且**沒有 Rust 不認得的欄位**（serde 預設會默默忽略拼錯的欄位） |
| `cli-*.txt` | vitest | `cargo test -p sipress --test cli_e2e` | Sidebar 的 CLI 預覽指令，每個參數都能被 CLI 解析 |
| `snapshot.json`、`report.json` | `cargo test -p sipress-gui` | vue-tsc + vitest | Rust 輸出的欄位與前端型別一致，store 讀到正確的值 |

**修改了欄位之後**，在本機重新產生 fixtures 並一起 commit：

```bash
# 前端：buildRustConfig() 或 cliCommand 有變更
cd gui && npm test -- -u

# Rust：StatsSnapshot 或 FinalReport 有變更（之後再跑一次 npm test 確認前端）
UPDATE_CONTRACT=1 cargo test -p sipress-gui
```

CI 不會改寫 fixtures，內容與程式不一致時直接失敗。

## 本機執行

```bash
# Rust：core + CLI（含流程與端到端測試）
cargo test -p sipress-core -p sipress

# 與 CI 相同的 lint（警告即失敗）
cargo clippy --workspace --all-targets --locked -- -D warnings

# 前端
cd gui
npm run typecheck
npm test            # 或 npm run test:watch

# GUI 後端（需要先 npm run build 產生 gui/dist）
cargo test -p sipress-gui
```

CI 使用 [cargo-nextest](https://nexte.st/) 執行 Rust 測試（設定在 `.config/nextest.toml`），本機也可以用 `cargo nextest run -p sipress-core -p sipress`。

## CI（`.github/workflows/ci.yml`）

在 push 到 main、PR、手動觸發，以及每週一排程時執行。

| Job | 必要 | 內容 |
|---|---|---|
| Lint | ✓ | clippy `-D warnings`（版本固定在 `LINT_TOOLCHAIN`）、actionlint 檢查 workflow 語法 |
| Test | ✓ | Linux / macOS / Windows 跑 core + CLI 全部測試、doctest、範例編譯；上傳 JUnit 報告 |
| Frontend | ✓ | vue-tsc 型別檢查、vitest（含契約）、vite 建置 |
| GUI backend | ✓ | Tauri 後端 clippy + 測試（含契約驗證） |
| Coverage | | 覆蓋率摘要（顯示在 run 的 Summary 頁）與 lcov 檔 |
| Dependency audit | | `cargo audit`、`npm audit --audit-level=high` |
| CI passed | | 彙整上面 4 個必要 job；設定 branch protection 時只需把它設為 required check |

穩定性措施：

- Rust 測試失敗會自動重試 2 次，重試後才通過的測試在報告中標示為 FLAKY。
- 單一測試超過 120 秒會被強制終止。
- 每個 job 都設有逾時上限。
- 所有 cargo 指令都加 `--locked`，`Cargo.lock` 與 `Cargo.toml` 不一致時直接失敗。

升級 Rust 版本時，把 `LINT_TOOLCHAIN` 改成新版本，並修正新版 clippy 提出的警告。
