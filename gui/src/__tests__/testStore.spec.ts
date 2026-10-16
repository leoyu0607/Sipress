import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { nextTick } from 'vue'
import { setActivePinia, createPinia } from 'pinia'
import { save } from '@tauri-apps/plugin-dialog'
import {
  useTestStore, mosGrade, isValidDtmf, MAX_SERIES, AGENT_REGISTER_TIMEOUT_MS,
  sipxPresetFlow, newFlowStep, expandFlowCode, flowErrors, flowToRust,
  type RustSnapshot, type RustReport,
} from '../stores/testStore'
import { routeInvoke, freshPinia } from './helpers'
import snapshotFixture from '../../contract/snapshot.json'
import reportFixture from '../../contract/report.json'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn() }))

const baseSnap: RustSnapshot = snapshotFixture
const baseReport: RustReport = reportFixture

function snap(patch: Partial<RustSnapshot>): RustSnapshot {
  return { ...baseSnap, ...patch }
}

beforeEach(freshPinia)

// ── 分級 ───────────────────────────────────────────────────────────
describe('mosGrade', () => {
  it.each([
    [4.4, 'excellent'], [4.0, 'excellent'],
    [3.99, 'fair'], [3.0, 'fair'],
    [2.99, 'poor'], [1.0, 'poor'],
    [0, 'none'],
  ])('MOS %s → %s', (mos, key) => {
    expect(mosGrade(mos).key).toBe(key)
  })
})

