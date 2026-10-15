# 效能基準

`core/examples/load_bench.rs`：本機 loopback 量測大量並發時引擎本身的 CPU、記憶體與排程延遲。mock SIPX（含 RTP sink）跑在**子行程**，量到的 CPU 只有引擎行程。

```bash
cargo build --release -p sipress-core --example load_bench
./target/release/examples/load_bench caller 5000 rtp   # 民眾端維持 5000 通並發、每通送 RTP
./target/release/examples/load_bench agent 1000        # 1000 個座席帳號各接一通、不送 RTP
./target/release/examples/load_bench raw 5000          # 對照組：只有 5000 個 socket 每 20 ms 送一個封包
```

流程：20 秒爬升到目標並發（民眾端 CPS = 並發 ÷ 20、每通 20 秒；座席端由 mock 以同樣速率打進來並一直保持），再穩定 20 秒，取穩定期中間 16 秒量測。

| 欄位 | 量法 |
|---|---|
| 引擎 CPU | `/proc/self/stat` 的 user + system 時間差 ÷ 經過時間；100% = 一個核心（僅 Linux） |
| RSS | `/proc/self/status` 的 `VmRSS` |
| 排程延遲 | 一個每 10 ms 醒來的 task 實際晚了多少；主迴圈與 RTP 計時器被拖慢時會變大 |
| RTP 實收 | mock 每秒實際收到的封包數；理論值 = 並發數 × 50 |

## 結果（2026-10，4 vCPU / 15 GB 的 Linux 容器，mock 與引擎共用 CPU）

| 情境 | 引擎 CPU | RSS | 排程延遲 p99 | RTP 實收 | 結果 |
|---|---|---|---|---|---|
| caller × 1000 | 6% | 7 MB | 2.1 ms | — | 全部接通 |
| caller × 5000 | 16% | 15 MB | 2.1 ms | — | 全部接通 |
| caller × 1000 + RTP | 71–73% | 23 MB | 5.9 ms | 96.7% | 全部接通 |
| caller × 5000 + RTP | 280% | 89 MB | 30 ms | 86.4% | 10000 通中 1 通逾時（CPU 已滿） |
| agent × 1000 | 1% | — | 2.1 ms | — | 全部接通 |
| agent × 5000 | 1% | 42 MB | 1.7 ms | — | 全部接通 |
| agent × 1000 + RTP | 68% | — | 4.6 ms | 96.8% | 全部接通 |
| agent × 5000 + RTP | 284% | 110 MB | 30 ms | 86.7% | 全部接通 |
| raw × 1000（對照組） | 55% | 6 MB | — | 97.1% | — |
| raw × 5000（對照組） | 247% | 12 MB | — | 93.9% | — |

## 結論

- **SIP 信令不是瓶頸**：沒有 RTP 時 5000 通並發只用 16%（民眾端）/ 1%（座席端）的單核 CPU。民眾端每 10 ms 掃描全部通話、座席端每帳號一個 socket 與 task 都不是問題。
- **RTP 是主要成本，且接近每路一個 socket 的下限**：與只做「計時器 + `send_to`」的對照組相比，引擎多出約 13–25%（統計、音檔、鎖），其餘是每個封包一次系統呼叫（loopback 時還包含接收端的核心處理）。5000 路 RTP 約需 3 個核心；CPU 飽和時 RTP 計時器延遲上升、mock 漏收。要再大幅降低只能改架構（單一計時器驅動所有串流 + `sendmmsg` 批次送出），目前不值得。
- **建議規模**：每個核心約 1500–2000 路 RTP；超過時把民眾端與座席端分在不同機器，或拆成多個 sipress 行程。

## 這次量測找到並修正的問題

| 問題 | 修正前 | 修正後 |
|---|---|---|
| 座席端每個帳號的 runner 各複製一份完整設定（含全部帳號清單），記憶體與啟動時間隨帳號數平方成長 | agent × 5000：RSS 4950 MB、啟動期間 CPU 40% | 42 MB、1% |
| 每個 UDP socket 在等待時都握著 64 KB 接收緩衝區 | agent × 5000：RSS 405 MB | 42 MB（改為有資料才用每執行緒共用的緩衝區） |
| 座席 REGISTER 遺失（UDP）時不重傳：大量帳號同時註冊時伺服器漏收，該帳號要等約 2 分鐘的刷新才上線 | agent × 5000：只有 4157 個帳號接到電話 | 依 RFC 3261 Timer E 重傳（0.5 s 起加倍、上限 4 s、32 s 放棄），5000 個全部上線 |
