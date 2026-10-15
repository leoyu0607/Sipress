/// 民眾端 + 座席端同時執行（commit d66aa65）的補充測試：覆蓋 store 的 per-mode 行為與邊界情況
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { save } from '@tauri-apps/plugin-dialog'
import { useTestStore, AGENT_REGISTER_TIMEOUT_MS, type RustSnapshot, type RustReport, type CallMode } from '../stores/testStore'
import { routeInvoke, freshPinia } from './helpers'
import snapshotFixture from '../../contract/snapshot.json'
import reportFixture from '../../contract/report.json'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn() }))

const baseSnap: RustSnapshot = snapshotFixture
const baseReport: RustReport = reportFixture
const snap = (patch: Partial<RustSnapshot>): RustSnapshot => ({ ...baseSnap, ...patch })

type Store = ReturnType<typeof useTestStore>
type Args = Record<string, unknown> | undefined
const modeOf = (a: Args) => a?.mode as CallMode

beforeEach(freshPinia)

function withAccount(s: Store) {
  s.config.agent.accounts = [{ id: 'a', extension: '1001', username: 'u', password: 'p', domain: '', status: 'idle' }]
}

describe('雙端：每個 invoke 帶正確的 mode (D1)', () => {
  it.each(['caller', 'agent'] as const)('%s：get_snapshot / get_report / stop_test / get_html_report', async mode => {
    vi.useFakeTimers()
    try {
      vi.mocked(save).mockResolvedValue('/tmp/out.html')
      const calls = routeInvoke({
        start_test:      () => 'started',
        stop_test:       () => null,
        get_snapshot:    () => snap({ finished: true }),
        get_report:      () => baseReport,
        get_html_report: () => '<html></html>',
        save_text_file:  () => null,
      })
      const s = useTestStore()
      withAccount(s)
      s.config.mode = mode
      await s.startTest()
      await vi.advanceTimersByTimeAsync(1000)
      await vi.waitFor(() => expect(s.status).toBe('done'))
      await s.stopTest()
      await s.exportHtml()

      for (const cmd of ['get_snapshot', 'get_report', 'stop_test', 'get_html_report']) {
        const hit = calls.filter(c => c.cmd === cmd)
        expect(hit.length, cmd).toBeGreaterThan(0)
        for (const c of hit) expect(c.args?.mode, cmd).toBe(mode)
      }
      const html = calls.find(c => c.cmd === 'get_html_report')!
      expect(html.args).toMatchObject({ serverAddr: s.config.server })
      expect(html.args?.timestamp).toMatch(/^\d{8}_\d{6}$/)
    } finally {
      vi.useRealTimers()
    }
  })
})