// ── 啟動前驗證 ─────────────────────────────────────────────────────
describe('validationErrors', () => {
  it('預設設定可直接開始', () => {
    expect(useTestStore().validationErrors).toEqual([])
  })

  it.each([
    ['', 'SIP server 不可空白'],
    ['10.0.0.1', 'SIP server 格式需為 host:port'],
    ['10.0.0.1:abc', 'SIP server 格式需為 host:port'],
  ])('server "%s" → %s', (server, msg) => {
    const s = useTestStore()
    s.config.server = server
    expect(s.validationErrors).toContain(msg)
  })

  it('民眾端欄位檢查', () => {
    const s = useTestStore()
    s.config.caller.accessNumber = ' '
    s.config.caller.concurrency = 0
    s.config.caller.cps = 0
    s.config.caller.enableAudio = true
    s.config.caller.audioFile = ''
    expect(s.validationErrors).toEqual([
      '主叫號碼不可空白', '併發數量需 ≥ 1', 'CPS 需 > 0', '已啟用播放音檔但尚未選擇檔案',
    ])
  })

  it('座席行為：秒數區間與比例', () => {
    const s = useTestStore()
    s.config.mode = 'agent'
    s.config.agent.accounts = [{ id: 'a', extension: '1001', username: 'u', password: 'p', domain: '', status: 'idle' }]
    const b = s.config.agent.behavior
    expect(s.validationErrors).toEqual([])
    Object.assign(b, { ringMin: 5, ringMax: 2 })
    expect(s.validationErrors).toEqual(['響鈴秒數最小值不可大於最大值'])
    Object.assign(b, { ringMin: 0, talkMin: -1 })
    expect(s.validationErrors).toEqual(['座席行為的秒數需 ≥ 0'])
    Object.assign(b, { talkMin: 0, rejectPct: 60, noAnswerPct: 50 })
    expect(s.validationErrors).toEqual(['拒接 + 不接比例合計不可超過 100%'])
    Object.assign(b, { rejectPct: 101, noAnswerPct: 0 })
    expect(s.validationErrors).toEqual(['拒接 / 不接比例需介於 0–100%'])
  })

  it('座席行為：預設值不出現在 CLI 預覽，舊設定補上預設值', () => {
    localStorage.setItem('sipress-config', JSON.stringify({ mode: 'agent', agent: { ackTimeout: 5 } }))
    const s = useTestStore()
    expect(s.config.agent.behavior.rejectPct).toBe(0)
    expect(s.config.agent.ackTimeout).toBe(5)
    // 預設響鈴 1–2 秒（SIPX 的 CTI 才收得到接聽事件）、註冊間隔 1000 ms
    expect(s.config.agent.behavior).toMatchObject({ ringMin: 1, ringMax: 2 })
    expect(s.cliCommand).toContain('--ring 1-2 ')
    expect(s.cliCommand).toContain('--register-interval 1000')
    expect(s.cliCommand).not.toMatch(/--talk|--reject-pct|--no-answer-pct|--wrap-up|--busy-in-call|--agent-flow/)
    expect(s.config.agent.behavior.busyInCall).toBe(false)
    Object.assign(s.config.agent.behavior, { ringMin: 2, ringMax: 2, talkMin: 10, talkMax: 60 })
    expect(s.cliCommand).toContain('--ring 2 ')
    expect(s.cliCommand).toContain('--talk 10-60')
    s.config.agent.behavior.busyInCall = true
    expect(s.cliCommand).toContain('--busy-in-call')
    expect(s.config.agent.reregisterAfterCall).toBe(false)
    expect(s.cliCommand).not.toContain('--reregister-after-call')
    s.config.agent.reregisterAfterCall = true
    expect(s.cliCommand).toContain('--reregister-after-call')
    s.config.agent.registerInterval = 0
    expect(s.cliCommand).not.toContain('--register-interval')
    s.config.agent.registerInterval = 50
    expect(s.cliCommand).toContain('--register-interval 50')
    s.config.agent.registerInterval = -1
    expect(s.validationErrors).toContain('註冊間隔需為 ≥ 0 的整數毫秒')
  })

  it('CPS 模式：漸增 / 尖峰需要持續時間，階梯至少一階', () => {
    const s = useTestStore()
    s.config.caller.duration = 0
    s.config.caller.cpsPattern.type = 'ramp'
    expect(s.validationErrors).toContain('漸增模式需設定持續時間（> 0）')
    s.config.caller.cpsPattern.type = 'spike'
    expect(s.validationErrors).toContain('尖峰模式需設定持續時間（> 0）')
    s.config.caller.cpsPattern.type = 'step'
    s.config.caller.cpsPattern.steps = []
    expect(s.validationErrors).toContain('階梯模式至少需要一個階層')
  })

  it.each([
    [0, true], [1024, true], [5070, true], [65535, true],
    [80, false], [65536, false], [5070.5, false],
  ])('本地埠 %s → 合法: %s', (port, ok) => {
    const s = useTestStore()
    s.config.localPort = port
    expect(s.validationErrors.includes('本地埠需為 0（自動）或 1024–65535')).toBe(!ok)
  })

  it('座席模式需要帳號', () => {
    const s = useTestStore()
    s.config.mode = 'agent'
    expect(s.validationErrors).toEqual(['座席模式需要至少一個帳號'])
  })

  it('驗證失敗時不呼叫後端，並寫入錯誤 log', async () => {
    const calls = routeInvoke({})
    const s = useTestStore()
    s.config.server = ''
    await s.startTest()
    expect(calls).toEqual([])
    expect(s.status).toBe('idle')
    expect(s.logs.some(l => l.level === 'err' && l.msg === 'SIP server 不可空白')).toBe(true)
  })
})

