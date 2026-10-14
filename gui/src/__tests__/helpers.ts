import { vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { createPinia, setActivePinia } from 'pinia'

type Handler = (args?: Record<string, unknown>) => unknown

/** 依 command 名稱分派 invoke；未列出的 command 一律丟錯，避免測試默默呼叫到預期外的後端 */
export function routeInvoke(handlers: Record<string, Handler>) {
  const calls: { cmd: string; args?: Record<string, unknown> }[] = []
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
    calls.push({ cmd, args: args as Record<string, unknown> | undefined })
    const h = handlers[cmd]
    if (!h) throw new Error(`unexpected invoke: ${cmd}`)
    return h(args as Record<string, unknown> | undefined)
  })
  return calls
}

/** 每個測試使用全新的 pinia + localStorage */
export function freshPinia() {
  localStorage.clear()
  vi.mocked(invoke).mockReset()
  setActivePinia(createPinia())
}
