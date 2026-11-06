/// 即時圖表的暫停 / 文字摘要 / 鍵盤查看，以及側欄寬度調整
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'
import { getActivePinia } from 'pinia'
import ChartPanel from '../components/ChartPanel.vue'
import SidebarResizer, { loadSidebarWidth, sidebarMaxFor, SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN } from '../components/SidebarResizer.vue'
import { useTestStore } from '../stores/testStore'
import { freshPinia } from './helpers'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn(), open: vi.fn() }))

// happy-dom 沒有 canvas 繪圖：所有 2D context 方法都當成 no-op
const noopCtx = new Proxy({}, {
  get: (_t, prop) => prop === 'measureText' ? () => ({ width: 10 })
    : prop === 'createLinearGradient' ? () => ({ addColorStop() {} })
    : () => {},
  set: () => true,
})
HTMLCanvasElement.prototype.getContext = (() => noopCtx) as unknown as typeof HTMLCanvasElement.prototype.getContext

const key = (el: Element, k: string) => el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true }))

function mount(comp: object) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(comp).use(getActivePinia()!)
  app.provide('isDark', ref(true))
  app.mount(root)
  return { root, app }
}

describe('即時圖表', () => {
  let m: { root: HTMLElement; app: App }
  beforeEach(() => { freshPinia(); m = mount(ChartPanel) })
  afterEach(() => { m.app.unmount(); m.root.remove() })

  const pauseBtn = () => m.root.querySelector<HTMLButtonElement>('.pause-btn')!
  const summary = () => m.root.querySelector('.chart-summary')!.textContent!.trim()
  const canvas = () => m.root.querySelector('canvas')!

  /** 民眾端執行中，CPS 序列 = values */
  async function run(values: number[]) {
    const s = useTestStore()
    s.sessions.caller.status = 'running'
    s.sessions.caller.series.cps.push(...values)
    s.sessions.caller.seriesTick++
    await nextTick()
    return s
  }

  it('沒有資料時不能暫停、沒有摘要', async () => {
    await nextTick()
    expect(pauseBtn().disabled).toBe(true)
    expect(summary()).toBe('')
    expect(canvas().getAttribute('aria-label')).toContain('尚無資料')
  })

  it('文字摘要列出目前 / 平均 / 最高 / 最低與時間範圍，canvas 的 aria-label 相同', async () => {
    await run([2, 4, 6, 8])
    expect(summary()).toBe('目前 8.0 · 平均 5.0 · 最高 8.0 · 最低 2.0（0–3s）')
    expect(canvas().getAttribute('role')).toBe('img')
    expect(canvas().getAttribute('aria-label')).toBe(`CPS — 每秒發起通數：${summary()}`)
  })

  it('暫停時畫面凍結（背景照常收集），繼續後回到最新資料', async () => {
    const s = await run([2, 4])
    pauseBtn().click()
    await nextTick()
    expect(pauseBtn().getAttribute('aria-pressed')).toBe('true')
    expect(summary()).toContain('已暫停')

    s.sessions.caller.series.cps.push(100)
    s.sessions.caller.seriesTick++
    await nextTick()
    expect(summary()).toMatch(/^目前 4\.0 .*最高 4\.0/)      // 仍是暫停當下的資料

    pauseBtn().click()
    await nextTick()
    expect(summary()).toMatch(/^目前 100\.0 /)
  })

  it('暫停中切換到另一端或開始新的測試，自動恢復', async () => {
    const s = await run([1, 2])
    pauseBtn().click()
    await nextTick()
    s.config.mode = 'agent'
    await nextTick()
    expect(pauseBtn().getAttribute('aria-pressed')).toBe('false')

    s.config.mode = 'caller'
    s.sessions.caller.status = 'done'
    await nextTick()
    pauseBtn().click()
    await nextTick()
    s.sessions.caller.status = 'running'
    await nextTick()
    expect(pauseBtn().getAttribute('aria-pressed')).toBe('false')
  })

  it('鍵盤：← → / Home / End 逐點查看並由 live region 讀出，Esc 離開', async () => {
    await run([3, 5, 7])
    const live = () => m.root.querySelector('[aria-live="polite"]')!.textContent
    key(canvas(), 'ArrowLeft')
    await nextTick()
    expect(live()).toBe('1s：5.0')
    key(canvas(), 'Home')
    await nextTick()
    expect(live()).toBe('0s：3.0')
    key(canvas(), 'ArrowLeft')                     // 已在頭，不會超出
    await nextTick()
    expect(live()).toBe('0s：3.0')
    key(canvas(), 'End')
    await nextTick()
    expect(live()).toBe('2s：7.0')
    key(canvas(), 'Escape')
    await nextTick()
    expect(live()).toBe('')
  })
})