// ── 執行流程：輪詢 snapshot → 最終報告 ──────────────────────────────
describe('測試執行流程', () => {
  beforeEach(() => { vi.useFakeTimers() })
  afterEach(() => { vi.useRealTimers() })

  /** 啟動測試，get_snapshot 依序回傳 snaps（用完後持續回傳最後一個） */
  async function startWith(snaps: RustSnapshot[], report: () => RustReport | null = () => baseReport) {
    let i = 0
    const calls = routeInvoke({
      start_test:   () => 'started',
      get_snapshot: () => snaps[Math.min(i++, snaps.length - 1)],
      get_report:   report,
    })
    const s = useTestStore()
    await s.startTest()
    return { s, calls }
  }
  const tick = () => vi.advanceTimersByTimeAsync(1000)

  it('失敗數有增加時才寫 WARN，不會每秒重複', async () => {
    const { s } = await startWith([
      snap({ calls_initiated: 5, calls_failed: 0, calls_timeout: 0 }),
      snap({ calls_initiated: 10, calls_failed: 1, calls_timeout: 1 }),
      snap({ calls_initiated: 12, calls_failed: 1, calls_timeout: 1 }),
      snap({ calls_initiated: 12, calls_failed: 1, calls_timeout: 1 }),
      snap({ calls_initiated: 15, calls_failed: 1, calls_timeout: 4 }),
    ])
    for (let i = 0; i < 5; i++) await tick()
    const warns = s.logs.filter(l => l.level === 'warn').map(l => l.msg)
    expect(warns).toHaveLength(2)
    expect(warns[0]).toMatch(/^新增失敗 2（失敗 1 逾時 1）\s+發起 10/)
    expect(warns[1]).toMatch(/^新增失敗 3（失敗 1 逾時 4）\s+發起 15/)
  })

  it('每秒的 snapshot 轉成指標與圖表序列', async () => {
    const { s } = await startWith([
      snap({ calls_initiated: 10, calls_answered: 6, calls_completed: 2, calls_failed: 1, calls_timeout: 1, cps: 10, pdd_avg_ms: null, pdd_p50_ms: null, rtp_mos: null }),
      snap({ calls_initiated: 25, calls_answered: 20, calls_completed: 10, calls_failed: 2, calls_timeout: 1, cps: 7.5 }),
    ])
    expect(s.status).toBe('running')

    await tick()
    expect(s.metrics.cps).toBe(10)
    expect(s.metrics.ccr).toBe(20)
    expect(s.metrics.failed).toBe(2)
    expect(s.metrics.pdd).toBe(0)
    expect(s.series.pdd).toEqual([])          // 尚無 PDD 樣本不推點
    expect(s.rtpMetrics.enabled).toBe(false)

    await tick()
    expect(s.metrics.cps).toBe(7.5)           // 直接採用引擎算的速率，不用輪詢差值（25 − 10）
    expect(s.metrics.initiated).toBe(25)
    expect(s.metrics.ccr).toBe(40)
    expect(s.metrics.asr).toBe(baseSnap.asr)  // ASR 直接採用 Rust 的值
    expect(s.metrics.pdd).toBe(baseSnap.pdd_p50_ms) // 測試中也用 p50，與最終報告同口徑
    expect(s.series.cps).toEqual([10, 7.5])
    expect(s.series.fail).toEqual([2, 3])
    expect(s.callStates).toEqual({ initiated: 25, answered: 20, completed: 10, failed: 2, timeout: 1 })
    expect(s.rtpMetrics).toMatchObject({
      enabled: true, mos: baseSnap.rtp_mos, packetLoss: baseSnap.rtp_loss_pct,
      jitter: baseSnap.rtp_jitter_ms, packetsSent: baseSnap.rtp_sent_packets,
    })
    expect(s.elapsedSec).toBe(2)
  })

  it('引擎結束後改用最終報告的數字', async () => {
    const { s, calls } = await startWith([snap({ finished: true })])
    await tick()
    await vi.waitFor(() => expect(s.status).toBe('done'))

    expect(calls.map(c => c.cmd)).toEqual(['start_test', 'get_snapshot', 'get_report'])
    expect(s.metrics).toMatchObject({
      cps: baseReport.actual_cps,
      initiated: baseReport.calls_initiated,
      failed: baseReport.calls_failed + baseReport.calls_timeout,
      pdd: baseReport.pdd_p50_ms,               // 結束後 PDD 改為 p50
      acd: baseReport.acd_secs,
      ccr: baseReport.ccr,
      concurrency: 0,                           // 最後一筆快照的並發數（收尾前）不保留
    })
    expect(baseSnap.calls_concurrent).toBeGreaterThan(0)
    expect(s.respCodes).toMatchObject({
      ...baseReport.fail_codes,
      '200': baseReport.calls_answered,
      '408': baseReport.calls_timeout,
    })
    expect(s.rtpMetrics.outOfOrder).toBe(baseReport.rtp_out_of_order)

    // 結束後輪詢停止
    await tick()
    expect(calls.filter(c => c.cmd === 'get_snapshot')).toHaveLength(1)
  })

  it('get_report 失敗 → 狀態為 error', async () => {
    const { s } = await startWith([snap({ finished: true })], () => { throw new Error('無效的 server 位址') })
    await tick()
    await vi.waitFor(() => expect(s.status).toBe('error'))
    expect(s.logs.some(l => l.level === 'err' && l.msg.includes('無效的 server 位址'))).toBe(true)
  })

  it('start_test 失敗 → 狀態為 error 且不輪詢', async () => {
    const calls = routeInvoke({ start_test: () => { throw new Error('已有壓測正在執行') } })
    const s = useTestStore()
    await s.startTest()
    expect(s.status).toBe('error')
    await tick()
    expect(calls.map(c => c.cmd)).toEqual(['start_test'])
  })

  it(`圖表序列最多保留 ${MAX_SERIES} 點，並記錄捨棄的筆數`, async () => {
    const snaps = Array.from({ length: MAX_SERIES + 5 }, (_, i) => snap({ calls_initiated: (i + 1) * 2 }))
    const { s } = await startWith(snaps)
    for (let i = 0; i < MAX_SERIES + 5; i++) await tick()
    expect(s.series.cps).toHaveLength(MAX_SERIES)
    expect(s.seriesOffset.cps).toBe(5)
    expect(s.seriesTick).toBeGreaterThan(MAX_SERIES)  // 長度不再變化後，圖表仍靠 tick 重繪
  })

  it('手動停止：等引擎收尾（finished）才取報告', async () => {
    let finished = false
    const calls = routeInvoke({
      start_test:   () => 'started',
      stop_test:    () => { return null },
      get_snapshot: () => snap({ finished }),
      get_report:   () => baseReport,
    })
    const s = useTestStore()
    await s.startTest()
    await s.stopTest()
    expect(s.stopping).toBe(true)
    await s.stopTest()                                   // 重複停止會被忽略
    expect(calls.filter(c => c.cmd === 'stop_test')).toHaveLength(1)

    await tick()
    expect(s.status).toBe('running')                     // 還在收尾
    finished = true
    await tick()
    await vi.waitFor(() => expect(s.status).toBe('done'))
    expect(s.stopping).toBe(false)
  })

  it('stop_test 失敗（引擎已結束）→ 直接取報告', async () => {
    routeInvoke({
      start_test:   () => 'started',
      stop_test:    () => { throw new Error('沒有正在執行的壓測') },
      get_snapshot: () => snap({}),
      get_report:   () => baseReport,
    })
    const s = useTestStore()
    await s.startTest()
    await s.stopTest()
    await vi.waitFor(() => expect(s.status).toBe('done'))
  })
})

