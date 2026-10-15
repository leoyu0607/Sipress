/**
 * 前端 ↔ Rust 資料契約
 *
 * 1. config-*.json：startTest() 實際送給 `start_test` 的設定。CI 中檔案內容必須與目前程式一致，
 *    sipress-gui 的 Rust 測試會驗證它們能被 `Config` 解析、通過 validate，且沒有 Rust 不認得的欄位。
 * 2. cli-*.txt：Sidebar 的 CLI 預覽指令。sipress CLI 的 e2e 測試會驗證每個參數都能被 clap 解析。
 * 3. snapshot.json / report.json：由 Rust 測試產生，型別檢查（vue-tsc）確保前端介面與其欄位一致。
 *
 * 改了 buildRustConfig / cliCommand 後，本機執行 `npm test -- -u` 更新 fixtures，並一起 commit。
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useTestStore, type TestConfig, type RustSnapshot, type RustReport } from '../stores/testStore'
import { routeInvoke, freshPinia } from './helpers'
import snapshotFixture from '../../contract/snapshot.json'
import reportFixture from '../../contract/report.json'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn() }))

beforeEach(() => { freshPinia(); vi.useFakeTimers() })   // 假計時器：startTest 的輪詢不會真的執行
afterEach(() => { vi.useRealTimers() })

type Scenario = { name: string; apply: (c: TestConfig) => void }

const SCENARIOS: Scenario[] = [
  { name: 'caller-default', apply: () => {} },
  {
    name: 'caller-ramp-rtp',
    apply: c => {
      c.transport = 'TCP'
      c.localPort = 0
      c.duration = 120
      c.cliFormat = 'json'
      c.cliVerbose = true
      Object.assign(c.caller, {
        calleeFixed: '0912345678', totalCalls: 500, callDuration: 0,
        enableAudio: true, audioFile: 'C:/audio/prompt 1.wav', codec: 'pcmu', dtmfSequence: '123#',
      })
      Object.assign(c.caller.cpsPattern, { type: 'ramp', rampStart: 2, rampEnd: 40 })
    },
  },
  {
    name: 'caller-step',
    apply: c => {
      c.cliFormat = 'csv'
      c.caller.cpsPattern.type = 'step'
      c.caller.cpsPattern.steps = [{ cps: 5, duration_secs: 10 }, { cps: 20, duration_secs: 20 }, { cps: 60, duration_secs: 30 }]
    },
  },
  {
    name: 'caller-spike',
    apply: c => {
      c.cliFormat = 'table'
      Object.assign(c.caller.cpsPattern, { type: 'spike', spikeBase: 5, spikePeak: 200, spikeAtPct: 25, spikeDurationSecs: 15 })
    },
  },
  {
    name: 'agent',
    apply: c => {
      c.mode = 'agent'
      c.duration = 0
      c.agent.ackTimeout = 5
      c.agent.byeTimeout = 60
      c.agent.defaultDomain = 'pbx.example'
      c.agent.accounts = [
        { id: 'a', extension: '1001', username: 'u1001', password: 'p1', domain: '', status: 'idle' },
        { id: 'b', extension: '1002', username: 'u1002', password: 'p2', domain: 'other.example', status: 'registered' },
      ]
    },
  },
]

describe.each(SCENARIOS)('$name', ({ name, apply }) => {
  it('送給 start_test 的設定（Rust 端驗證 gui/contract/config-*.json）', async () => {
    const calls = routeInvoke({ start_test: () => 'started' })
    const s = useTestStore()
    apply(s.config)
    expect(s.validationErrors).toEqual([])
    await s.startTest()

    const sent = calls.find(c => c.cmd === 'start_test')?.args?.config
    expect(sent).toBeDefined()
    await expect(JSON.stringify(sent, null, 2) + '\n').toMatchFileSnapshot(`../../contract/config-${name}.json`)
  })

  it('CLI 預覽指令（CLI e2e 驗證 gui/contract/cli-*.txt）', async () => {
    const s = useTestStore()
    apply(s.config)
    await expect(s.cliCommand + '\n').toMatchFileSnapshot(`../../contract/cli-${name}.txt`)
  })
})

describe('Rust 輸出 fixtures', () => {
  // 型別層面由 vue-tsc 檢查：Rust 刪除或改名欄位時，下列指派會編譯失敗
  const snap: RustSnapshot = snapshotFixture
  const report: RustReport = reportFixture

  it('前端用到的欄位都存在於 Rust 輸出', () => {
    // 執行期再確認一次（JSON 的 null 欄位在型別上無法區分「缺少」）
    const snapKeys: (keyof RustSnapshot)[] = [
      'calls_initiated', 'calls_answered', 'calls_completed', 'calls_failed', 'calls_timeout',
      'calls_concurrent', 'rtp_sessions', 'asr', 'error_rate', 'rtp_mos', 'rtp_loss_pct',
      'rtp_jitter_ms', 'rtp_sent_packets', 'rtp_recv_packets', 'pdd_avg_ms', 'finished',
      'agents_registered', 'agents_total',
    ]
    for (const k of snapKeys) expect(snap, `snapshot.${k}`).toHaveProperty(k)
    const reportKeys: (keyof RustReport)[] = [
      'calls_initiated', 'calls_answered', 'calls_completed', 'calls_failed', 'calls_timeout',
      'asr', 'ccr', 'actual_cps', 'acd_secs', 'pdd_p50_ms', 'pdd_p95_ms', 'mos', 'loss_rate_pct',
      'jitter_ms', 'rtp_sent', 'rtp_recv', 'rtp_out_of_order', 'fail_codes',
    ]
    for (const k of reportKeys) expect(report, `report.${k}`).toHaveProperty(k)
  })
})