describe('側欄寬度調整', () => {
  let m: { root: HTMLElement; app: App }
  const width = ref(SIDEBAR_DEFAULT)
  beforeEach(() => {
    freshPinia()
    width.value = SIDEBAR_DEFAULT
    m = mount(defineComponent({
      setup: () => () => h(SidebarResizer, { modelValue: width.value, 'onUpdate:modelValue': (v: number) => { width.value = v } }),
    }))
  })
  afterEach(() => { m.app.unmount(); m.root.remove() })

  const handle = () => m.root.querySelector<HTMLElement>('[role="separator"]')!

  it('是可聚焦的 separator，帶目前與上下限', async () => {
    await nextTick()
    expect(handle().tabIndex).toBe(0)
    expect(handle().getAttribute('aria-valuenow')).toBe(String(SIDEBAR_DEFAULT))
    expect(handle().getAttribute('aria-valuemin')).toBe(String(SIDEBAR_MIN))
    expect(handle().getAttribute('aria-valuemax')).toBe(String(SIDEBAR_MAX))
  })

  it('鍵盤調整並限制在上下限內，每一步都記在本機', async () => {
    // 每按一次鍵等重繪（實際使用時按鍵之間一定有重繪）
    const press = async (k: string) => { key(handle(), k); await nextTick() }
    await press('ArrowRight')
    expect(width.value).toBe(SIDEBAR_DEFAULT + 16)
    expect(loadSidebarWidth()).toBe(SIDEBAR_DEFAULT + 16)   // 存的是新值，不是上一次的
    await press('End')
    await press('ArrowRight')
    expect(width.value).toBe(SIDEBAR_MAX)
    await press('Home')
    await press('ArrowLeft')
    expect(width.value).toBe(SIDEBAR_MIN)
    expect(loadSidebarWidth()).toBe(SIDEBAR_MIN)
  })

  it('滑鼠拖曳改變寬度，雙擊恢復預設', async () => {
    handle().dispatchEvent(new MouseEvent('mousedown', { clientX: 300, bubbles: true }))
    window.dispatchEvent(new MouseEvent('mousemove', { clientX: 380 }))
    window.dispatchEvent(new MouseEvent('mouseup'))
    expect(width.value).toBe(SIDEBAR_DEFAULT + 80)
    expect(loadSidebarWidth()).toBe(SIDEBAR_DEFAULT + 80)

    await nextTick()
    handle().dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))
    expect(width.value).toBe(SIDEBAR_DEFAULT)
    expect(loadSidebarWidth()).toBe(SIDEBAR_DEFAULT)
  })

  it('上限依視窗寬度：主內容區至少保留 620px', () => {
    expect(sidebarMaxFor(1280)).toBe(SIDEBAR_MAX)
    expect(sidebarMaxFor(960)).toBe(340)
    expect(sidebarMaxFor(700)).toBe(SIDEBAR_MIN)          // 再窄也不小於最小寬度
  })

  it('本機存的值超出範圍或不是數字時，讀回會修正', () => {
    localStorage.setItem('sipress-sidebar-width', '9999')
    expect(loadSidebarWidth()).toBe(SIDEBAR_MAX)
    localStorage.setItem('sipress-sidebar-width', 'abc')
    expect(loadSidebarWidth()).toBe(SIDEBAR_DEFAULT)
  })
})

describe('側欄寬度：視窗較窄時的上限', () => {
  let m: { root: HTMLElement; app: App }
  const width = ref(400)                                   // 之前在大視窗設的寬度
  beforeEach(() => {
    freshPinia()
    width.value = 400
    m = mount(defineComponent({
      setup: () => () => h(SidebarResizer, { modelValue: width.value, max: 340, 'onUpdate:modelValue': (v: number) => { width.value = v } }),
    }))
  })
  afterEach(() => { m.app.unmount(); m.root.remove() })

  it('顯示與調整都以目前上限為準，不會從超出上限的值跳動', async () => {
    const handle = m.root.querySelector<HTMLElement>('[role="separator"]')!
    expect(handle.getAttribute('aria-valuenow')).toBe('340')
    expect(handle.getAttribute('aria-valuemax')).toBe('340')
    key(handle, 'ArrowLeft')
    await nextTick()
    expect(width.value).toBe(340 - 16)                    // 從畫面上的 340 減，不是從 400
    key(handle, 'End')
    await nextTick()
    expect(width.value).toBe(340)
  })
})