// ── 民眾端 + 座席端同時執行 ─────────────────────────────────────────
describe('雙端同時執行', () => {
  beforeEach(() => { vi.useFakeTimers() })
  afterEach(() => { vi.useRealTimers() })
  const tick = () => vi.advanceTimersByTimeAsync(1000)

  function withAccount(s: ReturnType<typeof useTestStore>) {
    s.config.agent.accounts = [{ id: 'a', extension: '1001', username: 'u', password: 'p', domain: '', status: 'idle' }]
  }

  /** 依 mode 回傳不同 snapshot，模擬後端兩個獨立 session */
  function routeByMode(finished: { caller: boolean; agent: boolean } = { caller: false, agent: false }) {
    return routeInvoke({
      start_test:   () => 'started',
      stop_test:    () => null,
      get_snapshot: a => snap({ calls_initiated: a?.mode === 'caller' ? 10 : 3, finished: finished[a?.mode as 'caller' | 'agent'] }),
      get_report:   () => baseReport,
    })
  }

  it('兩個模式可各自啟動，狀態與指標互不干擾', async () => {
    const calls = routeByMode()
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.startTest('agent')
    expect(calls.filter(c => c.cmd === 'start_test').map(c => (c.args?.config as { mode: string }).mode))
      .toEqual(['caller', 'agent'])

    await tick()
    expect(s.sessions.caller.metrics.initiated).toBe(10)
    expect(s.sessions.agent.metrics.initiated).toBe(3)

    // 畫面依檢視中的模式顯示
    s.config.mode = 'caller'
    expect(s.metrics.initiated).toBe(10)
    s.config.mode = 'agent'
    expect(s.metrics.initiated).toBe(3)
    expect(s.anyRunning).toBe(true)
  })

  it('停止其中一端不影響另一端', async () => {
    const finished = { caller: false, agent: false }
    const calls = routeByMode(finished)
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.startTest('agent')

    await s.stopTest('agent')
    expect(calls.find(c => c.cmd === 'stop_test')?.args).toEqual({ mode: 'agent' })
    finished.agent = true
    await tick()
    await vi.waitFor(() => expect(s.sessions.agent.status).toBe('done'))
    expect(s.sessions.caller.status).toBe('running')
    expect(calls.find(c => c.cmd === 'get_report')?.args).toEqual({ mode: 'agent' })
  })

  it('log 標記來源模式', async () => {
    routeByMode()
    const s = useTestStore()
    withAccount(s)
    await s.startTest('agent')
    expect(s.logs.some(l => l.mode === 'agent' && l.msg.includes('座席端'))).toBe(true)
  })

  it('雙端啟動：先座席，等待註冊後再啟動民眾端', async () => {
    const calls = routeByMode()
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(0)
    expect(s.sessions.agent.status).toBe('running')
    expect(s.sessions.caller.status).toBe('idle')

    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    await done
    expect(s.sessions.caller.status).toBe('running')
    expect(calls.filter(c => c.cmd === 'start_test').map(c => (c.args?.config as { mode: string }).mode))
      .toEqual(['agent', 'caller'])
  })

  it('雙端啟動等待期間停止座席 → 不啟動民眾端', async () => {
    const calls = routeByMode()
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(0)
    await s.stopTest('agent')
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    await done
    expect(s.sessions.caller.status).toBe('idle')
    expect(calls.filter(c => c.cmd === 'start_test')).toHaveLength(1)
  })

  it('雙端啟動需要兩端設定都有效', async () => {
    const calls = routeByMode()
    const s = useTestStore()
    await s.startBoth()                                  // 沒有座席帳號
    expect(calls).toEqual([])
    expect(s.bothValidationErrors).toEqual(['座席模式需要至少一個帳號'])
  })
})

