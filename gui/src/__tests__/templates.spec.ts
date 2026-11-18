import { describe, it, expect, vi, beforeEach } from 'vitest'
import { nextTick } from 'vue'
import { setActivePinia, createPinia } from 'pinia'
import { save } from '@tauri-apps/plugin-dialog'
import {
  useTestStore, builtinTemplates, parseTemplateFile, loadReportSettings, saveReportSettings,
  DEFAULT_THRESHOLDS, TEMPLATE_FILE_FORMAT, mergeConfig, defaultConfig, newCallerStep,
} from '../stores/testStore'

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v))
import { routeInvoke, freshPinia } from './helpers'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn() }))

beforeEach(freshPinia)

const last = <T>(xs: T[]): T => xs[xs.length - 1]

/** 有密碼的設定：撥號認證、座席帳號、座席密碼、TTS 金鑰 */
function withSecrets(s: ReturnType<typeof useTestStore>) {
  s.config.server = '10.0.0.1:5060'
  s.config.caller.calleeFixed = '1999'
  s.config.caller.authPass = 'auth-secret'
  s.config.caller.flow.tts.apiKey = 'tts-secret'
  s.config.agent.flow.agentPassword = 'agent-pwd-secret'
  s.config.agent.accounts = [
    { id: 'a', extension: '1001', username: '1001', password: 'acc-secret', domain: '', status: 'registered' },
  ]
}

