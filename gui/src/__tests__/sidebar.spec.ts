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

describe('通話聲音三選一', () => {
  const radios = () => [...root.querySelectorAll<HTMLButtonElement>('[role="radio"]')]
  const radio = (label: string) => radios().find(r => r.textContent === label)!

  it('預設不送語音，不顯示音檔、按鍵序列與 Codec', async () => {
    await nextTick()
    expect(radios().map(r => r.textContent)).toEqual(['不送語音', '循環播放音檔', '通話流程'])
    expect(radio('不送語音').getAttribute('aria-checked')).toBe('true')
    expect(root.querySelector('#sb-audio-file')).toBeNull()
    expect(root.querySelector('#sb-dtmf')).toBeNull()
    expect(root.querySelector('#sb-codec')).toBeNull()
  })

  it('選循環播放音檔只顯示音檔、按鍵序列與 Codec', async () => {
    const s = useTestStore()
    radio('循環播放音檔').click()
    await nextTick()
    expect(s.config.caller.audioMode).toBe('loop')
    expect(root.querySelector('#sb-audio-file')).not.toBeNull()
    expect(root.querySelector('#sb-dtmf')).not.toBeNull()
    expect(root.querySelector('#sb-codec')).not.toBeNull()
    expect(root.textContent).not.toContain('設定流程')
  })

  it('選通話流程時沒有步驟會直接開流程視窗；有步驟就只切換', async () => {
    const s = useTestStore()
    radio('通話流程').click()
    await nextTick()
    expect(s.config.caller.audioMode).toBe('flow')
    expect(document.querySelector('[role="dialog"]')).not.toBeNull()
    expect(root.querySelector('#sb-audio-file')).toBeNull()
    expect(root.textContent).toContain('設定流程')
  })

  it('方向鍵切換選項', async () => {
    const s = useTestStore()
    await nextTick()
    radio('不送語音').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }))
    await nextTick()
    expect(s.config.caller.audioMode).toBe('loop')
    expect(document.activeElement).toBe(radio('循環播放音檔'))
  })
})

describe('CLI 相關預設收起', () => {
  it('收起時不顯示指令預覽與 --參數標籤；展開後顯示並記住', async () => {
    await nextTick()
    expect(root.querySelector('.cli-box')).toBeNull()
    expect(root.querySelector('.sidebar')!.classList.contains('show-cli')).toBe(false)
    const toggle = root.querySelector<HTMLButtonElement>('.cli-toggle')!
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    toggle.click()
    await nextTick()
    expect(root.querySelector('.cli-box')).not.toBeNull()
    expect(root.querySelector('.sidebar')!.classList.contains('show-cli')).toBe(true)
    expect(localStorage.getItem('sipress-show-cli')).toBe('1')
  })
})