// ── 帳號匯入 / 生成 ─────────────────────────────────────────────────
describe('座席帳號', () => {
  beforeEach(() => { routeInvoke({ register_account: () => ({ status: 'registered', message: '', expires_secs: 3600, sip_code: 200 }) }) })

  it('解析 CSV / 冒號 / 空白三種格式，略過標題與註解', () => {
    const s = useTestStore()
    s.config.agent.defaultDomain = 'pbx.local'
    const parsed = s.parseAccountText([
      '# 註解',
      'extension,username,password,domain',
      '1001,u1001,p1,other.domain',
      '1002:pw2',
      '1003 u1003 p3',
      '',
    ].join('\r\n'))
    expect(parsed.map(a => [a.extension, a.username, a.password, a.domain])).toEqual([
      ['1001', 'u1001', 'p1', 'other.domain'],
      ['1002', '1002', 'pw2', 'pbx.local'],
      ['1003', 'u1003', 'p3', 'pbx.local'],
    ])
    expect(s.accountImportError).toBe('')
  })

  it('錯誤訊息使用原始檔案的行號', () => {
    const s = useTestStore()
    s.parseAccountText(['1001,u1', '', '# x', '1002'].join('\n'))
    expect(s.accountImportError).toBe('第 1 行：需為 extension,username,password；第 4 行：無法辨識格式')
  })

  it('超過 3 個錯誤時摘要顯示', () => {
    const s = useTestStore()
    s.parseAccountText(['a', 'b', 'c', 'd', 'e'].join('\n'))
    expect(s.accountImportError).toMatch(/…（共 5 行錯誤）$/)
  })

  it('匯入時依分機去重（含既有帳號與檔案內重複）', () => {
    const s = useTestStore()
    s.importAccountText('1001,a,p\n1002,b,p')
    s.importAccountText('1002,b,p\n1003,c,p\n1003,c,p')
    expect(s.config.agent.accounts.map(a => a.extension)).toEqual(['1001', '1002', '1003'])
    expect(s.config.agent.count).toBe(3)
    expect(s.logs[s.logs.length - 1]?.msg).toBe('匯入 1 個 SIP 帳號（略過 2 個重複分機）')
  })

  it('生成帳號：密碼模式與 {ext} 全部替換，略過重複分機', () => {
    const s = useTestStore()
    s.importAccountText('2001,x,p')
    const base = { count: 3, startExt: 2000, usernamePrefix: 'agent', samePassword: 'same', customPassword: '{ext}-pw-{ext}', domain: 'd' }
    s.generateAccounts({ ...base, passwordMode: 'custom' })
    expect(s.config.agent.accounts.map(a => a.extension)).toEqual(['2001', '2000', '2002'])
    const byExt = Object.fromEntries(s.config.agent.accounts.map(a => [a.extension, a]))
    expect(byExt['2001'].password).toBe('p')              // 既有帳號不被覆蓋
    expect(byExt['2000'].password).toBe('2000-pw-2000')
    expect(byExt['2002'].username).toBe('agent2002')

    s.clearAccounts()
    s.generateAccounts({ ...base, count: 1, passwordMode: 'ext' })
    expect(s.config.agent.accounts[0].password).toBe('2000')
    s.clearAccounts()
    s.generateAccounts({ ...base, count: 1, passwordMode: 'same' })
    expect(s.config.agent.accounts[0].password).toBe('same')
  })
})

