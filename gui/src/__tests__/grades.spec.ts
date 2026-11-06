/// 指標分級：顏色之外都有文字標籤；門檻邊界與畫面原本的配色一致
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { getActivePinia } from 'pinia'
import { asrGrade, errGrade, regGrade, lossGrade, jitterGrade, useTestStore } from '../stores/testStore'
import MetricStrip from '../components/MetricStrip.vue'
import Sidebar from '../components/Sidebar.vue'
import { freshPinia } from './helpers'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn(), open: vi.fn() }))

describe('分級門檻', () => {
  it('ASR：85 / 70 為邊界', () => {
    expect([90, 85, 84.9, 70, 69.9].map(v => asrGrade(v).label)).toEqual(['良好', '良好', '注意', '注意', '偏低'])
  })
  it('錯誤率：5 / 20 為邊界', () => {
    expect([0, 5, 5.1, 20, 20.1].map(v => errGrade(v).label)).toEqual(['良好', '良好', '注意', '注意', '偏高'])
  })
  it('座席註冊：全部 / 部分 / 未註冊', () => {
    expect([regGrade(8, 8), regGrade(3, 8), regGrade(0, 8)].map(g => g.label)).toEqual(['全部', '部分', '未註冊'])
  })
  it('掉包率：0.5 / 1 為邊界，用語與 MOS 一致', () => {
    expect([0, 0.5, 0.51, 1, 1.01].map(v => lossGrade(v).label)).toEqual(['優', '優', '普通', '普通', '差'])
  })
  it('Jitter：30 / 60 為邊界（不含）', () => {
    expect([10, 29.9, 30, 59.9, 60].map(v => jitterGrade(v).label)).toEqual(['優', '優', '普通', '普通', '差'])
  })
  it('每個等級的顏色都對應同一組主題色', () => {
    for (const g of [asrGrade(90), errGrade(10), lossGrade(5)]) {
      expect({ good: 'var(--accent)', warn: 'var(--warn)', bad: 'var(--danger)' }[g.key]).toBe(g.color)
    }
  })
})

function mount(comp: object) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(comp).use(getActivePinia()!)
  app.mount(root)
  return { root, app }
}

describe('指標列顯示文字分級', () => {
  let m: { root: HTMLElement; app: App }
  beforeEach(() => { freshPinia(); m = mount(MetricStrip) })
  afterEach(() => { m.app.unmount(); m.root.remove() })

  it('待命時、以及執行中但還沒有任何通話時不分級；有通話後 ASR 與錯誤率附文字', async () => {
    await nextTick()
    expect(m.root.querySelectorAll('.grade')).toHaveLength(0)

    const s = useTestStore()
    s.sessions.caller.status = 'running'
    await nextTick()
    expect(m.root.querySelectorAll('.grade')).toHaveLength(0)   // ASR 0% 不是「偏低」

    s.sessions.caller.metrics.initiated = 100
    s.sessions.caller.metrics.asr = 72
    s.sessions.caller.metrics.errorRate = 30
    await nextTick()
    expect([...m.root.querySelectorAll('.grade')].map(e => e.textContent?.trim()))
      .toEqual(['· 注意', '· 偏高'])
  })
})

describe('滑桿範圍隨數字框延伸', () => {
  let m: { root: HTMLElement; app: App }
  beforeEach(() => { freshPinia(); m = mount(Sidebar) })
  afterEach(() => { m.app.unmount(); m.root.remove() })

  const cpsSlider = () => m.root.querySelector<HTMLInputElement>('input[type="range"][aria-label="呼叫頻率 CPS"]')!

  it('數值在預設範圍內維持原上限，超過時上限跟著數值', async () => {
    const s = useTestStore()
    s.config.caller.cps = 50
    await nextTick()
    expect(cpsSlider().max).toBe('200')

    s.config.caller.cps = 350
    await nextTick()
    expect(cpsSlider().max).toBe('350')
  })
})