describe('雙端：狀態隔離', () => {
  beforeEach(() => { vi.useFakeTimers() })
  afterEach(() => { vi.useRealTimers() })
  const tick = () => vi.advanceTimersByTimeAsync(1000)

  /** 每個模式有自己的累計 initiated 序列 */
  function routeSeq(seq: Record<CallMode, number[]>, extra: Record<string, (a?: Args) => unknown> = {}) {
    const idx = { caller: 0, agent: 0 }
    return routeInvoke({
      start_test:   () => 'started',
      stop_test:    () => null,
      get_snapshot: a => {
        const m = modeOf(a)
        const arr = seq[m]
        return snap({ calls_initiated: arr[Math.min(idx[m]++, arr.length - 1)] })
      },
      get_report:   () => baseReport,
      ...extra,
    })
  }

  it('D2：切換檢視模式不會重置執行中那端的累積資料', async () => {
    routeSeq({ caller: [10, 20, 30, 40], agent: [1, 2, 3, 4] })
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await tick(); await tick()
    expect(s.series.cps).toEqual([10, 10])

    s.config.mode = 'agent'
    expect(s.series.cps).toEqual([])           // 座席端尚未啟動
    expect(s.status).toBe('idle')
    await tick()
    s.config.mode = 'caller'
    expect(s.series.cps).toEqual([10, 10, 10]) // 持續累積
    expect(s.metrics.initiated).toBe(30)
    expect(s.elapsedSec).toBe(3)
    expect(s.status).toBe('running')
  })

  it('D3：一端 start_test 失敗，不改變另一端狀態', async () => {
    routeInvoke({
      start_test: a => {
        if (modeOf((a as { config: Args }).config) === 'agent') throw new Error('座席端已有壓測正在執行')
        return 'started'
      },
      get_snapshot: () => snap({}),
    })
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    expect(await s.startTest('agent')).toBe(false)
    expect(s.sessions.agent.status).toBe('error')
    expect(s.sessions.caller.status).toBe('running')
    await tick()
    expect(s.sessions.caller.metrics.initiated).toBe(baseSnap.calls_initiated)
    expect(s.sessions.agent.metrics.initiated).toBe(0)
    expect(s.logs.some(l => l.level === 'err' && l.mode === 'agent' && l.msg.includes('啟動失敗'))).toBe(true)
  })

  it('D4：一端 get_report 失敗，只有該端 error', async () => {
    routeInvoke({
      start_test:   () => 'started',
      get_snapshot: () => snap({ finished: true }),
      get_report:   a => { if (modeOf(a) === 'agent') throw new Error('座席端引擎失敗'); return baseReport },
    })
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.startTest('agent')
    await tick()
    await vi.waitFor(() => {
      expect(s.sessions.caller.status).toBe('done')
      expect(s.sessions.agent.status).toBe('error')
    })
    expect(s.logs.some(l => l.mode === 'agent' && l.level === 'err' && l.msg.includes('座席端引擎失敗'))).toBe(true)
    expect(s.logs.some(l => l.mode === 'caller' && l.level === 'err')).toBe(false)
  })

  it('D5：一端完成後重新啟動，只重置該端；seriesTick 遞增', async () => {
    let callerFinished = false
    routeInvoke({
      start_test:   () => 'started',
      get_snapshot: a => snap({ calls_initiated: modeOf(a) === 'caller' ? 10 : 3, finished: modeOf(a) === 'caller' && callerFinished }),
      get_report:   () => baseReport,
    })
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.startTest('agent')
    await tick()
    callerFinished = true
    await tick()
    await vi.waitFor(() => expect(s.sessions.caller.status).toBe('done'))

    const agentBefore = JSON.stringify(s.sessions.agent)
    const tickBefore = s.sessions.caller.seriesTick
    callerFinished = false
    await s.startTest('caller')
    expect(s.sessions.caller.status).toBe('running')
    expect(s.sessions.caller.series.cps).toEqual([])
    expect(s.sessions.caller.metrics.initiated).toBe(0)
    expect(s.sessions.caller.elapsedSec).toBe(0)
    expect(s.sessions.caller.seriesTick).toBeGreaterThan(tickBefore)
    expect(JSON.stringify(s.sessions.agent)).toBe(agentBefore)
  })

  it('D6：progressPct / elapsedSec 依檢視中的模式', async () => {
    routeSeq({ caller: [1], agent: [1] })
    const s = useTestStore()
    withAccount(s)
    s.config.caller.duration = 10
    s.config.agent.duration = 10
    await s.startTest('caller')
    for (let i = 0; i < 4; i++) await tick()
    await s.startTest('agent')
    await tick(); await tick()

    s.config.mode = 'caller'
    expect(s.elapsedSec).toBe(6)
    expect(s.progressPct).toBe(60)
    s.config.mode = 'agent'
    expect(s.elapsedSec).toBe(2)
    expect(s.progressPct).toBe(20)
  })

  it('D17：兩端輪詢同時進行，CPS 差值互不污染', async () => {
    routeSeq({ caller: [100, 110, 130], agent: [5, 8, 8] })
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.startTest('agent')
    await tick(); await tick(); await tick()
    expect(s.sessions.caller.series.cps).toEqual([100, 10, 20])
    expect(s.sessions.agent.series.cps).toEqual([5, 3, 0])
  })
})