// ── 設定保存 ───────────────────────────────────────────────────────
describe('設定保存', () => {
  it('變更後寫入 localStorage，重開後還原', async () => {
    const s = useTestStore()
    s.config.server = '10.1.1.1:5060'
    s.config.caller.cpsPattern.type = 'ramp'
    await nextTick()

    setActivePinia(createPinia())
    const reopened = useTestStore()
    expect(reopened.config.server).toBe('10.1.1.1:5060')
    expect(reopened.config.caller.cpsPattern.type).toBe('ramp')
    expect(reopened.config.caller.cpsPattern.steps).toHaveLength(2)  // 未保存的欄位用預設值補齊
  })

  it('不合法的 transport 回退成 UDP，帳號註冊狀態不保留', () => {
    localStorage.setItem('sipress-config', JSON.stringify({
      transport: 'TLS',
      agent: { accounts: [{ id: 'a', extension: '1', username: 'u', password: 'p', domain: '', status: 'registered' }] },
    }))
    const s = useTestStore()
    expect(s.config.transport).toBe('UDP')
    expect(s.config.agent.accounts[0].status).toBe('idle')
    expect(s.config.agent.ackTimeout).toBe(10)
  })

  it('損毀的資料 → 使用預設值', () => {
    localStorage.setItem('sipress-config', '{not json')
    expect(useTestStore().validationErrors).toEqual([])
  })
})

// ── CLI 預覽 ───────────────────────────────────────────────────────
describe('CLI 預覽', () => {
  it('座席模式產生 --mode agent 指令，不帶民眾端專屬參數', () => {
    const s = useTestStore()
    s.config.mode = 'agent'
    s.config.caller.cpsPattern.type = 'step'
    s.config.caller.enableAudio = true
    s.config.caller.audioFile = 'a.wav'
    s.config.caller.dtmfSequence = '1,#'
    expect(s.cliCommand).toContain('--mode agent --accounts sipress_accounts.csv')
    expect(s.cliCommand).not.toContain('--cps')
    expect(s.cliCommand).not.toContain('--dtmf')
  })

  it('CPS 模式、codec、DTMF 都轉成 CLI 參數', () => {
    const s = useTestStore()
    const pat = s.config.caller.cpsPattern
    expect(s.cliCommand).toContain(`--cps ${s.config.caller.cps} `)
    expect(s.cliCommand).not.toContain('--codec')

    pat.type = 'ramp'; pat.rampStart = 1; pat.rampEnd = 50
    expect(s.cliCommand).toContain('--cps-ramp 1-50')
    expect(s.cliCommand).not.toContain('--cps ')
    pat.type = 'step'; pat.steps = [{ cps: 10, duration_secs: 30 }, { cps: 50, duration_secs: 60 }]
    expect(s.cliCommand).toContain('--cps-steps 10:30,50:60')
    pat.type = 'spike'; pat.spikeBase = 10; pat.spikePeak = 100; pat.spikeAtPct = 50; pat.spikeDurationSecs = 10
    expect(s.cliCommand).toContain('--cps-spike 10,100,50,10')

    s.config.caller.codec = 'pcmu'
    expect(s.cliCommand).toContain('--codec pcmu')

    s.config.caller.dtmfSequence = '1,w2000,#'
    expect(s.cliCommand).not.toContain('--dtmf')   // 沒有 RTP 時不會送 DTMF
    s.config.caller.enableAudio = true
    s.config.caller.audioFile = 'a.wav'
    expect(s.cliCommand).toContain('--rtp --rtp-port 16000 --audio "a.wav" --dtmf "1,w2000,#"')
  })
})

