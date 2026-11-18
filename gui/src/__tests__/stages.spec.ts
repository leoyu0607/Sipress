/// 三段式畫面：設定 → 執行中 → 結果（階段切換、判定標準、結束提示、結果結論）
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createApp, nextTick, type App, type Component } from 'vue'
import { getActivePinia } from 'pinia'
import { useTestStore, DEFAULT_THRESHOLDS, type VerdictSummary } from '../stores/testStore'
import StageNav from '../components/StageNav.vue'
import ReadyPanel from '../components/ReadyPanel.vue'
import RunBar from '../components/RunBar.vue'
import ResultBanner from '../components/ResultBanner.vue'
import FinishToast from '../components/FinishToast.vue'
import { freshPinia, routeInvoke } from './helpers'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn(), open: vi.fn() }))

let app: App | null = null
let root: HTMLElement

beforeEach(() => { freshPinia() })
afterEach(() => {
  app?.unmount()
  app = null
  root?.remove()
})

function mount(c: Component) {
  root = document.createElement('div')
  document.body.appendChild(root)
  app = createApp(c).use(getActivePinia()!)
  app.mount(root)
}
const btn = (text: string) =>
  [...root.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent?.includes(text))!

/** 可以開始的民眾端設定 */
function validCaller(s: ReturnType<typeof useTestStore>) {
  s.config.server = '127.0.0.1:5060'
  s.config.caller.accessNumber = '4008001234'
  s.config.caller.audioMode = 'silence'
}

const failVerdict: VerdictSummary = {
  verdict: 'fail',
  checks: [
    { name: '接通率 ASR', scope: '民眾端', value: 90, unit: '%', limit: '≥ 95', pass: false, note: '' },
    { name: 'Jitter', scope: '', value: 8, unit: 'ms', limit: '≤ 30', pass: true, note: '' },
  ],
}

describe('階段切換（store）', () => {
  it('待命 → 設定；執行中 → 執行中；結束後停在執行中，按了才看結果', () => {
    const s = useTestStore()
    expect(s.stage).toBe('setup')

    s.goStage('result')                       // 還沒跑過，看不到結果
    expect(s.stage).toBe('setup')

    s.sessions.caller.status = 'running'
    expect(s.stage).toBe('running')
    s.goStage('setup')                        // 執行中也能回去看設定（鎖定）
    expect(s.stage).toBe('setup')
    s.goStage('running')
    expect(s.stage).toBe('running')

    s.sessions.caller.status = 'done'
    s.finishedUnseen.caller = true
    expect(s.stage).toBe('running')
    s.goStage('result')
    expect(s.stage).toBe('result')
    expect(s.finishedUnseen.caller).toBe(false)

    s.goStage('setup')
    expect(s.stage).toBe('setup')
  })

  it('兩端各自記住階段', () => {
    const s = useTestStore()
    s.sessions.caller.status = 'done'
    s.goStage('result', 'caller')
    s.config.mode = 'agent'
    expect(s.stage).toBe('setup')
    s.config.mode = 'caller'
    expect(s.stage).toBe('result')
  })
})

describe('判定門檻', () => {
  it('修改後存在本機，已完成的一端重新判定', async () => {
    const calls = routeInvoke({ get_verdict: () => failVerdict })
    const s = useTestStore()
    expect(s.thresholds).toEqual(DEFAULT_THRESHOLDS)
    s.sessions.caller.status = 'done'

    s.setThresholds({ ...s.thresholds, asr_pct: 85 })
    await vi.waitFor(() => expect(s.verdicts.caller).toEqual(failVerdict))
    expect(calls).toEqual([{ cmd: 'get_verdict', args: { mode: 'caller', thresholds: { ...DEFAULT_THRESHOLDS, asr_pct: 85 } } }])
    expect(JSON.parse(localStorage.getItem('sipress-report')!).thresholds.asr_pct).toBe(85)
  })
})

describe('StageNav', () => {
  it('還不能去的階段停用；目前階段標示 aria-current', async () => {
    const s = useTestStore()
    mount(StageNav)
    await nextTick()
    expect(btn('設定').getAttribute('aria-current')).toBe('step')
    expect(btn('執行中').disabled).toBe(true)
    expect(btn('結果').disabled).toBe(true)

    s.sessions.caller.status = 'done'
    s.finishedUnseen.caller = true
    await nextTick()
    expect(btn('執行中').getAttribute('aria-current')).toBe('step')
    expect(btn('結果').classList.contains('attention')).toBe(true)

    btn('結果').click()
    await nextTick()
    expect(s.stage).toBe('result')
    expect(btn('結果').getAttribute('aria-current')).toBe('step')
  })
})