describe('雙端啟動 / 全部停止', () => {
  beforeEach(() => { vi.useFakeTimers() })
  afterEach(() => { vi.useRealTimers() })

  function route(handlers: Record<string, (a?: Args) => unknown> = {}) {
    return routeInvoke({
      start_test:   () => 'started',
      stop_test:    () => null,
      get_snapshot: () => snap({}),
      get_report:   () => baseReport,
      ...handlers,
    })
  }
  const startCount = (calls: { cmd: string }[]) => calls.filter(c => c.cmd === 'start_test').length

  it('D7：startBoth 連按兩次只啟動一組；任一端執行中時不動作', async () => {
    const calls = route()
    const s = useTestStore()
    withAccount(s)
    const first = s.startBoth()
    const second = s.startBoth()
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    await Promise.all([first, second])
    expect(startCount(calls)).toBe(2)
    expect(s.sessions.caller.status).toBe('running')
    expect(s.sessions.agent.status).toBe('running')

    // 兩端都在跑時再按
    await s.startBoth()
    expect(startCount(calls)).toBe(2)
  })

  it('D7：只有一端執行中時 startBoth 不動作', async () => {
    const calls = route()
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.startBoth()
    expect(startCount(calls)).toBe(1)
    expect(s.sessions.agent.status).toBe('idle')
  })

  it('D8：座席端 start_test 失敗 → 不啟動民眾端', async () => {
    const calls = route({ start_test: () => { throw new Error('boom') } })
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    await done
    expect(startCount(calls)).toBe(1)
    expect(s.sessions.agent.status).toBe('error')
    expect(s.sessions.caller.status).toBe('idle')
  })

  it('D9：等待期間 stopAll → 民眾端不啟動，promise 會結束', async () => {
    const calls = route()
    const s = useTestStore()
    withAccount(s)
    let settled = false
    const done = s.startBoth().then(() => { settled = true })
    await vi.advanceTimersByTimeAsync(0)
    expect(s.sessions.agent.status).toBe('running')

    await s.stopAll()
    await vi.advanceTimersByTimeAsync(0)   // 不需等滿 warmup
    expect(settled).toBe(true)
    await done
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    expect(startCount(calls)).toBe(1)
    expect(s.sessions.caller.status).toBe('idle')
  })

  it('D9：等待期間對尚未啟動的民眾端 stopTest → 取消雙端啟動，民眾端不啟動', async () => {
    const calls = route()
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(0)
    expect(s.sessions.agent.status).toBe('running')

    await s.stopTest('caller')
    await done
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    expect(startCount(calls)).toBe(1)
    expect(s.sessions.caller.status).not.toBe('running')
    expect(s.sessions.agent.status).toBe('running')     // 座席端不受影響
  })

  it('D10：stopAll 只對執行中的模式送 stop_test；都沒在跑時不送任何 invoke', async () => {
    const calls = route()
    const s = useTestStore()
    withAccount(s)
    await s.stopAll()
    expect(calls).toEqual([])

    await s.startTest('agent')
    calls.length = 0
    await s.stopAll()
    const stops = calls.filter(c => c.cmd === 'stop_test')
    expect(stops).toHaveLength(1)
    expect(stops[0].args).toEqual({ mode: 'agent' })
  })

  it('D10：兩端都在跑時 stopAll 兩端各送一次', async () => {
    const calls = route()
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.startTest('agent')
    await s.stopAll()
    expect(calls.filter(c => c.cmd === 'stop_test').map(c => modeOf(c.args)).sort()).toEqual(['agent', 'caller'])
    expect(s.sessions.caller.stopping).toBe(true)
    expect(s.sessions.agent.stopping).toBe(true)
  })

  it('D11：stopTest 對未執行的模式 → stop_test 失敗則直接取報告，不影響另一端', async () => {
    const calls = route({ stop_test: () => { throw new Error('沒有正在執行的壓測') } })
    const s = useTestStore()
    withAccount(s)
    await s.startTest('caller')
    await s.stopTest('agent')
    await vi.waitFor(() => expect(s.sessions.agent.status).not.toBe('idle'))
    expect(s.sessions.agent.status).toBe('done')
    expect(s.sessions.agent.stopping).toBe(false)
    expect(calls.filter(c => c.cmd === 'get_report').every(c => modeOf(c.args) === 'agent')).toBe(true)
    expect(s.sessions.caller.status).toBe('running')
    expect(s.sessions.caller.stopping).toBe(false)
  })
})