describe('範本', () => {
  it('存成範本後為「使用中」；改設定顯示已修改；更新後恢復', async () => {
    const s = useTestStore()
    withSecrets(s)
    s.saveAsTemplate('  月測  ', '每月第一週')
    expect(s.templates).toHaveLength(1)
    expect(s.templates[0]).toMatchObject({ name: '月測', description: '每月第一週', kind: 'full' })
    expect(s.templates[0].config.agent?.accounts?.[0].status).toBe('idle')
    expect(s.activeTemplate?.name).toBe('月測')
    expect(s.templateModified).toBe(false)

    // 註冊狀態變化不算修改
    s.config.agent.accounts[0].status = 'failed'
    expect(s.templateModified).toBe(false)

    s.config.caller.concurrency = 77
    expect(s.templateModified).toBe(true)
    s.updateActiveTemplate()
    expect(s.templateModified).toBe(false)
    expect(s.templates).toHaveLength(1)
    expect(s.templates[0].config.caller?.concurrency).toBe(77)
  })

  it('同名儲存會覆蓋，不會重複', () => {
    const s = useTestStore()
    s.saveAsTemplate('A', '')
    const id = s.templates[0].id
    s.config.caller.cps = 5
    s.saveAsTemplate('A', '新說明')
    expect(s.templates).toHaveLength(1)
    expect(s.templates[0]).toMatchObject({ id, description: '新說明' })
    expect(s.templates[0].config.caller?.cps).toBe(5)
  })

  it('套用完整範本會取代所有設定與判定門檻，可以復原', () => {
    const s = useTestStore()
    s.config.server = '10.0.0.1:5060'
    s.config.caller.concurrency = 5
    saveReportSettings({ ...DEFAULT_THRESHOLDS, mos: 3.6 }, '備註')
    s.saveAsTemplate('A', '')

    s.config.server = '10.9.9.9:5060'
    s.config.caller.concurrency = 50
    saveReportSettings({ ...DEFAULT_THRESHOLDS, mos: 4.2 }, '備註')
    s.saveAsTemplate('B', '')

    expect(s.applyTemplate(s.templates[0])).toBeNull()
    expect(s.config.server).toBe('10.0.0.1:5060')
    expect(s.config.caller.concurrency).toBe(5)
    expect(s.activeTemplate?.name).toBe('A')
    expect(loadReportSettings()).toEqual({ thresholds: { ...DEFAULT_THRESHOLDS, mos: 3.6 }, notes: '備註' })

    s.undoApplyTemplate()
    expect(s.config.server).toBe('10.9.9.9:5060')
    expect(s.config.caller.concurrency).toBe(50)
    expect(s.activeTemplate?.name).toBe('B')
    expect(loadReportSettings().thresholds.mos).toBe(4.2)
    expect(s.templateUndo).toBeNull()
  })

  it('內建情境保留伺服器、號碼與帳號，只改情境欄位', () => {
    const s = useTestStore()
    withSecrets(s)
    s.saveAsTemplate('A', '')
    const ivr = builtinTemplates().find(t => t.id === 'builtin-dtmf-ivr')!
    s.applyTemplate(ivr)
    expect(s.config.server).toBe('10.0.0.1:5060')
    expect(s.config.caller.calleeFixed).toBe('1999')
    expect(s.config.caller.authPass).toBe('auth-secret')
    expect(s.config.caller.flow.tts.apiKey).toBe('tts-secret')
    expect(s.config.agent.accounts).toHaveLength(1)
    expect(s.config.caller.flow.steps.map(st => st.type)).toEqual(['wait', 'dtmf', 'wait', 'dtmf', 'wait'])
    expect(s.config.mode).toBe('caller')
    expect(s.activeTemplate).toBeNull()
    expect(s.validationErrors).toEqual([])
  })

  it('每個內建情境都能通過開始前的檢查（語音 IVR 只缺區域）', () => {
    for (const t of builtinTemplates()) {
      freshPinia()
      const s = useTestStore()
      s.applyTemplate(t)
      const errs = s.validationErrors
      if (t.id === 'builtin-voice-ivr') expect(errs).toEqual(['Azure TTS 需要填寫區域（例如 eastasia）'])
      else expect(errs, t.name).toEqual([])
    }
  })

  it('測試執行中不能套用範本', () => {
    const s = useTestStore()
    s.saveAsTemplate('A', '')
    s.sessions.caller.status = 'running'
    expect(s.applyTemplate(s.templates[0])).toBe('測試執行中，請先停止再套用範本')
  })

  it('刪除使用中的範本後不再顯示使用中', () => {
    const s = useTestStore()
    s.saveAsTemplate('A', '')
    s.deleteTemplate(s.templates[0].id)
    expect(s.templates).toEqual([])
    expect(s.activeTemplate).toBeNull()
  })

  it('範本與使用中的範本在重新開啟後保留', async () => {
    const s = useTestStore()
    s.saveAsTemplate('A', 'x')
    await nextTick()
    setActivePinia(createPinia())
    const again = useTestStore()
    expect(again.templates.map(t => t.name)).toEqual(['A'])
    expect(again.activeTemplate?.name).toBe('A')
  })

  it('匯出預設不含密碼與金鑰；勾選才包含', async () => {
    vi.mocked(save).mockResolvedValue('/tmp/t.json')
    const calls = routeInvoke({ save_text_file: () => null })
    const s = useTestStore()
    withSecrets(s)
    s.saveAsTemplate('嘉義 1999/月測', '說明')
    const id = s.templates[0].id

    await s.exportTemplates([id], false)
    const file = JSON.parse(last(calls).args!.contents as string)
    expect(file.format).toBe(TEMPLATE_FILE_FORMAT)
    expect(file.templates[0]).toMatchObject({ name: '嘉義 1999/月測', description: '說明' })
    expect(JSON.stringify(file)).not.toMatch(/secret/)
    expect(file.templates[0].config.server).toBe('10.0.0.1:5060')
    expect(last(vi.mocked(save).mock.calls)?.[0]?.defaultPath).toBe('sipress_範本_嘉義_1999_月測.json')

    await s.exportTemplates([id], true)
    const withKeys = last(calls).args!.contents as string
    for (const secret of ['auth-secret', 'tts-secret', 'agent-pwd-secret', 'acc-secret'])
      expect(withKeys).toContain(secret)
  })

  it('匯入匯出檔：同名加編號、不覆蓋；不含密碼的範本匯入後填上密碼不算修改', async () => {
    vi.mocked(save).mockResolvedValue('/tmp/t.json')
    const calls = routeInvoke({ save_text_file: () => null })
    const s = useTestStore()
    withSecrets(s)
    s.saveAsTemplate('月測', '')
    await s.exportTemplates([s.templates[0].id], false)
    const text = last(calls).args!.contents as string

    expect(s.importTemplates(text)).toBe(1)
    expect(s.templates.map(t => t.name)).toEqual(['月測', '月測 (2)'])
    expect(s.templates[1].id).not.toBe(s.templates[0].id)

    s.applyTemplate(s.templates[1])
    expect(s.config.caller.authPass).toBe('')
    s.config.caller.authPass = 'typed-in'
    expect(s.templateModified).toBe(false)
  })

  it('匯入錯誤的檔案給出可讀的訊息', () => {
    expect(() => parseTemplateFile('not json')).toThrow('檔案不是有效的 JSON')
    expect(() => parseTemplateFile('{"format":"other"}')).toThrow('檔案裡沒有範本')
    // 單一範本物件（不是匯出檔格式）也接受
    const one = parseTemplateFile(JSON.stringify({ name: 'X', config: { caller: { cps: 3 } } }))
    expect(one).toHaveLength(1)
    expect(one[0]).toMatchObject({ name: 'X', kind: 'full', description: '' })
  })

  it.each([
    [{ enableAudio: true, audioFile: 'a.wav' }, 'loop'],
    [{ enableAudio: false }, 'silence'],
    [{ enableAudio: true, flow: { steps: [newCallerStep('wait', { secs: 1 })] } }, 'flow'],   // 舊版流程優先
    [{ enableAudio: false, flow: { steps: [] } }, 'silence'],
    [{ audioMode: 'loop', enableAudio: false }, 'loop'],
  ] as const)('舊版播放音檔設定 %j 轉成通話聲音 %s', (caller, mode) => {
    const cfg = mergeConfig(defaultConfig(), { caller: clone(caller) as never })
    expect(cfg.caller.audioMode).toBe(mode)
    expect('enableAudio' in cfg.caller).toBe(false)
  })

  it('內建情境沒提到通話聲音時保留目前的選擇', () => {
    const s = useTestStore()
    s.config.caller.audioMode = 'loop'
    s.applyTemplate(builtinTemplates().find(t => t.id === 'builtin-soak')!)
    expect(s.config.caller.audioMode).toBe('loop')
    s.applyTemplate(builtinTemplates().find(t => t.id === 'builtin-dtmf-ivr')!)
    expect(s.config.caller.audioMode).toBe('flow')
  })

  it('舊版本存的範本缺少新欄位時，套用後補上預設值', () => {
    localStorage.setItem('sipress-templates', JSON.stringify([
      { id: 'old', name: '舊', description: '', kind: 'full', config: { server: '1.2.3.4:5060', caller: { cps: 3 } }, updatedAt: '' },
    ]))
    const s = useTestStore()
    s.applyTemplate(s.templates[0])
    expect(s.config.server).toBe('1.2.3.4:5060')
    expect(s.config.caller.cps).toBe(3)
    expect(s.config.caller.flow.steps).toEqual([])
    expect(s.config.agent.flow.codes.ready).toBe('*96')
  })
})