describe('ReadyPanel', () => {
  it('列出開始前要修正的項目，修正後顯示可以開始', async () => {
    const s = useTestStore()
    s.config.server = ''
    mount(ReadyPanel)
    await nextTick()
    const items = [...root.querySelectorAll('.checks li')].map(li => li.textContent?.trim())
    expect(items).toEqual(s.validationErrors)
    expect(items).toContain('SIP server 不可空白')

    validCaller(s)
    await nextTick()
    expect(root.querySelector('.checks')).toBeNull()
    expect(root.textContent).toContain('設定完整，可以開始')
  })

  it('判定標準：改了就存；不合法的值還原；座席端不顯示接通率', async () => {
    const s = useTestStore()
    mount(ReadyPanel)
    await nextTick()
    const input = (label: string) =>
      [...root.querySelectorAll<HTMLLabelElement>('label.th')].find(l => l.textContent?.includes(label))?.querySelector('input')

    const asr = input('接通率')!
    asr.value = '90'
    asr.dispatchEvent(new Event('input'))
    asr.dispatchEvent(new Event('change'))
    expect(s.thresholds.asr_pct).toBe(90)

    const mos = input('MOS')!
    mos.value = '-1'
    mos.dispatchEvent(new Event('input'))
    mos.dispatchEvent(new Event('change'))
    await nextTick()
    expect(s.thresholds.mos).toBe(DEFAULT_THRESHOLDS.mos)
    expect(mos.value).toBe(String(DEFAULT_THRESHOLDS.mos))

    btn('恢復預設').click()
    expect(s.thresholds.asr_pct).toBe(DEFAULT_THRESHOLDS.asr_pct)

    s.config.mode = 'agent'
    await nextTick()
    expect(input('接通率')).toBeUndefined()
  })

  it('跑過之後顯示上次結果，可直接看完整結果', async () => {
    const s = useTestStore()
    mount(ReadyPanel)
    await nextTick()
    expect(root.querySelector('.card.last')).toBeNull()

    s.sessions.caller.status = 'done'
    s.verdicts.caller = failVerdict
    await nextTick()
    expect(root.querySelector('.card.last .verdict')?.textContent).toBe('未通過')
    btn('看完整結果').click()
    expect(s.stage).toBe('result')
  })

  it('執行中回來看設定時提示鎖定，可回到執行畫面', async () => {
    const s = useTestStore()
    s.sessions.caller.status = 'running'
    s.goStage('setup')
    mount(ReadyPanel)
    await nextTick()
    expect(root.querySelector('.running-note')).not.toBeNull()
    btn('回到執行畫面').click()
    expect(s.stage).toBe('running')
  })
})

describe('ReadyPanel：另一端執行中', () => {
  it('提示並可切換過去', async () => {
    const s = useTestStore()
    s.sessions.agent.status = 'running'
    mount(ReadyPanel)
    await nextTick()
    expect(root.querySelector('.running-note')?.textContent).toContain('座席端測試執行中')
    btn('切換到座席端').click()
    expect(s.config.mode).toBe('agent')
  })
})

describe('RunBar', () => {
  it('執行中顯示進度；結束後提供查看結果', async () => {
    const s = useTestStore()
    s.config.caller.duration = 300
    s.sessions.caller.status = 'running'
    s.sessions.caller.elapsedSec = 150
    mount(RunBar)
    await nextTick()
    expect(root.querySelector('.time')?.textContent).toBe('02:30 / 05:00')
    expect(root.querySelector<HTMLElement>('.fill')?.style.width).toBe('50%')

    s.sessions.caller.status = 'done'
    await nextTick()
    expect(root.textContent).toContain('民眾端測試已結束')
    btn('查看結果').click()
    expect(s.stage).toBe('result')
  })
})

describe('ResultBanner', () => {
  it('結論寫出未達標的項目與主要失敗原因；可再跑一次或回去調整', async () => {
    const calls = routeInvoke({ start_test: () => 'started', get_snapshot: () => null })
    vi.useFakeTimers()
    try {
      const s = useTestStore()
      validCaller(s)
      const r = s.sessions.caller
      r.status = 'done'
      r.metrics.initiated = 100
      r.metrics.succeeded = 90
      r.metrics.failed = 10
      r.metrics.asr = 90
      r.respCodes = { '200': 90, '486': 7, '408': 3 }
      s.verdicts.caller = failVerdict
      s.goStage('result')
      mount(ResultBanner)
      await nextTick()

      expect(root.querySelector('.tag')?.textContent).toBe('未通過')
      const h1 = root.querySelector('h1')!.textContent!
      expect(h1).toContain('發起 100 通，接通率 90.0%')
      expect(h1).toContain('未達標：接通率 ASR')
      expect(h1).toContain('失敗主要是 486 忙線（7 通）')
      const chips = [...root.querySelectorAll('.checks li')]
      expect(chips.map(c => c.className)).toEqual(['bad', 'ok'])

      btn('調整設定').click()
      expect(s.stage).toBe('setup')
      s.goStage('result')
      await nextTick()

      btn('再跑一次').click()
      await vi.waitFor(() => expect(calls.some(c => c.cmd === 'start_test')).toBe(true))
      expect(s.stage).toBe('running')
      s.stopAll()
    } finally {
      vi.useRealTimers()
    }
  })

  it('執行失敗時標示失敗，不顯示判定', async () => {
    const s = useTestStore()
    s.sessions.caller.status = 'error'
    s.goStage('result')
    mount(ResultBanner)
    await nextTick()
    expect(root.querySelector('.tag')?.textContent).toBe('執行失敗')
    expect(root.querySelector('h1')?.textContent).toContain('測試沒有正常結束')
  })
})

describe('FinishToast', () => {
  it('結束後提示，按「查看結果」切到那一端的結果；看過就不再提示', async () => {
    const s = useTestStore()
    mount(FinishToast)
    await nextTick()
    expect(root.querySelector('.toast')).toBeNull()

    s.sessions.agent.status = 'done'
    s.finishedUnseen.agent = true
    await nextTick()
    expect(root.querySelector('.toast')?.textContent).toContain('座席端測試完成')

    btn('查看結果').click()
    await nextTick()
    expect(s.config.mode).toBe('agent')
    expect(s.stage).toBe('result')
    expect(root.querySelector('.toast')).toBeNull()
  })

  it('可以關掉提示', async () => {
    const s = useTestStore()
    s.sessions.caller.status = 'done'
    s.finishedUnseen.caller = true
    mount(FinishToast)
    await nextTick()
    root.querySelector<HTMLButtonElement>('button[aria-label="關閉提示"]')!.click()
    await nextTick()
    expect(root.querySelector('.toast')).toBeNull()
    expect(s.stage).toBe('running')
  })
})