describe('驗證 (D12 / D13)', () => {
  it('D12：共用欄位錯誤只出現一次；兩端各自的錯誤都列出', () => {
    const s = useTestStore()
    s.config.server = ''
    s.config.caller.accessNumber = ''
    expect(s.bothValidationErrors).toEqual(['SIP server 不可空白', '座席模式需要至少一個帳號', '主叫號碼不可空白'])
    expect(s.bothValidationErrors.filter(e => e === 'SIP server 不可空白')).toHaveLength(1)
  })

  it('D12：兩端皆有效 → 無錯誤', () => {
    const s = useTestStore()
    withAccount(s)
    expect(s.bothValidationErrors).toEqual([])
  })

  it('D13：validationErrors 只依檢視中的模式', () => {
    const s = useTestStore()
    s.config.caller.accessNumber = ''
    expect(s.validationErrors).toEqual(['主叫號碼不可空白'])
    s.config.mode = 'agent'
    expect(s.validationErrors).toEqual(['座席模式需要至少一個帳號'])
    withAccount(s)
    expect(s.validationErrors).toEqual([])
    s.config.mode = 'caller'
    expect(s.validationErrors).toEqual(['主叫號碼不可空白'])
  })
})

describe('匯出 (D14)', () => {
  it.each(['caller', 'agent'] as const)('%s：JSON / CSV 用檢視中模式的資料，檔名含模式', async mode => {
    vi.mocked(save).mockReset()
    vi.mocked(save).mockResolvedValue('/tmp/out')
    const calls = routeInvoke({ save_text_file: () => null })
    const s = useTestStore()
    s.sessions.caller.series.cps = [11]; s.sessions.caller.series.conc = [1]
    s.sessions.caller.series.asr = [1]; s.sessions.caller.series.ccr = [1]; s.sessions.caller.series.fail = [0]
    s.sessions.agent.series.cps = [22]; s.sessions.agent.series.conc = [2]
    s.sessions.agent.series.asr = [2]; s.sessions.agent.series.ccr = [2]; s.sessions.agent.series.fail = [0]
    s.sessions.caller.metrics.initiated = 111
    s.sessions.agent.metrics.initiated = 222
    s.config.mode = mode

    await s.exportJson()
    await s.exportCsv()

    const paths = vi.mocked(save).mock.calls.map(c => (c[0] as { defaultPath: string }).defaultPath)
    expect(paths[0]).toMatch(new RegExp(`^sipress_${mode}_\\d{8}_\\d{6}\\.json$`))
    expect(paths[1]).toMatch(new RegExp(`^sipress_${mode}_\\d{8}_\\d{6}\\.csv$`))

    const writes = calls.filter(c => c.cmd === 'save_text_file').map(c => String(c.args?.contents))
    const json = JSON.parse(writes[0])
    expect(json.mode).toBe(mode)
    expect(json.metrics.initiated).toBe(mode === 'caller' ? 111 : 222)
    expect(json.series.cps).toEqual([mode === 'caller' ? 11 : 22])
    expect(writes[1].split('\n')[1].split(',')[1]).toBe(mode === 'caller' ? '11.00' : '22.00')
  })
})