describe('DTMF 驗證', () => {
  it('格式與 core 相同', () => {
    expect(isValidDtmf('1,w2000,2,w500,#')).toBe(true)
    expect(isValidDtmf('*, d ,W100')).toBe(true)
    for (const bad of ['', ',', '12', 'x', 'w', 'wabc', '1;2']) expect(isValidDtmf(bad)).toBe(false)
  })

  it('格式錯誤時無法開始民眾端', () => {
    const s = useTestStore()
    s.config.caller.dtmfSequence = '12#'
    expect(s.validationErrors.some(e => e.startsWith('DTMF 序列格式錯誤'))).toBe(true)
    s.config.caller.dtmfSequence = '1,2,#'
    expect(s.validationErrors).toEqual([])
  })
})

// ── 匯出 ───────────────────────────────────────────────────────────
describe('匯出', () => {
  it('CSV 的秒數從被捨棄的筆數接續', async () => {
    vi.mocked(save).mockResolvedValue('/tmp/out.csv')
    const calls = routeInvoke({ save_text_file: () => null })
    const s = useTestStore()
    s.series.cps = [1, 2]
    s.series.conc = [3, 4]
    s.series.asr = [50, 75]
    s.series.ccr = [10, 20]
    s.series.fail = [0, 1]
    s.seriesOffset.cps = 7
    await s.exportCsv()
    expect(calls[0].args).toEqual({
      path: '/tmp/out.csv',
      contents: 'sec,cps,conc,asr,ccr,fail\n7,1.00,3,50.00,10.00,0\n8,2.00,4,75.00,20.00,1',
    })
    expect(s.logs[s.logs.length - 1]?.msg).toBe('CSV 報告 已匯出 → /tmp/out.csv')
  })

  it('取消另存新檔不寫檔', async () => {
    vi.mocked(save).mockResolvedValue(null)
    const calls = routeInvoke({})
    await useTestStore().exportJson()
    expect(calls).toEqual([])
  })
})

