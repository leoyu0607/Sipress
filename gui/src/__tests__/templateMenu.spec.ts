/// 標題列「範本」選單與範本視窗
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { getActivePinia } from 'pinia'
import TemplateMenu from '../components/TemplateMenu.vue'
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
  app = createApp(TemplateMenu).use(getActivePinia()!)
  app.mount(root)
})
afterEach(() => {
  app?.unmount()
  root.remove()
  document.body.innerHTML = ''
})

const trigger = () => root.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]')!
const items = () => [...root.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
const item = (text: string) => items().find(b => b.textContent?.includes(text))
async function openMenu() {
  trigger().click()
  await nextTick()
  await nextTick()
}

describe('範本選單', () => {
  it('沒有範本時說明怎麼建立，並列出內建情境', async () => {
    await openMenu()
    expect(root.textContent).toContain('還沒有範本')
    for (const name of ['試打幾通', '按鍵 IVR', '語音 IVR（TTS）', '長時間耐久'])
      expect(item(name), name).toBeTruthy()
    expect(item('把目前設定存成範本')).toBeTruthy()
    expect(document.activeElement).toBe(items()[0])
  })

  it('套用內建情境後關閉選單，可從選單復原', async () => {
    const s = useTestStore()
    s.config.caller.concurrency = 42
    await openMenu()
    item('試打幾通')!.click()
    await nextTick()
    expect(root.querySelector('[role="menu"]')).toBeNull()
    expect(s.config.caller.concurrency).toBe(1)

    await openMenu()
    item('復原')!.click()
    await nextTick()
    expect(s.config.caller.concurrency).toBe(42)
  })

  it('按鈕顯示使用中的範本，改設定後標示已修改，可直接更新', async () => {
    const s = useTestStore()
    s.saveAsTemplate('月測', '')
    await nextTick()
    expect(trigger().textContent).toContain('月測')
    expect(trigger().textContent).not.toContain('已修改')

    s.config.caller.cps = 9
    await nextTick()
    expect(trigger().textContent).toContain('已修改')

    await openMenu()
    item('更新範本「月測」')!.click()
    await nextTick()
    expect(trigger().textContent).not.toContain('已修改')
  })

  it('測試執行中：套用停用並說明原因，存成範本仍可用', async () => {
    const s = useTestStore()
    s.saveAsTemplate('月測', '')
    s.sessions.caller.status = 'running'
    await openMenu()
    expect(root.textContent).toContain('測試執行中，停止後才能套用範本')
    expect(item('月測')!.disabled).toBe(true)
    expect(item('試打幾通')!.disabled).toBe(true)
    expect(item('把目前設定存成範本')!.disabled).toBe(false)
  })

  it('存成範本視窗：同名提示覆蓋，儲存後成為使用中', async () => {
    const s = useTestStore()
    s.saveAsTemplate('月測', '')
    await openMenu()
    item('把目前設定存成範本')!.click()
    await nextTick()
    await nextTick()
    const dlg = document.querySelector<HTMLElement>('[role="dialog"]')!
    expect(dlg.textContent).toContain('已有同名範本，儲存會覆蓋它')   // 預設帶入使用中範本的名稱
    const name = dlg.querySelector<HTMLInputElement>('input[type="text"]')!
    name.value = '季測'
    name.dispatchEvent(new Event('input'))
    await nextTick()
    expect(dlg.textContent).not.toContain('已有同名範本')
    ;[...dlg.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent?.trim() === '儲存')!.click()
    await nextTick()
    expect(s.templates.map(t => t.name)).toEqual(['月測', '季測'])
    expect(s.activeTemplate?.name).toBe('季測')
  })

  it('管理視窗：刪除需要按兩次確認', async () => {
    const s = useTestStore()
    s.saveAsTemplate('月測', '')
    await openMenu()
    item('管理範本')!.click()
    await nextTick()
    await nextTick()
    const dlg = document.querySelector<HTMLElement>('[role="dialog"]')!
    const del = () => [...dlg.querySelectorAll<HTMLButtonElement>('button')].find(b => /刪除/.test(b.textContent ?? ''))!
    del().click()
    await nextTick()
    expect(del().textContent).toContain('確定刪除？')
    expect(s.templates).toHaveLength(1)
    del().click()
    await nextTick()
    expect(s.templates).toHaveLength(0)
  })
})