describe('log 的 mode 標記 (D15)', () => {
  beforeEach(() => { vi.useFakeTimers() })
  afterEach(() => { vi.useRealTimers() })

  it('session 相關訊息帶 mode；startBoth 提示與帳號管理訊息不帶', async () => {
    routeInvoke({
      start_test: () => 'started', stop_test: () => null,
      get_snapshot: () => snap({ calls_failed: 1 }), get_report: () => baseReport,
      register_account: () => ({ status: 'registered', message: '', expires_secs: 3600, sip_code: 200 }),
    })
    const s = useTestStore()
    s.generateAccounts({ count: 1, startExt: 1001, usernamePrefix: '', passwordMode: 'ext', samePassword: '', customPassword: '', domain: '' })
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    await done
    await s.stopAll()

    const hint = s.logs.find(l => l.msg.startsWith('雙端啟動'))!
    expect(hint).toBeTruthy()
    expect(hint.mode).toBeUndefined()
    const gen = s.logs.find(l => l.msg.startsWith('生成'))!
    expect(gen.mode).toBeUndefined()
    expect(s.logs.filter(l => l.msg.includes('註冊成功')).every(l => l.mode === undefined)).toBe(true)

    for (const m of ['agent', 'caller'] as const) {
      expect(s.logs.some(l => l.mode === m && l.msg.startsWith('測試開始')), `${m} 測試開始`).toBe(true)
      expect(s.logs.some(l => l.mode === m && l.msg.startsWith('失敗')), `${m} 失敗`).toBe(true)
      expect(s.logs.some(l => l.mode === m && l.msg.startsWith('測試已手動停止')), `${m} 停止`).toBe(true)
    }
    // 所有「測試開始 / 手動停止 / 失敗 / 測試完成」都必須帶 mode
    expect(s.logs.filter(l => /^(測試開始|測試已手動停止|失敗|測試完成)/.test(l.msg)).every(l => !!l.mode)).toBe(true)
  })
})

describe('設定載入 (D16)', () => {
  it('舊版 localStorage（含 mode: agent）→ 檢視模式正確，兩端皆 idle', () => {
    localStorage.setItem('sipress-config', JSON.stringify({
      server: '10.0.0.9:5060', mode: 'agent',
      agent: { accounts: [{ id: 'x', extension: '1', username: 'u', password: 'p', domain: '', status: 'registered' }] },
    }))
    const s = useTestStore()
    expect(s.config.mode).toBe('agent')
    expect(s.status).toBe('idle')
    expect(s.sessions.caller.status).toBe('idle')
    expect(s.sessions.agent.status).toBe('idle')
    expect(s.anyRunning).toBe(false)
    expect(s.config.agent.accounts[0].status).toBe('idle')
    expect(s.validationErrors).toEqual([])
  })
})

describe('本地埠只套用在民眾端', () => {
  it('民眾端送出 local_addr，座席端為 null；0 = 自動', async () => {
    const calls = routeInvoke({ start_test: () => 'started' })
    const s = useTestStore()
    withAccount(s)
    s.config.localPort = 5070
    await s.startTest('caller')
    await s.startTest('agent')
    const sent = calls.filter(c => c.cmd === 'start_test').map(c => c.args?.config as { mode: string; local_addr: string | null })
    expect(sent.map(c => [c.mode, c.local_addr])).toEqual([['caller', '0.0.0.0:5070'], ['agent', null]])
  })

  it('本地埠為 0 時民眾端也送 null，CLI 預覽不帶 --local', () => {
    const s = useTestStore()
    s.config.localPort = 0
    expect(s.cliCommand).not.toContain('--local')
  })
})

