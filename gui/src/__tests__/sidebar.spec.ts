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
