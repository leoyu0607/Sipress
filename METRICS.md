# sipress — 指標計算邏輯

本文件說明 sipress 每個數據「怎麼算出來的」：資料從哪個 SIP / RTP 事件取得、公式、單位，以及即時值與最終報告的差異。
所有描述都對應目前的程式碼，括號內標註來源位置，修改計算方式時請一併更新本文件。

## 目錄

1. [資料流概觀](#1-資料流概觀)
2. [通話計數器](#2-通話計數器)
3. [比率指標：ASR / CCR / ERR%](#3-比率指標asr--ccr--err)
4. [CPS 與並發](#4-cps-與並發)
5. [時間指標：PDD / Setup Time / ACD](#5-時間指標pdd--setup-time--acd)
6. [RTP 聲音品質：掉包率 / Jitter / MOS](#6-rtp-聲音品質掉包率--jitter--mos)
7. [多通電話的 RTP 聚合](#7-多通電話的-rtp-聚合)
8. [回應碼統計](#8-回應碼統計)
9. [座席模式（Agent）的差異](#9-座席模式agent的差異)
10. [各介面的顯示與分級門檻](#10-各介面的顯示與分級門檻)
11. [已知限制與解讀注意事項](#11-已知限制與解讀注意事項)
12. [端到端對照](#12-端到端對照)
13. [分時段統計](#13-分時段統計)

---

## 1. 資料流概觀

```
SIP / RTP 事件
   │
   ├─▶ LiveStats（原子計數器，無鎖）          core/src/stats.rs
   │      calls_initiated / answered / completed / failed / timeout / active
   │
   ├─▶ DetailedStats（HDR Histogram，µs）     core/src/stats.rs
   │      PDD、Setup Time、通話時長、失敗回應碼
   │
   └─▶ RtpStats（每通電話一份）               core/src/rtp/stats.rs
          收發封包、序號、Jitter → 掉包率、MOS

每 1 秒：LiveStats.snapshot() + PDD 平均 + 進行中通話的 RTP 聚合 → StatsSnapshot（即時值）
結束時：FinalReport::build(snapshot, histograms, 全部通話 RTP 聚合)   → FinalReport（最終報告）
```

- **即時值（StatsSnapshot）**：引擎的進度 task 每秒產生一次；GUI 每 1 秒呼叫 `get_snapshot` 取最新一份，TUI 也用它。
- **最終報告（FinalReport）**：測試結束後產生，CLI 的 Table / JSON / CSV、HTML 報告與 GUI 結束後的數字都來自它。
- 時間直方圖使用 HDR Histogram，3 位有效數字，百分位數的相對誤差約 0.1%。
  - PDD / Setup Time 的範圍是 1 µs ~ 60 s。這兩個值受 INVITE 逾時限制，不會超過 60 秒。
  - 通話時長的範圍是 1 µs ~ 24 小時。
  - 超出範圍的樣本會被丟棄。

---

## 2. 通話計數器

`LiveStats`（`core/src/stats.rs`）只在以下時機遞增，每通電話**只會落入一個結束狀態**：

| 計數器 | 何時 +1 | 民眾端（Caller）觸發點 |
|---|---|---|
| `calls_initiated` | 發起通話 | 送出 INVITE（`engine.rs` 發話迴圈） |
| `calls_answered` | 接通 | 第一次收到 INVITE 的 200 OK（200 OK 重傳不重複計數） |
| `calls_completed` | 正常結束 | 收到 BYE 的任何最終回應（200–699），或收到伺服器的 BYE，或送出 BYE 後超過 `invite_timeout` 仍無回應（強制結束） |
| `calls_failed` | 被拒 | 在 Calling / Trying / Ringing 狀態收到 INVITE 的 3xx–6xx |
| `calls_timeout` | 逾時 | INVITE 送出後超過 `invite_timeout` 仍未收到最終回應（送 CANCEL），**或測試結束 / 手動停止時仍未接通的通話**（送 CANCEL） |
| `calls_active` | 進行中 | INVITE 時 +1；completed / failed / timeout 時 −1 |

關係式（通話都結束後成立）：

```
calls_initiated = calls_completed + calls_failed + calls_timeout + calls_active
calls_answered  ≈ calls_completed + （已接通、尚未結束的通話）
```

> `invite_timeout`：CLI 由 `--invite-timeout` 設定；GUI 固定為 8 秒。

---

## 3. 比率指標：ASR / CCR / ERR%

分母為 0 時回報 0。接通、失敗、逾時三者互斥，`已有結果 = calls_answered + calls_failed + calls_timeout`。

| 指標 | 即時值（`LiveStats::snapshot()`） | 最終報告（`FinalReport::build()`） |
|---|---|---|
| **ASR**（Answer Seizure Ratio，接通率） | `calls_answered / 已有結果 × 100` | `calls_answered / calls_initiated × 100` |
| **ERR%**（Error Rate） | `(calls_failed + calls_timeout) / 已有結果 × 100`（`error_rate`） | — |
| **CCR**（Call Completion Rate，完成率） | `calls_completed / calls_initiated × 100`（GUI 在前端計算） | 同左 |

注意：

- 即時 ASR / ERR% 不把還在等回應或響鈴的通話算進分母，兩者合計 100%。民眾端結束時未接通的通話都以 CANCEL 計入逾時，所以全部通話都有結果，即時值與最終報告相同；座席端結束時若仍有響鈴中的來電，最終 ASR 會略低於最後的即時值。
- CCR 只在通話「結束」時才計入，通話時長（`--call-duration`）越長，即時 CCR 落後 ASR 越多。

---

## 4. CPS 與並發

### 目標 CPS（發話排程）

發話時間採**累積排程**：每發出一通，下一通的預定時間 `next_call += 1 / current_cps`。主迴圈落後時會在同一輪補發所有到期的通話（每輪最多 200 通，`MAX_CALLS_PER_TICK`），所以實際速率不會被迴圈頻率限制。

`current_cps` 由 CPS 模式決定（`CpsPattern::current_cps`，`core/src/config.rs`），`t` = 已執行時間、`T` = 測試總時長：

| 模式 | 公式 |
|---|---|
| Flat | `cps`（固定值） |
| Ramp | `start + (end − start) × clamp(t / T, 0, 1)`，線性內插；`T = 0` 時直接用 `end` |
| Step | 依序累加各階層的 `duration_secs`，`t` 落在哪個區間就用該階層的 `cps`；超過所有階層後維持最後一階 |
| Spike | 尖峰開始 `= T × spike_at_pct / 100`，持續 `spike_duration_secs`；區間內用 `peak`，其他時間用 `base` |

計算出的 CPS 下限為 0.1（避免除以 0）。

### 並發上限

發話前統計 Calling / Trying / Ringing / Connected 狀態的通話數，達到 `max_concurrent_calls` 時暫停發話。**受上限限制期間不累積欠額**（`next_call = now`），名額空出後依原速率繼續，不會一次補發。因此並發上限太低時，實際 CPS 會低於目標 CPS。

### 顯示的 CPS 與並發

| 指標 | 公式 | 出處 |
|---|---|---|
| **即時 CPS**（GUI 指標列 / 圖表） | 引擎每秒快照時計算：`calls_initiated` 增加量 ÷ 距上次快照的實際秒數（座席端為收到的 INVITE） | 快照的 `cps`（`RateMeter`，`core/src/stats.rs`） |
| **實際 CPS**（最終報告） | `calls_initiated / 測試總時長（秒）` | `FinalReport::build()` |
| **CONCUR / 並發** | `calls_active`（負值時以 0 顯示） | `LiveStats::snapshot()` |

> 測試總時長包含結束後等待 BYE / CANCEL 回應的收尾時間（最多 `invite_timeout` 秒），所以最終報告的實際 CPS 會略低於目標值。

---

## 5. 時間指標：PDD / Setup Time / ACD

時間點由 `Dialog` 記錄（`core/src/sip/dialog.rs`），使用單調時鐘 `Instant`：

```
invite_sent_at ──▶ ringing_at ──▶ answered_at ─────────▶ ended_at
   (送 INVITE)     (180/183)       (200 OK)              (結束)
   │◀──── PDD ───▶│
   │◀────── Setup Time ─────────▶│
                                  │◀──── 通話時長 ──────▶│
```

| 指標 | 公式 | 記錄時機 |
|---|---|---|
| **PDD**（Post Dial Delay） | `ringing_at − invite_sent_at`（ms） | 第一次收到 180 Ringing **或 183 Session Progress** |
| **Setup Time** | `answered_at − invite_sent_at`（ms） | 第一次收到 INVITE 的 200 OK |
| **通話時長** | `結束時間 − answered_at`（秒） | 收到 BYE 回應、收到伺服器 BYE，或 BYE 逾時強制結束時 |

說明：

- `invite_sent_at` 是**第一次**送出 INVITE 的時間，INVITE 重傳不會重設，所以重傳造成的延遲會反映在 PDD / Setup Time 上。
- 沒有回 180/183、直接 200 OK 的通話**不會產生 PDD 樣本**，但會產生 Setup Time 樣本。
- BYE 逾時強制結束時，通話時長會包含等待 BYE 回應的時間（最多 `invite_timeout` 秒）。

### 統計方式

| 輸出 | 計算 |
|---|---|
| PDD 即時中位數（`pdd_p50_ms`） | PDD 直方圖目前所有樣本的 p50；尚無樣本時為 `None`（GUI 顯示「—」）。GUI 測試中與結束後都顯示 p50 |
| PDD 即時平均（`pdd_avg_ms`） | 目前所有樣本的平均值（快照仍提供，GUI 不使用） |
| PDD / Setup p50、p95、p99 | 直方圖 `value_at_quantile(0.50 / 0.95 / 0.99)` |
| PDD / Setup max | 直方圖最大值 |
| **ACD**（Average Call Duration） | 通話時長直方圖的平均值；`calls_completed = 0` 時為 0 |

> ACD 只有最終報告才有；測試進行中 GUI 沒有 ACD 即時值。

---

## 6. RTP 聲音品質：掉包率 / Jitter / MOS

每通接通的電話各有一份 `RtpStats`（`core/src/rtp/stats.rs`）。這些指標只在**有啟用 RTP** 時才有值：

- CLI 需要加 `--enable-rtp`。
- GUI 需要勾選播放音檔並選擇檔案。

### 6.1 封包計數

| 欄位 | 何時 +1 |
|---|---|
| `sent_packets` | `socket.send()` **成功之後**（送失敗不算） |
| `recv_packets` | 每收到一個 RTP 封包（包含重複、亂序封包） |
| `duplicates` | 序號與上一個封包相同 |
| `out_of_order` | 序號比上一個封包舊（`seq − last_seq` 以 u16 計算 > 0x7FFF） |

### 6.2 掉包率（RFC 3550 §A.3）

以**接收端序號空間**計算應收封包數：

```
expected = seq_cycles × 65536 + (max_seq − first_seq) + 1
lost     = expected − received          （小於 0 時視為 0）
loss     = lost / expected
```

- `first_seq`：收到的第一個序號；`max_seq`：收到的最新（最大）序號。
- `seq_cycles`：序號從 65535 回繞到 0 的次數，長時間通話也能正確計算。
- **SSRC 切換**（例如 RE-INVITE 後對端換了媒體來源）：舊 SSRC 的 `expected` / `received` 先累加到 `prior_expected` / `prior_received`，新 SSRC 重新開始追蹤序號，最後合併計算：

  ```
  loss = (prior_expected + cur_expected − prior_received − cur_received)
         / (prior_expected + cur_expected)
  ```

  這樣不會因為兩段序號差距很大而算出假的掉包。
- **有送無收的保護**：完全沒收到對端 RTP（`expected = 0`）但本端有送出封包時，掉包率記為 **100%**，`lost = sent_packets`。避免單向無聲的通話被誤判為「0% 掉包、MOS 優良」。
- 沒送也沒收時為 0%。

### 6.3 Jitter（RFC 3550 §A.8）

每收到一個**比上一個封包新**的封包（重複、亂序封包不計入）：

```
D = (到達時間差 × 8000) − (RTP timestamp 差)      ← 皆以 8 kHz 時鐘單位
J = J + (|D| − J) / 16                           ← 指數加權移動平均
jitter_ms = J / 8                                 ← 8 kHz 單位 → ms
```

- 實作上以 `J × 16` 的整數形式保存：`J16 = J16 + |D| − J16 / 16`，`jitter_ms = J16 / 16 / 8`。
- 時鐘固定以 8 kHz 計算，與 G.711（PCMA / PCMU）的取樣率一致。
- SSRC 切換時 Jitter 歸零重新累計。

### 6.4 MOS（ITU-T G.107 E-Model 簡化版）

由掉包率與 Jitter 估算（`estimate_mos()`）：

```
Ppl    = 掉包率（%，上限 100）
Ie_eff = Ie + (95 − Ie) × Ppl / (Ppl + Bpl)      Ie = 0、Bpl = 4.3（G.711）
Id     = 0                         （jitter < 150 ms）
       = (jitter_ms − 150) × 0.1   （jitter ≥ 150 ms）
R      = clamp(93.2 − Ie_eff − Id, 0, 100)
MOS    = 1 + 0.035R + R(R − 60)(100 − R) × 7×10⁻⁶     （G.107 §B.4）
         R ≤ 0 → 1.0；R ≥ 100 → 4.5；結果限制在 1.0 ~ 5.0
```

對照表（Jitter < 150 ms 時）：

| 掉包率 | R 值 | MOS |
|---|---|---|
| 0% | 93.2 | ≈ 4.41（本公式的最高值） |
| 1% | 75.3 | ≈ 3.83 |
| 3% | 54.2 | ≈ 2.79 |
| 5% | 42.1 | ≈ 2.17 |
| 100% | 2.6 | ≈ 1.0 |

注意：

- 這是**估算值**，沒有分析實際音訊內容，也沒有量測單向延遲。
- Jitter 在公式中被當作延遲的近似，**150 ms 以下對 MOS 完全沒有影響**，所以 MOS 實際上幾乎只反映掉包率。
- `Bpl = 4.3` 是 G.711 沒有封包遺失補償（PLC）時的值，1% 的掉包就會讓 MOS 掉到 4 以下。

---

## 7. 多通電話的 RTP 聚合

畫面與報告上的 RTP 數字，都是由 `aggregate_snapshots()` 把多通電話合併而成：

| 指標 | 聚合方式 | 原因 |
|---|---|---|
| 掉包率 | `Σ lost / Σ (received + lost) × 100`（依封包數加權） | 與加總的掉包數一致，短通話不會被放大 |
| Jitter | `Σ (jitter × received) / Σ received`（依收到的封包數加權） | 沒收到封包的通話沒有 Jitter 樣本，不應拉低平均 |
| MOS | 每通電話 MOS 的**算術平均** | 每通電話的體驗各占一票 |
| 送出、收到、亂序、重複封包數 | 直接加總 | — |

聚合的範圍：

| 時機 | 民眾端 | 座席端 |
|---|---|---|
| 即時（每秒） | 只含**進行中**的通話 | 只含**進行中**的通話 |
| 最終報告 | 已結束通話 + 結束時仍在進行的通話 | 已結束通話 + 結束時仍在進行的通話 |

> 即時 MOS 是「目前在線通話」的品質。通話之間有空檔時，即時 RTP 欄位可能暫時沒有新值，畫面會保留上一次的數字。

---

## 8. 回應碼統計

- **失敗碼分類**：INVITE 收到 3xx–6xx 時記錄。
  - `fail_4xx`、`fail_5xx`、`fail_6xx`：依區間累計。
  - `fail_codes`：記錄每個具體回應碼的次數。
  - 3xx 會計入 `fail_codes` 與 `calls_failed`，但不屬於任何 4xx/5xx/6xx 區間。
- **GUI 右側回應碼面板**：
  - `200` = `calls_answered`（接通數）。
  - `408` = `calls_timeout`（**本機逾時數**，包含停止測試時取消的通話，不一定是伺服器真的回了 408）。
  - 其他回應碼在測試結束後由 `fail_codes` 填入。

---

## 9. 座席模式（Agent）的差異

座席模式（`core/src/agent_engine.rs`）是被叫端，計數器意義不同：

| 計數器 / 指標 | 座席模式的意義 |
|---|---|
| `calls_initiated` | 收到新來電（新的 INVITE，不含 RE-INVITE） |
| `calls_answered` | 回出 200 OK（自動接聽，幾乎等於 `calls_initiated`） |
| `calls_completed` | 收到 BYE，或 ACK / BYE 逾時後由座席主動 BYE |
| `calls_failed` / `calls_timeout` | 不使用（固定為 0） |
| ASR | 接近 100%，沒有參考價值 |
| PDD / Setup Time | **不記錄**（座席立即自動接聽，只會量到本機處理時間） |
| 通話時長 / ACD | 從送出 200 OK 到通話結束 |
| RTP 指標 | 計算方式與民眾端相同（需啟用 RTP 且 INVITE SDP 有 RTP 位址） |

ACK 逾時預設 10 秒，BYE 逾時預設 150 秒；逾時後座席主動送 BYE，並計入 completed。

---

## 10. 各介面的顯示與分級門檻

### GUI 指標列（MetricStrip）

| 欄位 | 即時值來源 | 測試結束後 |
|---|---|---|
| CPS | 快照的 `cps`（引擎每秒計算） | 最終報告的實際 CPS |
| CONCUR | `calls_active` | （保留最後的即時值） |
| SUCCESS | `calls_answered` | 同左（最終值） |
| FAILED | `calls_failed + calls_timeout` | 同左（最終值） |
| TOTAL | `calls_initiated` | 同左（最終值） |
| ASR | `asr`（分母為已有結果的通話） | 最終報告 ASR |
| ERR% | `error_rate` | `(failed + timeout) / initiated × 100` |
| PDD | `pdd_p50_ms`（p50） | 最終報告 PDD p50 |
| REG（僅座席端檢視，取代 PDD） | `agents_registered / agents_total` | （保留最後的即時值） |

座席註冊數（`agents_registered`）：每個帳號收到 REGISTER 200 OK 時計為已註冊（`Expires: 0` 除外）；以下情況取消計數：認證重試用盡、3xx–6xx 最終回應、超過 Expires 秒都沒有再收到 200 OK、送出 `Expires=0` 取消註冊、帳號的 runner 結束。同一帳號重複刷新成功不會重複計數。

### 分級門檻

| 指標 | 良好 | 普通 | 差 | 使用位置 |
|---|---|---|---|---|
| ASR | ≥ 85% | ≥ 70% | < 70% | GUI 指標列；HTML 報告（ASR 與 CCR 共用） |
| REG（已註冊座席） | 全部 | ≥ 1 | 0 | GUI 指標列（座席端） |
| ERR% | ≤ 5% | ≤ 20% | > 20% | GUI 指標列 |
| MOS | ≥ 4.0 優 | ≥ 3.0 普通 | < 3.0 差 | GUI `mosGrade()`；HTML 報告的 badge 顏色 |
| 掉包率 | < 1% | < 3% | ≥ 3% | GUI 右側面板 |
| Jitter | < 30 ms | < 60 ms | ≥ 60 ms | GUI 右側面板 |

另外有兩組 MOS 文字標籤，都以 **MOS 的整數部分**分級：

- CLI 的 `mos_label()`：5 優秀、4 良好、3 普通、2 差、其餘劣。
- HTML 報告的 `mos_label_str()`：4–5 良好、3 普通、2 差、其餘劣。

### GUI 圖表

每秒推入一個點，保留最近 90 點：

| 圖表 | 每個點的值 |
|---|---|
| CPS | 每秒 `calls_initiated` 差值 |
| 並發 | `calls_active` |
| ASR | 累計 ASR |
| CCR | 累計 CCR |
| PDD | PDD 累計平均（有樣本才推入） |
| 失敗 | 累計 `failed + timeout` |
| MOS | 進行中通話的平均 MOS（有 RTP 才推入） |

---

## 11. 已知限制與解讀注意事項

1. **停止測試會增加逾時數**：結束或手動停止時，所有尚未接通的通話都以 CANCEL 結束並計入 `calls_timeout`。高 CPS、短測試時長的 ERR% 會因此偏高。
2. **即時 CCR 偏低**：分母包含還在通話中的通話（見第 3 節）；即時 ASR / ERR% 只看已有結果的通話，沒有這個問題。
3. **GUI 圖表的時間軸**：GUI 每秒輪詢一次引擎的最新快照，兩個計時器不同步時，偶爾某一秒會重複上一秒的值或略過一秒；數值本身由引擎計算，不會出現 0 與兩倍交替。
4. **MOS 是估算值**：只由掉包率與 Jitter 推算，沒有分析音訊內容，最高約 4.41；Jitter 低於 150 ms 時不影響分數（見 6.4）。
5. **回應碼 408 不是伺服器的回應**：GUI 面板的 408 是本機逾時數（見第 8 節）。
6. **超過 24 小時的通話不計入 ACD**：通話時長直方圖的上限是 24 小時（見第 1 節）。

---

## 12. 端到端對照

`core/src/correlate.rs`。輸入為兩端的每通追蹤紀錄（`core/src/trace.rs`，時間為系統時鐘 epoch 毫秒）。

**配對**

1. `P-CIN-CallId` 相同 → 同一通。民眾端的值取自 SIPX 回的 18x / 200，座席端取自收到的 INVITE。同一通可能配到多個座席 INVITE（ACD 改派）。
2. 其餘座席 INVITE 依時間排序，找主叫號碼相同（只比數字、去掉開頭的 0，長度 ≥ 6 時允許前綴不同）且滿足 `民眾端 INVITE − 2 秒 ≤ 座席 INVITE ≤ 民眾端 INVITE + 時間窗` 的民眾端通話；兩端都有 `P-CIN-CallId` 但不同的不配。候選中優先選「還沒有座席接起」的，再取時間最晚（最接近）的一通；候選超過一通時標記為不確定。

**指標**

| 指標 | 計算 |
|---|---|
| 漏接 | 民眾端結果為接通，且配到的座席 INVITE 中沒有任何一個接起 |
| 漏接率 | 漏接 ÷ 民眾端接通 × 100% |
| 分配延遲 | 第一個座席 INVITE 時間 − 民眾端 INVITE 時間；統計平均、P50、P95、MAX（取最接近的樣本，不內插） |
| 座席接起 | 接起的座席 200 OK 時間 − 民眾端 INVITE 時間 |
| ACD 改派 | 配到兩個以上座席 INVITE 的通數；多送出的 INVITE 數 = Σ(INVITE 數 − 1) |
| 雙向 MOS | 只計民眾端與接起的座席端都有 MOS 的通話；各自取平均與最低 |

兩端不在同一台電腦時，延遲包含兩端的時鐘誤差。

---

## 13. 分時段統計

`core/src/timeline.rs`。長時間耐久測試用來看「第幾段開始變差」。時段長度為 `stats_interval_secs`（CLI `--interval`，預設 5 分鐘；0 = 不分段），從測試開始起算。

引擎另有一個每秒執行的取樣 task（與是否有進度回報無關），跨過時段邊界時以「累計值的差」結算一段：

| 欄位 | 計算 |
|---|---|
| 發起 / 接通 / 完成 / 失敗 / 逾時 | 段尾累計值 − 段首累計值（事件發生在哪一段就算哪一段） |
| CPS | 本段發起 ÷ 本段秒數 |
| ASR | 本段接通 ÷（本段接通 + 失敗 + 逾時）× 100；本段沒有任何結果時為空 |
| PDD p50 / p95 | 累計 PDD 直方圖 − 段首的直方圖，再取分位數（只含本段的樣本） |
| MOS | 本段每秒即時 MOS（進行中通話的聚合，見第 7 節）的平均；沒有 RTP 時為空 |
| 失敗碼 | 各失敗回應碼的累計次數差 |

- 測試結束時結算最後一段（不足一個時段也列出）。時間到後的收尾若不到 1 秒，併入上一段，避免圖表最後掉到 0。
- 各段的計數加總等於最終報告的總數。
- 輸出：JSON 報告的 `timeline`、HTML 報告的「分時段統計」（兩段以上才顯示：CPS / ASR / PDD p50 / MOS 趨勢圖與明細表）、CLI 表格輸出後附表、`--timeline-csv` 每段一列。
