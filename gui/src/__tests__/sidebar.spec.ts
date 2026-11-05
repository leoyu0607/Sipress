/// Sidebar 元件：執行中鎖定設定，但模式分頁（檢視切換）不可被鎖住
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { getActivePinia } from 'pinia'
import Sidebar from '../components/Sidebar.vue'
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
  app = createApp(Sidebar).use(getActivePinia()!)
  app.mount(root)
})
afterEach(() => {
  app?.unmount()
  root.remove()
})

const tabs = () => [...root.querySelectorAll<HTMLButtonElement>('button.mode-tab')]
const serverInput = () => root.querySelector<HTMLInputElement>('input[placeholder="host:port"]')!

describe('執行中鎖定', () => {
  it('檢視中的一端執行時設定被鎖定，但仍可切換分頁查看另一端', async () => {
    const s = useTestStore()
    s.sessions.caller.status = 'running'
    s.sessions.agent.status = 'running'
    await nextTick()

    expect(root.querySelector('.lock-banner')).not.toBeNull()
    expect(serverInput().closest('fieldset')?.disabled).toBe(true)
    expect(tabs()).toHaveLength(2)
    for (const t of tabs()) expect(t.disabled || !!t.closest('fieldset:disabled')).toBe(false)

    tabs()[1].click()
    await nextTick()
    expect(s.config.mode).toBe('agent')
    tabs()[0].click()
    await nextTick()
    expect(s.config.mode).toBe('caller')
  })

  it('沒有執行時設定可編輯', async () => {
    await nextTick()
    expect(root.querySelector('.lock-banner')).toBeNull()
    expect(serverInput().closest('fieldset')?.disabled).toBe(false)
  })
})

describe('無法開始的原因直接顯示', () => {
  it('有驗證錯誤時列出每一項，修正後消失', async () => {
    const s = useTestStore()
    s.config.server = ''
    await nextTick()
    const box = root.querySelector('.start-blockers')
    expect(box).not.toBeNull()
    const items = [...box!.querySelectorAll('li')].map(li => li.textContent)
    expect(items).toEqual(s.validationErrors)
    expect(items).toContain('SIP server 不可空白')

    s.config.server = '127.0.0.1:5060'
    s.config.caller.accessNumber = '4008001234'
    s.config.caller.enableAudio = false
    await nextTick()
    expect(s.validationErrors).toEqual([])
    expect(root.querySelector('.start-blockers')).toBeNull()
  })

  it('執行中改顯示鎖定提示，不顯示驗證清單', async () => {
    const s = useTestStore()
    s.config.server = ''
    s.sessions.caller.status = 'running'
    await nextTick()
    expect(root.querySelector('.lock-banner')).not.toBeNull()
    expect(root.querySelector('.start-blockers')).toBeNull()
  })
})

describe('無障礙語意', () => {
  it('每個 label[for] 都指到存在的輸入框', async () => {
    const s = useTestStore()
    for (const mode of ['caller', 'agent'] as const) {
      s.config.mode = mode
      await nextTick()
      const labels = [...root.querySelectorAll<HTMLLabelElement>('label[for]')]
      expect(labels.length).toBeGreaterThan(5)
      for (const l of labels) {
        const target = root.querySelector(`#${l.htmlFor}`)
        expect(target, `label for=${l.htmlFor}`).not.toBeNull()
        expect(['INPUT', 'SELECT', 'TEXTAREA']).toContain(target!.tagName)
      }
    }
  })

  it('SIP 伺服器標籤綁定輸入框，格式錯誤時標示 aria-invalid', async () => {
    const s = useTestStore()
    s.config.server = 'bad'
    await nextTick()
    const label = root.querySelector<HTMLLabelElement>('label[for="sb-server"]')!
    expect(root.querySelector(`#${label.htmlFor}`)).toBe(serverInput())
    expect(serverInput().getAttribute('aria-invalid')).toBe('true')
  })

  it('區塊標題可用鍵盤收合，並更新 aria-expanded', async () => {
    const hdr = root.querySelector<HTMLElement>('.section-hdr')!
    expect(hdr.getAttribute('role')).toBe('button')
    expect(hdr.tabIndex).toBe(0)
    expect(hdr.getAttribute('aria-expanded')).toBe('true')
    hdr.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    await nextTick()
    expect(hdr.getAttribute('aria-expanded')).toBe('false')
    hdr.dispatchEvent(new KeyboardEvent('keydown', { key: ' ' }))
    await nextTick()
    expect(hdr.getAttribute('aria-expanded')).toBe('true')
  })

  it('模式切換是 tablist，aria-selected 跟著目前模式', async () => {
    const s = useTestStore()
    expect(root.querySelector('.mode-tabs')?.getAttribute('role')).toBe('tablist')
    expect(tabs().map(t => t.getAttribute('aria-selected'))).toEqual(['true', 'false'])
    s.config.mode = 'agent'
    await nextTick()
    expect(tabs().map(t => t.getAttribute('aria-selected'))).toEqual(['false', 'true'])
  })
})