describe('雙端啟動等待座席註冊', () => {
  beforeEach(() => { vi.useFakeTimers() })
  afterEach(() => { vi.useRealTimers() })

  /** 座席端快照回報 registered/total；民眾端維持一般快照 */
  function routeAgents(registered: number, total: number) {
    return routeInvoke({
      start_test:   () => 'started',
      stop_test:    () => null,
      get_snapshot: a => modeOf(a) === 'agent'
        ? snap({ agents_registered: registered, agents_total: total })
        : snap({ agents_registered: 0, agents_total: 0 }),
      get_report:   () => baseReport,
    })
  }
  const startedModes = (calls: { cmd: string; args?: Args }[]) =>
    calls.filter(c => c.cmd === 'start_test').map(c => (c.args?.config as { mode: string }).mode)

  it('座席全部註冊成功 → 不等逾時就開始撥號', async () => {
    const calls = routeAgents(2, 2)
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(1500)              // 第一筆快照（1s）後下一次檢查
    await done
    expect(startedModes(calls)).toEqual(['agent', 'caller'])
    expect(s.sessions.agent.agents).toEqual({ registered: 2, total: 2 })
    expect(s.logs.some(l => l.level === 'ok' && l.msg.includes('座席全部註冊成功（2/2）'))).toBe(true)
  })

  it('逾時但部分成功 → 繼續撥號並警告', async () => {
    const calls = routeAgents(1, 2)
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS - 500)
    expect(startedModes(calls)).toEqual(['agent'])       // 逾時前還在等
    await vi.advanceTimersByTimeAsync(1000)
    await done
    expect(startedModes(calls)).toEqual(['agent', 'caller'])
    expect(s.logs.some(l => l.level === 'warn' && l.msg.includes('僅 1/2 個座席註冊成功'))).toBe(true)
  })

  it('逾時且沒有任何座席註冊成功 → 不撥號，座席端繼續執行', async () => {
    const calls = routeAgents(0, 2)
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(AGENT_REGISTER_TIMEOUT_MS + 1000)
    await done
    expect(startedModes(calls)).toEqual(['agent'])
    expect(s.sessions.caller.status).toBe('idle')
    expect(s.sessions.agent.status).toBe('running')
    expect(s.logs.some(l => l.level === 'err' && l.msg.includes('沒有任何帳號註冊成功'))).toBe(true)
  })

  it('等待中座席引擎自行結束（例如設定錯誤）→ 不撥號', async () => {
    const calls = routeInvoke({
      start_test:   () => 'started',
      get_snapshot: () => snap({ agents_registered: 0, agents_total: 2, finished: true }),
      get_report:   () => { throw new Error('無效的 server 位址') },
    })
    const s = useTestStore()
    withAccount(s)
    const done = s.startBoth()
    await vi.advanceTimersByTimeAsync(2000)
    await done
    expect(startedModes(calls)).toEqual(['agent'])
    expect(s.sessions.caller.status).toBe('idle')
  })

  it('座席端檢視：metrics 之外另提供 agents 註冊狀態', async () => {
    routeAgents(48, 50)
    const s = useTestStore()
    withAccount(s)
    s.config.mode = 'agent'
    await s.startTest()
    await vi.advanceTimersByTimeAsync(1000)
    expect(s.agents).toEqual({ registered: 48, total: 50 })
    s.config.mode = 'caller'
    expect(s.agents).toEqual({ registered: 0, total: 0 })
  })
})

