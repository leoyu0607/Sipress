/// 標題列「報告」選單：整合報告 + 5 種匯出檔
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { getActivePinia } from 'pinia'
import ExportMenu from '../components/ExportMenu.vue'
import { useTestStore } from '../stores/testStore'
import { freshPinia } from './helpers'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn(), open: vi.fn() }))

let app: App | null = null
let root: HTMLElement

beforeEach(() => {
  freshPinia()
  root = document.createElement('div')
  document.body.appendChild(root)
  app = createApp(ExportMenu).use(getActivePinia()!)
  app.mount(root)
})
afterEach(() => {
  app?.unmount()
  root.remove()
})

const trigger = () => root.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]')!
const menu = () => root.querySelector<HTMLElement>('[role="menu"]')
const items = () => [...root.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
const key = (el: Element, k: string) => el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true }))

/** 有每秒資料、但測試尚未完成：JSON / CSV 可用，HTML / 數據 / 對照不可用 */
async function withSeries() {
  const s = useTestStore()
  s.series.cps.push(1)
  await nextTick()
  return s
}

const fileItems = () => items().slice(1)

describe('報告選單', () => {
  it('沒有測試資料時仍可開啟（整合報告可用），匯出檔全部停用並說明', async () => {
    await nextTick()
    expect(trigger().disabled).toBe(false)
    trigger().click()
    await nextTick()
    expect(items()[0].textContent).toContain('整合報告')
    expect(items()[0].disabled).toBe(false)
    expect(fileItems().every(b => b.disabled)).toBe(true)
    expect(root.textContent).toContain('尚無測試資料')
  })

  it('整合報告在第一項，下面列出全部 5 種匯出檔，不能用的直接寫出原因', async () => {
    await withSeries()
    trigger().click()
    await nextTick()
    expect(menu()).not.toBeNull()
    expect(trigger().getAttribute('aria-expanded')).toBe('true')
    const labels = items().map(b => b.querySelector('.item-label')!.textContent)
    expect(labels).toEqual(['整合報告…', 'JSON 報告', 'CSV 時序', 'HTML 報告', '壓測數據', '端到端對照'])
    expect(fileItems().map(b => b.disabled)).toEqual([false, false, true, true, true])
    expect(fileItems()[2].querySelector('.item-desc')!.textContent).toBe('測試完成後可匯出')
  })

  it('選一項會執行對應的匯出並關閉選單', async () => {
    const s = await withSeries()
    const spy = vi.fn()
    s.exportCsv = spy
    trigger().click()
    await nextTick()
    fileItems()[1].click()
    await nextTick()
    expect(spy).toHaveBeenCalledOnce()
    expect(menu()).toBeNull()
  })

  it('選整合報告會關閉選單並開啟報告視窗', async () => {
    trigger().click()
    await nextTick()
    items()[0].click()
    await nextTick()
    expect(menu()).toBeNull()
  })

  it('鍵盤：開啟時焦點在第一項，↓ 只在可用項目間循環，Esc 關閉並回到按鈕', async () => {
    await withSeries()
    trigger().click()
    await nextTick()
    await nextTick()
    expect(document.activeElement).toBe(items()[0])
    key(document.activeElement!, 'ArrowDown')
    expect(document.activeElement).toBe(items()[1])
    key(document.activeElement!, 'ArrowDown')
    expect(document.activeElement).toBe(items()[2])
    key(document.activeElement!, 'ArrowDown')          // 後面三項停用 → 回到第一項
    expect(document.activeElement).toBe(items()[0])
    key(document.activeElement!, 'End')
    expect(document.activeElement).toBe(items()[2])
    key(document.activeElement!, 'Escape')
    await nextTick()
    expect(menu()).toBeNull()
    expect(document.activeElement).toBe(trigger())
  })

  it('在按鈕上按 ↓ 也會開啟選單', async () => {
    key(trigger(), 'ArrowDown')
    await nextTick()
    expect(menu()).not.toBeNull()
  })

  it('點選單外面會關閉', async () => {
    trigger().click()
    await nextTick()
    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }))
    await nextTick()
    expect(menu()).toBeNull()
  })
})
