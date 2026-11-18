/// 標題列開始按鈕：一顆主按鈕（目前分頁那一端）+ ▾ 選單（兩端一起跑、全部停止）
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { getActivePinia } from 'pinia'
import StartButton from '../components/StartButton.vue'
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
  app = createApp(StartButton).use(getActivePinia()!)
  app.mount(root)
})
afterEach(() => {
  app?.unmount()
  root.remove()
})

const main  = () => root.querySelector<HTMLButtonElement>('.start-btn')!
const caret = () => root.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]')!
const item  = (text: string) =>
  [...root.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(b => b.textContent?.includes(text))!

/** 兩端都能開始的設定 */
function validBoth(s: ReturnType<typeof useTestStore>) {
  s.config.server = '127.0.0.1:5060'
  s.config.agent.accounts = [{ id: 'a', extension: '1001', username: '1001', password: 'p', domain: '', status: 'idle' }]
}

describe('開始按鈕', () => {
  it('待命時主按鈕開始目前分頁那一端', async () => {
    const s = useTestStore()
    validBoth(s)
    const start = vi.fn()
    s.startTest = start
    await nextTick()
    expect(main().textContent).toContain('開始測試')
    main().click()
    expect(start).toHaveBeenCalledOnce()
  })

  it('有驗證錯誤時主按鈕停用，提示寫出原因', async () => {
    const s = useTestStore()
    s.config.server = ''
    await nextTick()
    expect(main().disabled).toBe(true)
    expect(main().parentElement!.title).toContain('SIP server 不可空白')
  })

  it('目前這端執行中 → 停止測試；兩端都在跑 → 全部停止', async () => {
    const s = useTestStore()
    const stop = vi.fn(), stopAll = vi.fn()
    s.stopTest = stop
    s.stopAll = stopAll
    s.sessions.caller.status = 'running'
    await nextTick()
    expect(main().textContent).toContain('停止測試')
    main().click()
    expect(stop).toHaveBeenCalledOnce()

    s.sessions.agent.status = 'running'
    await nextTick()
    expect(main().textContent).toContain('全部停止')
    main().click()
    expect(stopAll).toHaveBeenCalledOnce()
  })

  it('▾ 選單：兩端一起跑，執行後關閉選單', async () => {
    const s = useTestStore()
    validBoth(s)
    const both = vi.fn()
    s.startBoth = both
    caret().click()
    await nextTick()
    expect(item('全部停止').disabled).toBe(true)
    item('兩端一起跑').click()
    await nextTick()
    expect(both).toHaveBeenCalledOnce()
    expect(root.querySelector('[role="menu"]')).toBeNull()
  })

  it('▾ 選單：兩端設定有問題時停用「兩端一起跑」並列出原因', async () => {
    const s = useTestStore()
    s.config.server = '127.0.0.1:5060'          // 沒有座席帳號
    caret().click()
    await nextTick()
    const b = item('兩端一起跑')
    expect(b.disabled).toBe(true)
    expect(b.textContent).toContain('座席模式需要至少一個帳號')
  })

  it('▾ 選單：有一端在跑時可全部停止，不能再兩端一起跑', async () => {
    const s = useTestStore()
    validBoth(s)
    const stopAll = vi.fn()
    s.stopAll = stopAll
    s.sessions.agent.status = 'running'
    caret().click()
    await nextTick()
    expect(item('兩端一起跑').disabled).toBe(true)
    item('全部停止').click()
    expect(stopAll).toHaveBeenCalledOnce()
  })
})