// ── 座席流程 ───────────────────────────────────────────────────────
describe('座席流程', () => {
  it('{agent} 取分機末幾碼、{pwd} 代入密碼（與 core 一致）', () => {
    const f = { agentDigits: 4, agentPassword: '000000' }
    expect(expandFlowCode('*94{agent}*{pwd}', '0000021113333001', f)).toBe('*943001*000000')
    expect(expandFlowCode('*94{agent}', '1001', { ...f, agentDigits: 0 })).toBe('*941001')
    expect(expandFlowCode('{agent}', '1001', { ...f, agentDigits: 8 })).toBe('1001')
  })

  it('SIPX 預設：開始登入 → 等待 → 就緒；每通後就緒；結束未就緒 → 登出', () => {
    const s = useTestStore()
    const preset = sipxPresetFlow(s.config.agent.flow.codes)
    const codes = (steps: typeof preset.onStart) => steps.map(st => st.type === 'dial' ? st.code : `wait ${st.secs}`)
    expect(codes(preset.onStart)).toEqual(['*94{agent}*{pwd}', 'wait 1', '*96'])
    expect(codes(preset.afterCall)).toEqual(['*96'])
    expect(codes(preset.onStop)).toEqual(['*97', '*95'])
  })

  it('驗證：空白撥碼、非法字元、用了 {pwd} 卻沒設密碼', () => {
    const s = useTestStore()
    s.config.mode = 'agent'
    s.config.agent.accounts = [{ id: 'a', extension: '1001', username: 'u', password: 'p', domain: '', status: 'idle' }]
    const flow = s.config.agent.flow
    flow.onStart = [newFlowStep('dial', '*94{agent}*{pwd}', '登入')]
    expect(s.validationErrors).toContain('座席流程使用 {pwd}，但尚未設定座席密碼')
    flow.agentPassword = '000000'
    expect(s.validationErrors).toEqual([])
    flow.afterCall = [newFlowStep('dial', ' '), newFlowStep('dial', '*96;x')]
    expect(flowErrors(flow)).toEqual(['座席流程的撥碼不可空白', '座席流程的撥碼「*96;x」含不支援的字元'])
    flow.afterCall = [newFlowStep('wait', '', '', -1)]
    expect(flowErrors(flow)).toEqual(['座席流程的等待秒數需 ≥ 0'])
  })

  it('有流程時 CLI 預覽帶 --agent-flow，送給 Rust 的格式與 --agent-flow 檔案相同', () => {
    const s = useTestStore()
    s.config.mode = 'agent'
    expect(s.cliCommand).not.toContain('--agent-flow')
    Object.assign(s.config.agent.flow, sipxPresetFlow(s.config.agent.flow.codes))
    expect(s.cliCommand).toContain('--agent-flow sipress_flow.json')
    expect(flowToRust(s.config.agent.flow).on_start).toEqual([
      { type: 'dial', code: '*94{agent}*{pwd}', label: '登入' },
      { type: 'wait', secs: 1 },
      { type: 'dial', code: '*96', label: '就緒' },
    ])
  })

  it('舊設定沒有 flow 欄位時補上預設值（含常用碼）', () => {
    localStorage.setItem('sipress-config', JSON.stringify({ mode: 'agent', agent: { flow: { agentPassword: '1234', codes: { ready: '*86' } } } }))
    const f = useTestStore().config.agent.flow
    expect(f.agentPassword).toBe('1234')
    expect(f.onStart).toEqual([])
    expect(f.codes).toEqual({ login: '*94{agent}*{pwd}', ready: '*86', notReady: '*97', logout: '*95' })
  })

  it('手動撥碼送出常用碼；座席端沒在跑時記錯誤', async () => {
    const calls = routeInvoke({ agent_run_steps: () => 'sent' })
    const s = useTestStore()
    await s.runAgentCode('ready')
    expect(calls.find(c => c.cmd === 'agent_run_steps')?.args).toEqual({ steps: [{ type: 'dial', code: '*96', label: '就緒' }] })

    routeInvoke({ agent_run_steps: () => { throw new Error('座席端沒有正在執行的壓測') } })
    await s.runAgentCode('logout')
    expect(s.logs.some(l => l.level === 'err' && l.msg.includes('手動撥碼「登出」失敗'))).toBe(true)
  })

  it('撥碼結果：快照有新結果時記錄累計，結束時逐項列出', async () => {
    routeInvoke({
      start_test:   () => 'started',
      get_snapshot: () => snap({ agent_dials_ok: 2, agent_dials_failed: 1, finished: true }),
      get_report:   () => ({ ...baseReport, agent_dials: [
        { code: '*96', label: '就緒', ok: 2, failed: 0, fail_codes: {} },
        { code: '*97', label: '未就緒', ok: 0, failed: 1, fail_codes: { 486: 1 } },
      ] }),
    })
    vi.useFakeTimers()
    try {
      const s = useTestStore()
      s.config.mode = 'agent'
      s.config.agent.accounts = [{ id: 'a', extension: '1001', username: 'u', password: 'p', domain: '', status: 'idle' }]
      await s.startTest('agent')
      await vi.advanceTimersByTimeAsync(1500)
      const msgs = s.logs.map(l => l.msg)
      expect(msgs.some(m => m.includes('座席撥碼累計：成功 2  失敗 1'))).toBe(true)
      expect(msgs.some(m => m.includes('撥碼 就緒（*96）：成功 2  失敗 0'))).toBe(true)
      expect(msgs.some(m => m.includes('撥碼 未就緒（*97）：成功 0  失敗 1  486×1'))).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })
})