describe('持續時間兩端分開設定', () => {
  it('各端送出自己的 duration_secs，CLI 預覽跟著檢視中的模式', async () => {
    const calls = routeInvoke({ start_test: () => 'started' })
    const s = useTestStore()
    withAccount(s)
    s.config.caller.duration = 120
    s.config.agent.duration = 300
    await s.startTest('caller')
    await s.startTest('agent')
    const sent = calls.filter(c => c.cmd === 'start_test').map(c => c.args?.config as { mode: string; duration_secs: number })
    expect(sent.map(c => [c.mode, c.duration_secs])).toEqual([['caller', 120], ['agent', 300]])

    expect(s.cliCommand).toContain('-d 120')
    s.config.mode = 'agent'
    expect(s.cliCommand).toContain('-d 300')
  })

  it('舊版 localStorage 的共用 duration → 兩端都沿用', () => {
    localStorage.setItem('sipress-config', JSON.stringify({ duration: 45 }))
    const s = useTestStore()
    expect(s.config.caller.duration).toBe(45)
    expect(s.config.agent.duration).toBe(45)
    expect('duration' in s.config).toBe(false)
  })

  it('新版設定的各端 duration 優先於舊欄位', () => {
    localStorage.setItem('sipress-config', JSON.stringify({ duration: 45, caller: { duration: 10 }, agent: { duration: 0 } }))
    const s = useTestStore()
    expect(s.config.caller.duration).toBe(10)
    expect(s.config.agent.duration).toBe(0)
  })

  it.each([
    [60, true], [0, false], [120, false], [300, false],
  ])('雙端啟動：民眾端 120s、座席端 %ss → 警告=%s', async (agentDuration, warned) => {
    vi.useFakeTimers()
    try {
      routeInvoke({
        start_test:   () => 'started',
        get_snapshot: () => snap({ agents_registered: 1, agents_total: 1 }),
      })
      const s = useTestStore()
      withAccount(s)
      s.config.caller.duration = 120
      s.config.agent.duration = agentDuration
      const done = s.startBoth()
      await vi.advanceTimersByTimeAsync(1500)
      await done
      expect(s.sessions.caller.status).toBe('running')
      expect(s.logs.some(l => l.level === 'warn' && l.msg.includes('座席會先結束'))).toBe(warned)
    } finally {
      vi.useRealTimers()
    }
  })
})

describe('端到端對照報告', () => {
  it('民眾端完成、座席端執行過才可匯出，送出 get_correlation_html 並存檔', async () => {
    vi.useFakeTimers()
    try {
      vi.mocked(save).mockResolvedValue('/tmp/e2e.html')
      const calls = routeInvoke({
        start_test:           () => 'started',
        get_snapshot:         a => modeOf(a) === 'caller' ? snap({ finished: true }) : snap({}),
        get_report:           () => baseReport,
        get_correlation_html: () => '<html>e2e</html>',
        save_text_file:       () => null,
      })
      const s = useTestStore()
      withAccount(s)
      expect(s.canExportE2e).toBe(false)
      await s.startTest('caller')
      expect(s.canExportE2e).toBe(false)          // 民眾端執行中、座席端沒跑過
      await vi.advanceTimersByTimeAsync(1000)
      await vi.waitFor(() => expect(s.sessions.caller.status).toBe('done'))
      expect(s.canExportE2e).toBe(false)          // 座席端沒跑過
      await s.startTest('agent')
      expect(s.canExportE2e).toBe(true)           // 座席端仍在執行也可匯出

      await s.exportE2eHtml()
      const req = calls.find(c => c.cmd === 'get_correlation_html')
      expect(req?.args).toMatchObject({ serverAddr: s.config.server })
      expect(req?.args?.timestamp).toMatch(/^\d{8}_\d{6}$/)
      const saved = calls.find(c => c.cmd === 'save_text_file')
      expect(saved?.args).toEqual({ path: '/tmp/e2e.html', contents: '<html>e2e</html>' })
      const saveCalls = vi.mocked(save).mock.calls
      expect(saveCalls[saveCalls.length - 1]?.[0]?.defaultPath).toMatch(/^sipress_e2e_\d{8}_\d{6}\.html$/)
      expect(s.logs.some(l => l.level === 'ok' && l.msg.includes('端到端對照報告 已匯出'))).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })

  it('每通不同主叫號碼：送出 caller_number_seq，CLI 預覽加 --from-seq', async () => {
    const calls = routeInvoke({ start_test: () => 'started' })
    const s = useTestStore()
    expect(s.cliCommand).not.toContain('--from-seq')
    s.config.caller.numberSeq = true
    expect(s.cliCommand).toContain('--from-seq')
    await s.startTest('caller')
    const sent = calls.find(c => c.cmd === 'start_test')?.args?.config as { caller_number_seq: boolean }
    expect(sent.caller_number_seq).toBe(true)
  })
})
