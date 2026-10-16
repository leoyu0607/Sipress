import { defineStore } from 'pinia'
import { ref, computed, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { save } from '@tauri-apps/plugin-dialog'

// ── Types ─────────────────────────────────────────────────────────────────────

export type CallMode = 'caller' | 'agent'

export interface SipAccount {
  id: string
  extension: string
  username: string
  password: string
  domain: string
  status: 'idle' | 'registering' | 'registered' | 'failed'
}

export type CpsPatternType = 'flat' | 'ramp' | 'step' | 'spike' | 'batch'

export interface CpsStep {
  cps: number
  duration_secs: number
}

export interface CpsPatternConfig {
  type: CpsPatternType
  rampStart: number
  rampEnd: number
  steps: CpsStep[]
  spikeBase: number
  spikePeak: number
  spikeAtPct: number
  spikeDurationSecs: number
  batchCount: number   // 批次：每批通數
  batchEvery: number   // 批次：每幾秒一批
}

export interface CallerProfile {
  accessNumber:  string
  /** 每通使用不同主叫號碼（從 accessNumber 遞增），端到端對照拿不到 P-CIN-CallId 時靠它配對 */
  numberSeq:     boolean
  /** 撥號認證（SIPX 對 INVITE 回 401 / 407 時用）；密碼空白 = 不認證，帳號空白 = 用主叫號碼 */
  authUser:      string
  authPass:      string
  calleeFixed:   string   // '' = 用 prefix+range 隨機產生
  calleePrefix:  string
  calleeRange:   number
  concurrency:   number
  cps:           number
  cpsPattern:    CpsPatternConfig
  refillDelay:   number   // 線路釋放後幾秒才補新的通話（0 = 立即補）
  totalCalls:    number   // 0 = unlimited
  callDuration:  number   // per-call duration seconds (0 = unlimited)
  enableAudio:   boolean
  audioFile:     string
  dtmfSequence:  string
  codec:         'pcma' | 'pcmu'
  duration:      number   // 壓測持續秒數（0 = 不限）
}

/** 座席行為模擬（秒數區間在 min–max 內隨機；max = 0 停用） */
export interface AgentBehaviorConfig {
  ringMin:     number   // 響鈴幾秒後才接
  ringMax:     number
  rejectPct:   number   // 拒接（486）比例 %
  noAnswerPct: number   // 不接（等 CANCEL）比例 %
  talkMin:     number   // 接聽幾秒後座席主動掛斷
  talkMax:     number
  wrapUp:      number   // 兩通之間休息秒數
  busyInCall:  boolean  // 通話中（含響鈴中）的新來電回 486
}

/** 座席流程的一個步驟（dial：撥特殊碼；wait：等待秒數） */
export interface FlowStepConfig {
  id:    string
  type:  'dial' | 'wait'
  code:  string   // dial：撥碼範本，可含 {agent}、{pwd}
  label: string   // dial：顯示名稱（例如「就緒」）
  secs:  number   // wait：秒數
}

export type FlowPhase = 'onStart' | 'afterCall' | 'onStop'
export type FlowCodeKey = 'login' | 'ready' | 'notReady' | 'logout'

/** 座席流程：註冊後 / 每通結束後 / 結束前依序執行的撥碼與等待 */
export interface AgentFlowConfig {
  onStart:       FlowStepConfig[]
  afterCall:     FlowStepConfig[]
  onStop:        FlowStepConfig[]
  agentDigits:   number   // {agent} 取分機末幾碼（0 = 整個分機）
  agentPassword: string   // {pwd}
  holdSecs:      number   // 撥通（200）或開始播提示音（18x）後保持幾秒再掛斷
  /** 常用碼：手動按鈕與「套用 SIPX 預設」使用（只存在 GUI） */
  codes: Record<FlowCodeKey, string>
}

export const FLOW_CODE_NAMES: Record<FlowCodeKey, string> = {
  login: '登入', ready: '就緒', notReady: '未就緒', logout: '登出',
}
export const FLOW_PHASE_NAMES: Record<FlowPhase, string> = {
  onStart: '測試開始（註冊成功後）', afterCall: '每通結束後', onStop: '測試結束（解除註冊前）',
}

export function newFlowStep(type: 'dial' | 'wait', code = '', label = '', secs = 2): FlowStepConfig {
  return { id: uid(), type, code, label, secs }
}

/**
 * SIPX 預設流程：開始時登入 → 就緒；結束時未就緒 → 等待 1 秒 → 登出。
 * 自動就緒（預設）：掛線後座席自己回到就緒，每通結束後不撥碼——在掛線後撥碼會佔住分機，
 * CTI 這時派來的電話會失敗（506），座席之後不再被派話。
 * 非自動就緒：每通結束後等 3 秒（避開 SIPX 掛線後的收尾）再撥就緒。
 */
export function sipxPresetFlow(codes: Record<FlowCodeKey, string>, autoReady = true): Pick<AgentFlowConfig, FlowPhase> {
  const dial = (k: FlowCodeKey) => newFlowStep('dial', codes[k], FLOW_CODE_NAMES[k])
  return {
    onStart:   [dial('login'), newFlowStep('wait', '', '', 1), dial('ready')],
    afterCall: autoReady ? [] : [newFlowStep('wait', '', '', 3), dial('ready')],
    // 未就緒剛掛斷時 CTI 還在處理，馬上撥登出會被拒（603 CTIError2140）
    onStop:    [dial('notReady'), newFlowStep('wait', '', '', 1), dial('logout')],
  }
}

/** 代入 {agent}（分機末 agentDigits 碼）與 {pwd}；與 core 的 AgentFlow::expand 一致 */
export function expandFlowCode(code: string, ext: string, flow: Pick<AgentFlowConfig, 'agentDigits' | 'agentPassword'>): string {
  const d = flow.agentDigits
  const agent = d <= 0 || d >= ext.length ? ext : ext.slice(ext.length - d)
  return code.replaceAll('{agent}', agent).replaceAll('{pwd}', flow.agentPassword)
}

function stepToRust(st: FlowStepConfig) {
  return st.type === 'dial'
    ? { type: 'dial', code: st.code.trim(), label: st.label.trim() }
    : { type: 'wait', secs: st.secs }
}

/** 送給 Rust 的 AgentFlow（也是 --agent-flow 檔案的格式） */
export function flowToRust(f: AgentFlowConfig) {
  return {
    on_start:       f.onStart.map(stepToRust),
    after_call:     f.afterCall.map(stepToRust),
    on_stop:        f.onStop.map(stepToRust),
    agent_digits:   f.agentDigits,
    agent_password: f.agentPassword,
    hold_secs:      f.holdSecs,
  }
}

export function flowStepCount(f: AgentFlowConfig): number {
  return f.onStart.length + f.afterCall.length + f.onStop.length
}

/** 流程設定的錯誤（空陣列 = 沒問題）；與 core 的 AgentFlow::validate 一致 */
export function flowErrors(f: AgentFlowConfig): string[] {
  const errs: string[] = []
  const steps = [...f.onStart, ...f.afterCall, ...f.onStop]
  for (const st of steps) {
    if (st.type === 'dial') {
      if (!st.code.trim()) errs.push('座席流程的撥碼不可空白')
      else if (!/^[0-9A-Za-z*#{}]+$/.test(st.code.trim())) errs.push(`座席流程的撥碼「${st.code}」含不支援的字元`)
    } else if (!(Number.isFinite(st.secs) && st.secs >= 0)) {
      errs.push('座席流程的等待秒數需 ≥ 0')
    }
  }
  if (!(Number.isFinite(f.holdSecs) && f.holdSecs >= 0 && f.holdSecs <= 60)) errs.push('撥通後保持秒數需介於 0–60')
  if (!(Number.isInteger(f.agentDigits) && f.agentDigits >= 0)) errs.push('{agent} 的位數需為 ≥ 0 的整數')
  if (!f.agentPassword && steps.some(st => st.type === 'dial' && st.code.includes('{pwd}')))
    errs.push('座席流程使用 {pwd}，但尚未設定座席密碼')
  return [...new Set(errs)]
}

export interface AgentProfile {
  count: number
  accounts: SipAccount[]
  defaultDomain: string
  ackTimeout: number
  byeTimeout: number
  behavior:   AgentBehaviorConfig
  reregisterAfterCall: boolean   // 每通結束後立即重新 REGISTER（預設只在到期前刷新）
  registerInterval: number       // 各帳號開始註冊的間隔毫秒（0 = 同時註冊）
  registerExpires:  number       // REGISTER 要求的有效期秒數（伺服器可能改給其他值；2/3 時刷新）
  flow:       AgentFlowConfig     // 座席流程（撥特殊碼登入 / 就緒 / 登出）
  duration:   number   // 壓測持續秒數（0 = 不限）；通常比民眾端長，先開後關
}

export interface TestConfig {
  server: string
  transport: 'UDP' | 'TCP'
  localPort: number
  /** 僅影響 CLI 預覽（GUI 執行不使用） */
  cliFormat: 'tui' | 'json' | 'csv' | 'table'
  /** 僅影響 CLI 預覽（GUI 執行不使用） */
  cliVerbose: boolean
  mode: CallMode
  caller: CallerProfile
  agent: AgentProfile
}

export interface Metrics {
  cps: number
  concurrency: number
  succeeded: number
  failed: number
  initiated: number
  asr: number
  ccr: number
  errorRate: number
  pdd: number
  acd: number
}

export interface RtpMetrics {
  enabled: boolean
  mos: number
  packetLoss: number
  jitter: number
  packetsSent: number
  packetsRecv: number
  outOfOrder: number
}

export interface CallStates {
  initiated: number
  answered:  number
  completed: number
  failed:    number
  timeout:   number
}

export interface LogEntry {
  id: number
  ts: string
  level: 'ok' | 'info' | 'warn' | 'err' | 'dbg'
  msg: string
  /** 來源模式（兩端同時執行時用來區分）；未設定 = 共用訊息 */
  mode?: CallMode
}

export type TestStatus = 'idle' | 'running' | 'done' | 'error'

export type SeriesKey = 'cps' | 'conc' | 'asr' | 'ccr' | 'pdd' | 'fail' | 'mos'

/** 單一模式的執行狀態；民眾端與座席端各一份，可同時執行 */
export interface RunSession {
  status:       TestStatus
  /** 已送出停止、等待引擎收尾中（BYE/CANCEL）；防止重複停止 */
  stopping:     boolean
  elapsedSec:   number
  metrics:      Metrics
  series:       Record<SeriesKey, number[]>
  /** 每個系列已被捨棄（shift 掉）的筆數，用於 X 軸時間標籤 */
  seriesOffset: Record<SeriesKey, number>
  /** 每次系列資料變動 +1；圖表監聽此值重繪（長度到上限後不再變化） */
  seriesTick:   number
  callStates:   CallStates
  respCodes:    Record<string, number>
  rtpMetrics:   RtpMetrics
  /** 座席端註冊狀態（民眾端皆為 0） */
  agents:       { registered: number; total: number }
}

// Rust StatsSnapshot shape
export interface RustSnapshot {
  calls_initiated:  number
  calls_answered:   number
  calls_completed:  number
  calls_failed:     number
  calls_timeout:    number
  calls_concurrent: number
  rtp_sessions:     number   // 目前活躍 RTP session 數（> 0 = 音訊傳送中）
  asr:              number
  error_rate:       number
  // 即時 RTP 品質（null = RTP 未啟用或無活躍 session）
  rtp_mos:           number | null
  rtp_loss_pct:      number | null
  rtp_jitter_ms:     number | null
  rtp_sent_packets:  number | null
  rtp_recv_packets:  number | null
  // 目前為止的 PDD 平均 / 中位數（null = 尚無樣本）；畫面使用中位數，與最終報告的 p50 同口徑
  pdd_avg_ms:        number | null
  pdd_p50_ms:        number | null
  // 最近一秒的實際發起速率（引擎計算）
  cps:               number
  // 座席端：註冊成功數 / 帳號總數（民眾端皆為 0）
  agents_registered: number
  agents_total:      number
  // 座席端：座席流程的撥碼成功 / 失敗累計
  agent_dials_ok:     number
  agent_dials_failed: number
  // 引擎是否已結束
  finished:          boolean
}

// Rust FinalReport shape (subset we use)
export interface RustReport {
  calls_initiated:  number
  calls_answered:   number
  calls_completed:  number
  calls_failed:     number
  calls_timeout:    number
  asr:              number
  ccr:              number
  actual_cps:       number
  acd_secs:         number
  pdd_p50_ms:       number
  pdd_p95_ms:       number
  mos:              number | null
  loss_rate_pct:    number | null
  jitter_ms:        number | null
  rtp_sent:         number | null
  rtp_recv:         number | null
  rtp_out_of_order: number | null
  fail_codes:       Record<string, number>
  // 座席端：座席流程每個撥碼的結果（沒有流程時不存在）
  agent_dials?:     { code: string; label: string; ok: number; failed: number; fail_codes: Record<string, number> }[]
}

// ── Constants ─────────────────────────────────────────────────────────────────
export const MAX_SERIES  = 90
const MAX_LOG     = 300
const STORAGE_KEY = 'sipress-config'
/** 雙端啟動：等待座席全部註冊成功的上限；逾時仍有部分成功則繼續撥號 */
export const AGENT_REGISTER_TIMEOUT_MS = 10000
/** 雙端啟動檢查座席註冊狀態的間隔 */
const BOTH_POLL_MS = 250

export const MODE_LABEL: Record<CallMode, string> = { caller: '民眾端', agent: '座席端' }

export const CODEC_LABEL: Record<CallerProfile['codec'], { short: string; full: string }> = {
  pcma: { short: 'G.711A', full: 'G.711A (PCMA)' },
  pcmu: { short: 'G.711U', full: 'G.711U (PCMU)' },
}

// ── Helpers ───────────────────────────────────────────────────────────────────
function uid()    { return Math.random().toString(36).slice(2, 8) }

function nowTs() {
  const d = new Date()
  return [d.getHours(), d.getMinutes(), d.getSeconds()]
    .map(n => String(n).padStart(2, '0')).join(':')
    + '.' + String(d.getMilliseconds()).padStart(3, '0')
}

function fileTs() {
  const d   = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}${pad(d.getMonth()+1)}${pad(d.getDate())}_${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`
}

function emptySeries(): Record<SeriesKey, number[]> {
  return { cps: [], conc: [], asr: [], ccr: [], pdd: [], fail: [], mos: [] }
}

function newSession(): RunSession {
  return {
    status: 'idle', stopping: false, elapsedSec: 0,
    metrics: { cps:0, concurrency:0, succeeded:0, failed:0, initiated:0, asr:0, ccr:0, errorRate:0, pdd:0, acd:0 },
    series: emptySeries(),
    seriesOffset: { cps:0, conc:0, asr:0, ccr:0, pdd:0, fail:0, mos:0 },
    seriesTick: 0,
    callStates: { initiated:0, answered:0, completed:0, failed:0, timeout:0 },
    respCodes: {},
    rtpMetrics: { enabled:false, mos:0, packetLoss:0, jitter:0, packetsSent:0, packetsRecv:0, outOfOrder:0 },
    agents: { registered: 0, total: 0 },
  }
}

/** MOS 分級（全 App 共用）：≥4.0 優 / ≥3.0 普通 / 其餘 差 */
export function mosGrade(m: number): { key: 'excellent' | 'fair' | 'poor' | 'none'; label: string; color: string } {
  if (m >= 4.0) return { key: 'excellent', label: '優',   color: 'var(--accent)' }
  if (m >= 3.0) return { key: 'fair',      label: '普通', color: 'var(--warn)' }
  if (m > 0)    return { key: 'poor',      label: '差',   color: 'var(--danger)' }
  return { key: 'none', label: '—', color: 'var(--text2)' }
}

function fmtDuration(sec: number) {
  return sec > 0 ? `${sec}s` : '不限'
}

function defaultConfig(): TestConfig {
  return {
    server: '192.168.166.60:5066',
    transport: 'UDP',
    localPort: 5070,
    cliFormat: 'tui',
    cliVerbose: false,
    mode: 'caller',
    caller: {
      accessNumber: '0938530031',
      numberSeq:    false,
      authUser:     '',
      authPass:     '',
      calleeFixed:  '',
      calleePrefix: '2',
      calleeRange:  9999,
      concurrency:  100,
      cps:          10,
      cpsPattern: {
        type: 'flat',
        rampStart: 1,
        rampEnd: 50,
        steps: [{ cps: 10, duration_secs: 30 }, { cps: 50, duration_secs: 30 }],
        spikeBase: 10,
        spikePeak: 100,
        spikeAtPct: 50,
        spikeDurationSecs: 10,
        batchCount: 3,
        batchEvery: 20,
      },
      refillDelay:  0,
      totalCalls:   0,
      callDuration: 30,
      enableAudio:  false,
      audioFile:    '',
      dtmfSequence: '',
      codec:        'pcma',
      duration:     60,
    },
    agent: {
      count: 0,
      accounts: [],
      defaultDomain: '192.168.166.60',
      ackTimeout: 10,
      byeTimeout: 150,
      // 響鈴 1–2 秒再接：0 秒時 100 / 180 / 200 同時送出，SIPX 的 CTI 可能漏掉「接聽」事件
      behavior: { ringMin: 1, ringMax: 2, rejectPct: 0, noAnswerPct: 0, talkMin: 0, talkMax: 0, wrapUp: 0, busyInCall: false },
      reregisterAfterCall: false,
      registerInterval: 1000,
      registerExpires: 3600,
      flow: {
        onStart: [], afterCall: [], onStop: [],
        agentDigits: 4, agentPassword: '', holdSecs: 1,
        codes: { login: '*94{agent}*{pwd}', ready: '*96', notReady: '*97', logout: '*95' },
      },
      duration: 0,
    },
  }
}

/** 從 localStorage 載入設定，與預設值合併（新版本新增的欄位會取預設值） */
function loadConfig(): TestConfig {
  const def = defaultConfig()
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return def
    const saved = JSON.parse(raw) as Partial<TestConfig> & { duration?: number }
    // v0.4 以前持續時間是兩端共用的頂層欄位 → 兩端都沿用舊值
    const legacyDuration = typeof saved.duration === 'number' ? { duration: saved.duration } : {}
    delete saved.duration
    const cfg: TestConfig = {
      ...def,
      ...saved,
      caller: {
        ...def.caller,
        ...legacyDuration,
        ...saved.caller,
        cpsPattern: { ...def.caller.cpsPattern, ...saved.caller?.cpsPattern },
      },
      agent: {
        ...def.agent,
        ...legacyDuration,
        ...saved.agent,
        behavior: { ...def.agent.behavior, ...saved.agent?.behavior },
        flow: {
          ...def.agent.flow,
          ...saved.agent?.flow,
          codes: { ...def.agent.flow.codes, ...saved.agent?.flow?.codes },
        },
      },
    }
    if (cfg.transport !== 'UDP' && cfg.transport !== 'TCP') cfg.transport = 'UDP'
    // 註冊狀態不跨啟動保留
    cfg.agent.accounts = (cfg.agent.accounts ?? []).map(a => ({ ...a, status: 'idle' as const }))
    return cfg
  } catch {
    return def
  }
}

// ── Store ─────────────────────────────────────────────────────────────────────
/** DTMF 序列：逗號分隔，每段為單一按鍵（0-9 * # A-D）或 wN（等待 N 毫秒）；與 core 的檢查一致 */
export function isValidDtmf(seq: string): boolean {
  const parts = seq.split(',').map(x => x.trim()).filter(Boolean)
  return parts.length > 0 && parts.every(x => /^[0-9*#A-Da-d]$/.test(x) || /^[wW]\d+$/.test(x))
}

/** CLI 預覽的 CPS 參數（依 CPS 模式） */
export function cpsFlag(pat: CpsPatternConfig, cps: number): string {
  switch (pat.type) {
    case 'ramp':  return `--cps-ramp ${pat.rampStart}-${pat.rampEnd}`
    case 'step':  return `--cps-steps ${pat.steps.map(st => `${st.cps}:${st.duration_secs}`).join(',')}`
    case 'spike': return `--cps-spike ${pat.spikeBase},${pat.spikePeak},${pat.spikeAtPct},${pat.spikeDurationSecs}`
    case 'batch': return `--cps-batch ${pat.batchCount}/${pat.batchEvery}`
    default:      return `--cps ${cps}`
  }
}

export const useTestStore = defineStore('test', () => {

  const config = ref<TestConfig>(loadConfig())

  watch(config, c => {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(c)) } catch { /* quota / private mode */ }
  }, { deep: true })

  const sessions = ref<Record<CallMode, RunSession>>({ caller: newSession(), agent: newSession() })
  /** 畫面目前檢視的模式（= 模式分頁） */
  const view = computed(() => sessions.value[config.value.mode])

  const status       = computed(() => view.value.status)
  const stopping     = computed(() => view.value.stopping)
  const elapsedSec   = computed(() => view.value.elapsedSec)
  const metrics      = computed(() => view.value.metrics)
  const series       = computed(() => view.value.series)
  const seriesOffset = computed(() => view.value.seriesOffset)
  const seriesTick   = computed(() => view.value.seriesTick)
  const callStates   = computed(() => view.value.callStates)
  const respCodes    = computed(() => view.value.respCodes)
  const rtpMetrics   = computed(() => view.value.rtpMetrics)
  const agents       = computed(() => view.value.agents)

  const logs        = ref<LogEntry[]>([])
  const accountImportError = ref('')

  /** 不需反應式的每模式執行期資料 */
  const runtime: Record<CallMode, {
    pollTimer:     ReturnType<typeof setInterval> | null
    clockTimer:    ReturnType<typeof setInterval> | null
    prevFailed:    number    // 上一筆 snapshot 的失敗 + 逾時累計（只在增加時寫 WARN）
    rtpLoggedOnce: boolean   // 防止重複寫 RTP session 啟動 log
    prevDials:     number    // 上一筆 snapshot 的撥碼成功 + 失敗累計（只在增加時寫 log）
  }> = {
    caller: { pollTimer: null, clockTimer: null, prevFailed: 0, rtpLoggedOnce: false, prevDials: 0 },
    agent:  { pollTimer: null, clockTimer: null, prevFailed: 0, rtpLoggedOnce: false, prevDials: 0 },
  }
  let logSeq = 0
  /** 雙端啟動中等待座席註冊的輪詢；取消時 resolve('cancelled')，startBoth 不再啟動民眾端 */
  type BothWaitResult = 'ready' | 'timeout' | 'cancelled'
  let bothWait: { timer: ReturnType<typeof setInterval>; resolve: (r: BothWaitResult) => void } | null = null
  function settleBothWait(r: BothWaitResult) {
    if (!bothWait) return
    clearInterval(bothWait.timer)
    bothWait.resolve(r)
    bothWait = null
  }
  function cancelBothWait() { settleBothWait('cancelled') }

  // ── Computed ──────────────────────────────────────────────────────────────────
  const isRunning = computed(() => status.value === 'running')
  const anyRunning = computed(() => sessions.value.caller.status === 'running' || sessions.value.agent.status === 'running')

  const mosRating = computed(() => mosGrade(rtpMetrics.value.mos).key)

  const codecLabel = computed(() => CODEC_LABEL[config.value.caller.codec] ?? CODEC_LABEL.pcma)

  const registeredCount = computed(() =>
    config.value.agent.accounts.filter(a => a.status === 'registered').length
  )

  /** 啟動前驗證；非空代表無法開始該模式的測試 */
  function validate(mode: CallMode): string[] {
    const c = config.value
    const errs: string[] = []
    if (!c.server.trim()) errs.push('SIP server 不可空白')
    else if (!/^[^\s:]+:\d{1,5}$/.test(c.server.trim())) errs.push('SIP server 格式需為 host:port')
    if (mode === 'caller') {
      const p = c.caller
      if (c.localPort !== 0 && !(Number.isInteger(c.localPort) && c.localPort >= 1024 && c.localPort <= 65535))
        errs.push('本地埠需為 0（自動）或 1024–65535')
      if (!p.accessNumber.trim()) errs.push('主叫號碼不可空白')
      if (!(p.concurrency >= 1)) errs.push('併發數量需 ≥ 1')
      if (p.cpsPattern.type === 'flat' && !(p.cps > 0)) errs.push('CPS 需 > 0')
      if (p.cpsPattern.type === 'step' && p.cpsPattern.steps.length === 0) errs.push('階梯模式至少需要一個階層')
      if (p.cpsPattern.type === 'batch') {
        if (!(Number.isInteger(p.cpsPattern.batchCount) && p.cpsPattern.batchCount >= 1)) errs.push('批次模式每批通數需為 ≥ 1 的整數')
        if (!(p.cpsPattern.batchEvery > 0)) errs.push('批次模式的間隔秒數需 > 0')
      }
      if (!(Number.isFinite(p.refillDelay) && p.refillDelay >= 0)) errs.push('補話延遲需 ≥ 0 秒')
      if ((p.cpsPattern.type === 'ramp' || p.cpsPattern.type === 'spike') && p.duration === 0)
        errs.push(`${p.cpsPattern.type === 'ramp' ? '漸增' : '尖峰'}模式需設定持續時間（> 0）`)
      if (p.enableAudio && !p.audioFile.trim()) errs.push('已啟用播放音檔但尚未選擇檔案')
      if (p.dtmfSequence.trim() && !isValidDtmf(p.dtmfSequence))
        errs.push('DTMF 序列格式錯誤：每段為單一按鍵（0-9 * # A-D）或 wN（等待毫秒），以逗號分隔')
    } else {
      if (c.agent.accounts.length === 0) errs.push('座席模式需要至少一個帳號')
      const b = c.agent.behavior
      const nonNeg = (v: number) => Number.isFinite(v) && v >= 0
      if (![b.ringMin, b.ringMax, b.talkMin, b.talkMax, b.wrapUp].every(nonNeg)) errs.push('座席行為的秒數需 ≥ 0')
      else {
        if (b.ringMin > b.ringMax) errs.push('響鈴秒數最小值不可大於最大值')
        if (b.talkMin > b.talkMax) errs.push('通話秒數最小值不可大於最大值')
      }
      const pctOk = (v: number) => Number.isFinite(v) && v >= 0 && v <= 100
      if (!(Number.isInteger(c.agent.registerExpires) && c.agent.registerExpires >= 30))
        errs.push('註冊有效期需為 ≥ 30 的整數秒')
      if (!(Number.isInteger(c.agent.registerInterval) && c.agent.registerInterval >= 0))
        errs.push('註冊間隔需為 ≥ 0 的整數毫秒')
      errs.push(...flowErrors(c.agent.flow))
      if (!pctOk(b.rejectPct) || !pctOk(b.noAnswerPct)) errs.push('拒接 / 不接比例需介於 0–100%')
      else if (b.rejectPct + b.noAnswerPct > 100) errs.push('拒接 + 不接比例合計不可超過 100%')
    }
    return errs
  }
  const validationErrors = computed(() => validate(config.value.mode))
  /** 雙端啟動的驗證（兩端錯誤合併，去除重複的共用欄位錯誤） */
  const bothValidationErrors = computed(() => [...new Set([...validate('agent'), ...validate('caller')])])

  const cliCommand = computed(() => {
    const c = config.value
    const p      = c.caller
    const parts  = [`./sipress -s ${c.server}`]
    if (c.mode === 'agent') {
      // 帳號檔由「匯出 CSV」產生（sipress_accounts.csv）
      const a = c.agent
      parts.push('--mode agent --accounts sipress_accounts.csv')
      if (a.defaultDomain.trim()) parts.push(`--account-domain ${a.defaultDomain.trim()}`)
      parts.push(`-d ${a.duration}`, `--ack-timeout ${a.ackTimeout}`, `--bye-timeout ${a.byeTimeout}`)
      const b = a.behavior
      const range = (min: number, max: number) => min === max ? `${max}` : `${min}-${max}`
      if (b.ringMax > 0)     parts.push(`--ring ${range(b.ringMin, b.ringMax)}`)
      if (b.rejectPct > 0)   parts.push(`--reject-pct ${b.rejectPct}`)
      if (b.noAnswerPct > 0) parts.push(`--no-answer-pct ${b.noAnswerPct}`)
      if (b.talkMax > 0)     parts.push(`--talk ${range(b.talkMin, b.talkMax)}`)
      if (b.wrapUp > 0)      parts.push(`--wrap-up ${b.wrapUp}`)
      if (b.busyInCall)      parts.push('--busy-in-call')
      if (a.reregisterAfterCall) parts.push('--reregister-after-call')
      if (a.registerInterval > 0) parts.push(`--register-interval ${a.registerInterval}`)
      if (a.registerExpires !== 3600) parts.push(`--register-expires ${a.registerExpires}`)
      // 流程檔由「座席流程」的「匯出 JSON」產生（sipress_flow.json）
      if (flowStepCount(a.flow) > 0) parts.push('--agent-flow sipress_flow.json')
      parts.push(`--transport ${c.transport.toLowerCase()}`)
    } else {
      parts.push(`--from ${p.accessNumber}`)
      if (p.numberSeq) parts.push('--from-seq')
      // 密碼不顯示在預覽（畫面截圖、複製貼上時外洩），只放佔位字
      if (p.authPass) parts.push(...(p.authUser.trim() ? [`--auth-user ${p.authUser.trim()}`] : []), '--auth-pass "<密碼>"')
      parts.push(p.calleeFixed ? `--to ${p.calleeFixed}` : `--to-prefix ${p.calleePrefix} --to-range ${p.calleeRange}`)
      parts.push(`-c ${p.concurrency}`, cpsFlag(p.cpsPattern, p.cps), `-d ${p.duration}`, `--call-duration ${p.callDuration}`)
      if (p.refillDelay > 0) parts.push(`--refill-delay ${p.refillDelay}`)
      if (p.totalCalls > 0) parts.push(`--max-calls ${p.totalCalls}`)
      parts.push(`--transport ${c.transport.toLowerCase()}`)
      if (c.localPort) parts.push(`--local 0.0.0.0:${c.localPort}`)
    }
    if (p.codec !== 'pcma') parts.push(`--codec ${p.codec}`)
    if (p.enableAudio && p.audioFile) {
      parts.push(`--rtp --rtp-port 16000 --audio "${p.audioFile}"`)
      // DTMF 經 RTP 送出，沒有 RTP 時 GUI 也不會送
      if (c.mode === 'caller' && p.dtmfSequence.trim()) parts.push(`--dtmf "${p.dtmfSequence.trim()}"`)
    }
    parts.push(c.cliFormat === 'tui' ? '--tui' : `--format ${c.cliFormat}`)
    if (c.cliVerbose) parts.push('--verbose')
    return parts.join(' ')
  })

  const progressPct = computed(() => {
    const duration = config.value[config.value.mode].duration
    return status.value === 'running' && duration > 0
      ? Math.min(100, (elapsedSec.value / duration) * 100)
      : 0  // duration=0 means unlimited → no progress bar
  })

  // ── Log ───────────────────────────────────────────────────────────────────────
  function addLog(level: LogEntry['level'], msg: string, mode?: CallMode) {
    logs.value.push({ id: ++logSeq, ts: nowTs(), level, msg, ...(mode ? { mode } : {}) })
    if (logs.value.length > MAX_LOG) logs.value.shift()
  }
  function clearLog() { logs.value = [] }

  // ── Series ────────────────────────────────────────────────────────────────────
  function pushSeries(r: RunSession, key: SeriesKey, val: number) {
    r.series[key].push(val)
    if (r.series[key].length > MAX_SERIES) {
      r.series[key].shift()
      r.seriesOffset[key]++
    }
    r.seriesTick++
  }

  // ── CPS pattern builder ──────────────────────────────────────────────────────
  function buildCpsPattern(p: CpsPatternConfig) {
    switch (p.type) {
      case 'ramp':  return { type: 'ramp',  start: p.rampStart, end: p.rampEnd }
      case 'step':  return { type: 'step',  levels: p.steps }
      case 'spike': return { type: 'spike', base: p.spikeBase, peak: p.spikePeak, spike_at_pct: p.spikeAtPct, spike_duration_secs: p.spikeDurationSecs }
      case 'batch': return { type: 'batch', count: p.batchCount, every_secs: p.batchEvery }
      default:      return { type: 'flat' }
    }
  }

  // ── Rust config builder ───────────────────────────────────────────────────────
  function buildRustConfig(mode: CallMode = config.value.mode) {
    const c = config.value
    return {
      server_addr:          c.server.trim(),
      // 座席端每個帳號各自綁定隨機 port，本地埠只套用在民眾端
      local_addr:           mode === 'caller' && c.localPort ? `0.0.0.0:${c.localPort}` : null,
      local_domain:         null,
      caller_number:        c.caller.accessNumber,
      caller_number_seq:    c.caller.numberSeq,
      caller_auth_username: c.caller.authUser.trim() || null,
      caller_auth_password: c.caller.authPass || null,
      callee_prefix:        c.caller.calleePrefix || '2',
      callee_range:         c.caller.calleeRange  || 9999,
      callee_fixed:         c.caller.calleeFixed ? c.caller.calleeFixed : null,
      cps:                  c.caller.cps,
      cps_pattern:          buildCpsPattern(c.caller.cpsPattern),
      refill_delay_secs:    c.caller.refillDelay,
      max_concurrent_calls: c.caller.concurrency,
      max_total_calls:      c.caller.totalCalls > 0 ? c.caller.totalCalls : null,
      duration_secs:        c[mode].duration,
      call_duration_secs:   c.caller.callDuration,
      invite_timeout_secs:  8,
      transport:            c.transport.toLowerCase(),
      logs_dir:             'logs',
      rtp_base_port:        16000,
      audio_file:           c.caller.enableAudio && c.caller.audioFile ? c.caller.audioFile : null,
      codec:                c.caller.codec,
      enable_rtp:           c.caller.enableAudio && !!c.caller.audioFile,
      dtmf_sequence:        c.caller.dtmfSequence || null,
      mode,                                                                  // 'caller' | 'agent'
      agent_accounts:       mode === 'agent'
        ? c.agent.accounts.map(a => ({
            extension: a.extension,
            username:  a.username,
            password:  a.password,
            domain:    a.domain || c.agent.defaultDomain,
          }))
        : [],
      agent_ack_timeout_secs: c.agent.ackTimeout,
      agent_reregister_after_call: c.agent.reregisterAfterCall,
      agent_register_interval_ms: c.agent.registerInterval,
      agent_register_expires: c.agent.registerExpires,
      agent_flow:           flowToRust(c.agent.flow),
      agent_bye_timeout_secs: c.agent.byeTimeout,
      agent_behavior: {
        ring_min_secs: c.agent.behavior.ringMin,
        ring_max_secs: c.agent.behavior.ringMax,
        reject_pct:    c.agent.behavior.rejectPct,
        no_answer_pct: c.agent.behavior.noAnswerPct,
        talk_min_secs: c.agent.behavior.talkMin,
        talk_max_secs: c.agent.behavior.talkMax,
        wrap_up_secs:  c.agent.behavior.wrapUp,
        busy_in_call:  c.agent.behavior.busyInCall,
      },
    }
  }

  // ── Snapshot → store ─────────────────────────────────────────────────────────
  function applySnapshot(mode: CallMode, snap: RustSnapshot) {
    const r   = sessions.value[mode]
    const rt  = runtime[mode]
    // CPS 由引擎每秒計算，不用前端輪詢差值（兩個計時器不同步會出現 0 與兩倍交替）
    const cps = snap.cps
    const failed = snap.calls_failed + snap.calls_timeout

    r.metrics = {
      cps,
      concurrency: snap.calls_concurrent,
      succeeded:   snap.calls_answered,
      failed,
      initiated:   snap.calls_initiated,
      asr:         snap.asr,
      ccr:         snap.calls_initiated > 0
        ? snap.calls_completed / snap.calls_initiated * 100
        : 0,
      errorRate:   snap.error_rate,
      pdd:         snap.pdd_p50_ms ?? 0,
      acd:         0,
    }

    r.callStates = {
      initiated: snap.calls_initiated,
      answered:  snap.calls_answered,
      completed: snap.calls_completed,
      failed:    snap.calls_failed,
      timeout:   snap.calls_timeout,
    }

    pushSeries(r, 'cps',  cps)
    pushSeries(r, 'conc', snap.calls_concurrent)
    pushSeries(r, 'asr',  snap.asr)
    pushSeries(r, 'ccr',  r.metrics.ccr)
    pushSeries(r, 'fail', failed)
    if (snap.pdd_p50_ms != null) pushSeries(r, 'pdd', snap.pdd_p50_ms)

    r.agents = { registered: snap.agents_registered ?? 0, total: snap.agents_total ?? 0 }

    // Approximate live respCodes from snapshot counters
    r.respCodes['200'] = snap.calls_answered
    r.respCodes['408'] = snap.calls_timeout

    // 只在有新增失敗時記錄（累計值每秒重寫會洗掉其他事件）
    if (failed > rt.prevFailed)
      addLog('warn', `新增失敗 ${failed - rt.prevFailed}（失敗 ${snap.calls_failed} 逾時 ${snap.calls_timeout}）  發起 ${snap.calls_initiated}  ASR ${snap.asr.toFixed(1)}%`, mode)
    rt.prevFailed = Math.max(rt.prevFailed, failed)

    // 座席流程撥碼：有新結果時記錄累計
    const dialOk = snap.agent_dials_ok ?? 0
    const dialFailed = snap.agent_dials_failed ?? 0
    if (dialOk + dialFailed > rt.prevDials) {
      addLog(dialFailed > 0 ? 'warn' : 'ok', `座席撥碼累計：成功 ${dialOk}  失敗 ${dialFailed}`, mode)
      rt.prevDials = dialOk + dialFailed
    }

    // RTP session 啟動後只寫一次 log
    if ((snap.rtp_sessions ?? 0) > 0 && !rt.rtpLoggedOnce) {
      rt.rtpLoggedOnce = true
      addLog('ok', `RTP ${codecLabel.value.short} 音訊傳送中（${snap.rtp_sessions} session${snap.rtp_sessions > 1 ? 's' : ''} active）`, mode)
    }

    // 即時 RTP 品質更新
    if (snap.rtp_mos !== null && snap.rtp_mos !== undefined) {
      r.rtpMetrics = {
        enabled:     true,
        mos:         snap.rtp_mos,
        packetLoss:  snap.rtp_loss_pct ?? 0,
        jitter:      snap.rtp_jitter_ms ?? 0,
        packetsSent: snap.rtp_sent_packets ?? 0,
        packetsRecv: snap.rtp_recv_packets ?? 0,
        outOfOrder:  0,
      }
      pushSeries(r, 'mos', snap.rtp_mos)
    }
  }

  /** 取得最終報告；引擎執行失敗時回傳 false（get_report 會丟出錯誤原因） */
  async function _tryFetchReport(mode: CallMode): Promise<boolean> {
    const r = sessions.value[mode]
    try {
      const report = await invoke<RustReport | null>('get_report', { mode })
      if (!report) return true
      r.metrics = {
        ...r.metrics,
        // 引擎結束時所有通話都已結束（最後一筆快照可能是收尾前取的）
        concurrency: 0,
        cps:       report.actual_cps,
        succeeded: report.calls_answered,
        failed:    report.calls_failed + report.calls_timeout,
        initiated: report.calls_initiated,
        asr:       report.asr,
        ccr:       report.ccr,
        errorRate: report.calls_initiated > 0
          ? (report.calls_failed + report.calls_timeout) / report.calls_initiated * 100
          : 0,
        pdd:       report.pdd_p50_ms,
        acd:       report.acd_secs,
      }
      r.callStates = {
        initiated: report.calls_initiated,
        answered:  report.calls_answered,
        completed: report.calls_completed,
        failed:    report.calls_failed,
        timeout:   report.calls_timeout,
      }
      if (report.mos !== null) {
        r.rtpMetrics = {
          enabled:     true,
          mos:         report.mos ?? 0,
          packetLoss:  report.loss_rate_pct ?? 0,
          jitter:      report.jitter_ms ?? 0,
          packetsSent: report.rtp_sent ?? 0,
          packetsRecv: report.rtp_recv ?? 0,
          outOfOrder:  report.rtp_out_of_order ?? 0,
        }
        pushSeries(r, 'mos', report.mos ?? 0)
      }
      // Populate respCodes from final per-code breakdown
      if (report.fail_codes) {
        for (const [code, count] of Object.entries(report.fail_codes)) {
          r.respCodes[code] = count
        }
      }
      // 200 and 408 come from summary counters (may not appear in fail_codes)
      r.respCodes['200'] = report.calls_answered
      r.respCodes['408'] = report.calls_timeout
      for (const d of report.agent_dials ?? []) {
        const codes = Object.entries(d.fail_codes ?? {})
          .map(([c, n]) => `${c === '0' ? '無回應' : c}×${n}`).join(' ')
        addLog(d.failed > 0 ? 'warn' : 'ok',
          `撥碼 ${d.label || d.code}（${d.code}）：成功 ${d.ok}  失敗 ${d.failed}${codes ? `  ${codes}` : ''}`, mode)
      }
      addLog('ok', `測試完成  發起 ${report.calls_initiated}  接通 ${report.calls_answered}  失敗 ${report.calls_failed + report.calls_timeout}  ASR ${report.asr.toFixed(1)}%  CPS ${report.actual_cps.toFixed(1)}  PDD p50 ${report.pdd_p50_ms.toFixed(0)}ms`, mode)
      return true
    } catch (e) {
      addLog('err', `壓測執行失敗: ${e}`, mode)
      return false
    }
  }

  function _finishTest(mode: CallMode) {
    const r  = sessions.value[mode]
    const rt = runtime[mode]
    if (rt.pollTimer)  { clearInterval(rt.pollTimer);  rt.pollTimer  = null }
    if (rt.clockTimer) { clearInterval(rt.clockTimer); rt.clockTimer = null }
    r.stopping = false
    _tryFetchReport(mode).then(ok => { r.status = ok ? 'done' : 'error' })
  }

  // ── Start / Stop ──────────────────────────────────────────────────────────────
  /** 啟動指定模式（預設為目前檢視的模式）；另一模式可同時執行 */
  async function startTest(mode: CallMode = config.value.mode): Promise<boolean> {
    const r  = sessions.value[mode]
    const rt = runtime[mode]
    if (r.status === 'running') return false
    const errs = validate(mode)
    if (errs.length > 0) {
      for (const e of errs) addLog('err', e, mode)
      return false
    }
    sessions.value[mode] = { ...newSession(), status: 'running', seriesTick: r.seriesTick + 1 }
    rt.prevFailed = 0
    rt.rtpLoggedOnce = false
    rt.prevDials = 0
    const cur = sessions.value[mode]

    const c = config.value
    try {
      await invoke('start_test', { config: buildRustConfig(mode) })
      addLog('ok',   `測試開始 → ${c.server}  [${MODE_LABEL[mode]}]`, mode)
      if (mode === 'caller') {
        addLog('info', `持續時間: ${fmtDuration(c.caller.duration)}  cps: ${c.caller.cps}  併發: ${c.caller.concurrency}${c.caller.totalCalls > 0 ? `  總通數: ${c.caller.totalCalls}` : ''}`, mode)
        if (c.caller.enableAudio && c.caller.audioFile) {
          const fname = c.caller.audioFile.replace(/\\/g, '/').split('/').pop() ?? c.caller.audioFile
          addLog('info', `RTP codec=${codecLabel.value.full}  audio="${fname}"  port=${buildRustConfig(mode).rtp_base_port}`, mode)
        } else {
          addLog('info', `RTP 未啟用（僅 SIP signaling）`, mode)
        }
      } else {
        const total = c.agent.accounts.length
        addLog('info', `座席端：${total} 個帳號待註冊；持續時間: ${fmtDuration(c.agent.duration)}`, mode)
        addLog('info', `行為：每個帳號 REGISTER → 等待 INVITE → 自動 200 OK → 等 BYE → 結束 deregister`, mode)
      }
    } catch (e) {
      addLog('err', `啟動失敗: ${e}`, mode)
      cur.status = 'error'
      return false
    }

    rt.pollTimer = setInterval(async () => {
      try {
        const snap = await invoke<RustSnapshot | null>('get_snapshot', { mode })
        if (snap) {
          applySnapshot(mode, snap)
          // 引擎已結束（duration 到 / max_total_calls 完成 / graceful stop）
          if (snap.finished) {
            _finishTest(mode)
          }
        }
      } catch { /* ignore */ }
    }, 1000)

    rt.clockTimer = setInterval(() => {
      cur.elapsedSec++
    }, 1000)
    return true
  }

  async function stopTest(mode: CallMode = config.value.mode) {
    const r  = sessions.value[mode]
    const rt = runtime[mode]
    if (r.stopping) return
    // 雙端啟動仍在等待座席註冊 → 取消後續的民眾端啟動
    cancelBothWait()
    r.stopping = true
    if (rt.clockTimer) { clearInterval(rt.clockTimer); rt.clockTimer = null }
    addLog('warn', `測試已手動停止  經過: ${r.elapsedSec}s`, mode)
    try {
      await invoke('stop_test', { mode })
    } catch {
      // 引擎已結束（沒有可停止的壓測）→ 直接收尾取報告
      _finishTest(mode)
      return
    }
    // 引擎會先對進行中的通話送 BYE/CANCEL 並等待回應，
    // 由輪詢偵測 snapshot.finished 後再取最終報告
    addLog('info', '等待進行中通話收尾（BYE/CANCEL）…', mode)
    if (!rt.pollTimer) _finishTest(mode)
  }

  /**
   * 雙端啟動：先啟動座席端（REGISTER），等座席全部註冊成功再啟動民眾端撥號。
   * 等待超過 AGENT_REGISTER_TIMEOUT_MS：有部分座席註冊成功就繼續並警告；一個都沒有則不撥號。
   */
  async function startBoth() {
    if (anyRunning.value || bothWait) return
    if (bothValidationErrors.value.length > 0) {
      for (const e of bothValidationErrors.value) addLog('err', e)
      return
    }
    const cd = config.value.caller.duration
    const ad = config.value.agent.duration
    if (ad > 0 && (cd === 0 || ad < cd))
      addLog('warn', `座席端持續時間（${fmtDuration(ad)}）比民眾端（${fmtDuration(cd)}）短，座席會先結束，後段來電將無人接聽`)
    if (!await startTest('agent')) return
    // 分批註冊時，最後一個帳號要 (帳號數 - 1) × 間隔後才開始註冊，等待上限一併延長
    const a0 = config.value.agent
    const waitMs = AGENT_REGISTER_TIMEOUT_MS + a0.registerInterval * Math.max(0, a0.accounts.length - 1)
    addLog('info', `雙端啟動：等待座席註冊完成（最多 ${fmtDuration(Math.ceil(waitMs / 1000))}）後開始撥號`)
    const deadline = Date.now() + waitMs
    const result = await new Promise<BothWaitResult>(resolve => {
      const timer = setInterval(() => {
        const a = sessions.value.agent
        if (a.status !== 'running' || a.stopping) settleBothWait('cancelled')
        else if (a.agents.total > 0 && a.agents.registered >= a.agents.total) settleBothWait('ready')
        else if (Date.now() >= deadline) settleBothWait('timeout')
      }, BOTH_POLL_MS)
      bothWait = { timer, resolve }
    })
    const { registered, total } = sessions.value.agent.agents
    if (result === 'cancelled') {
      addLog('warn', '雙端啟動已取消，民眾端不啟動')
      return
    }
    if (result === 'timeout') {
      if (registered === 0) {
        addLog('err', `座席在 ${fmtDuration(Math.ceil(waitMs / 1000))} 內沒有任何帳號註冊成功，民眾端不啟動`)
        return
      }
      addLog('warn', `僅 ${registered}/${total} 個座席註冊成功，仍開始撥號`)
    } else {
      addLog('ok', `座席全部註冊成功（${registered}/${total}），開始撥號`)
    }
    await startTest('caller')
  }

  /** 停止所有執行中的模式 */
  async function stopAll() {
    cancelBothWait()
    const modes = (['caller', 'agent'] as const).filter(m => sessions.value[m].status === 'running')
    await Promise.all(modes.map(m => stopTest(m)))
  }

  // ── SIP Account Management ────────────────────────────────────────────────────
  interface GenerateOptions {
    count: number
    startExt: number
    usernamePrefix: string
    passwordMode: 'same' | 'ext' | 'custom'
    samePassword: string
    customPassword: string
    domain: string
  }

  function generateAccounts(opts: GenerateOptions) {
    const existing = new Set(config.value.agent.accounts.map(a => a.extension))
    const created: SipAccount[] = []
    for (let i = 0; i < opts.count; i++) {
      const ext  = String(opts.startExt + i)
      if (existing.has(ext)) continue
      const user = opts.usernamePrefix ? `${opts.usernamePrefix}${ext}` : ext
      const pass = opts.passwordMode === 'ext'    ? ext
                 : opts.passwordMode === 'custom' ? opts.customPassword.replaceAll('{ext}', ext)
                 : opts.samePassword
      const acc: SipAccount = {
        id: uid(), extension: ext, username: user,
        password: pass, domain: opts.domain, status: 'idle',
      }
      config.value.agent.accounts.push(acc)
      created.push(acc)
    }
    config.value.agent.count = config.value.agent.accounts.length
    const skipped = opts.count - created.length
    addLog('ok', `生成 ${created.length} 個座席帳號（分機 ${opts.startExt}–${opts.startExt + opts.count - 1}）${skipped > 0 ? `，略過 ${skipped} 個重複分機` : ''}`)
    // 立即註冊每個新建帳號
    registerMany(created)
  }

  // ── REGISTER：對單一帳號發 SIP REGISTER ────────────────────────────────
  interface RustRegisterResult {
    status: 'registered' | 'auth_failed' | 'rejected' | 'timeout' | 'network_error'
    message: string
    expires_secs: number | null
    sip_code: number | null
  }

  async function registerAccount(id: string) {
    const acc = config.value.agent.accounts.find(a => a.id === id)
    if (!acc) return
    if (!acc.username || !acc.password) {
      acc.status = 'failed'
      addLog('warn', `帳號 ${acc.extension}：缺少帳號或密碼，跳過註冊`)
      return
    }
    acc.status = 'registering'
    try {
      const result = await invoke<RustRegisterResult>('register_account', {
        server:    config.value.server,
        domain:    acc.domain || config.value.agent.defaultDomain || null,
        username:  acc.username,
        password:  acc.password,
        expires:   3600,
        transport: config.value.transport,
      })
      // result.status 來自 Rust（snake_case）
      if (result.status === 'registered') {
        acc.status = 'registered'
        addLog('ok', `[${acc.extension}] 註冊成功（expires=${result.expires_secs ?? 3600}s）`)
      } else if (result.status === 'auth_failed') {
        acc.status = 'failed'
        addLog('err', `[${acc.extension}] 認證失敗 — ${result.message}`)
      } else {
        acc.status = 'failed'
        addLog('err', `[${acc.extension}] 註冊失敗 — ${result.message}`)
      }
    } catch (e) {
      acc.status = 'failed'
      addLog('err', `[${acc.extension}] 註冊呼叫錯誤 — ${e}`)
    }
  }

  /// 依「註冊間隔」逐一開始註冊（不等前一個完成；間隔 0 = 全部同時）
  async function registerMany(accounts: SipAccount[]) {
    const gap = config.value.agent.registerInterval
    for (const [i, a] of accounts.entries()) {
      if (i > 0 && gap > 0) await new Promise(r => setTimeout(r, gap))
      registerAccount(a.id)
    }
  }

  /// 重新註冊所有 idle/failed 帳號（手動觸發用）
  async function registerAll() {
    await registerMany(config.value.agent.accounts.filter(a => a.status !== 'registered'))
  }

  function removeAccount(id: string) {
    config.value.agent.accounts = config.value.agent.accounts.filter(a => a.id !== id)
    config.value.agent.count = config.value.agent.accounts.length
  }

  function clearAccounts() {
    config.value.agent.accounts = []
    config.value.agent.count = 0
  }

  function updateAccount(id: string, patch: Partial<SipAccount>) {
    const acc = config.value.agent.accounts.find(a => a.id === id)
    if (acc) Object.assign(acc, patch)
  }

  function parseAccountText(text: string): SipAccount[] {
    accountImportError.value = ''
    const domain = config.value.agent.defaultDomain
    const errors: string[] = []
    const result: SipAccount[] = []
    let firstContent = true

    text.split(/\r?\n/).forEach((rawLine, idx) => {
      const line   = rawLine.trim()
      const lineNo = idx + 1
      if (!line || line.startsWith('#')) return
      const isFirst = firstContent
      firstContent = false
      if (isFirst && /^(extension|ext|username|user)\b/i.test(line)) return

      if (line.includes(',')) {
        const p = line.split(',').map(s => s.trim())
        if (p.length < 3) { errors.push(`第 ${lineNo} 行：需為 extension,username,password`); return }
        result.push({ id:uid(), extension:p[0], username:p[1], password:p[2], domain:p[3]||domain, status:'idle' })
      } else if (line.includes(':')) {
        const p = line.split(':')
        result.push({ id:uid(), extension:p[0].trim(), username:p[0].trim(), password:p[1]?.trim()??'', domain:p[2]?.trim()||domain, status:'idle' })
      } else {
        const p = line.split(/\s+/)
        if (p.length < 3) { errors.push(`第 ${lineNo} 行：無法辨識格式`); return }
        result.push({ id:uid(), extension:p[0], username:p[1], password:p[2], domain:p[3]||domain, status:'idle' })
      }
    })

    if (errors.length > 0) {
      accountImportError.value = errors.length > 3
        ? `${errors.slice(0, 3).join('；')}…（共 ${errors.length} 行錯誤）`
        : errors.join('；')
    }
    return result
  }

  function importAccountText(text: string) {
    const parsed = parseAccountText(text)
    const existing = new Set(config.value.agent.accounts.map(a => a.extension))
    const fresh: SipAccount[] = []
    for (const a of parsed) {
      if (existing.has(a.extension)) continue
      existing.add(a.extension)
      fresh.push(a)
    }
    const dup = parsed.length - fresh.length
    if (fresh.length > 0) {
      config.value.agent.accounts.push(...fresh)
      config.value.agent.count = config.value.agent.accounts.length
      // 匯入後立即註冊
      registerMany(fresh)
    }
    if (parsed.length > 0)
      addLog('ok', `匯入 ${fresh.length} 個 SIP 帳號${dup > 0 ? `（略過 ${dup} 個重複分機）` : ''}`)
  }

  // ── Export ────────────────────────────────────────────────────────────────────
  /** 開啟另存新檔對話框並寫入；回傳實際路徑（取消則為 null） */
  async function saveTextFile(defaultName: string, ext: string, label: string, contents: string): Promise<string | null> {
    const path = await save({ defaultPath: defaultName, filters: [{ name: label, extensions: [ext] }] })
    if (!path) return null
    await invoke('save_text_file', { path, contents })
    return path
  }

  /** 匯出座席流程（CLI --agent-flow 使用的 JSON） */
  function exportFlowJson() {
    return exportWith('座席流程', () =>
      saveTextFile('sipress_flow.json', 'json', 'JSON', JSON.stringify(flowToRust(config.value.agent.flow), null, 2) + '\n'))
  }

  /** 手動撥碼：座席端執行中，所有已註冊座席依註冊間隔逐一撥出常用碼 */
  async function runAgentCode(key: FlowCodeKey) {
    const flow = config.value.agent.flow
    const code = flow.codes[key].trim()
    const name = FLOW_CODE_NAMES[key]
    const errs = flowErrors({ ...flow, onStart: [newFlowStep('dial', code, name)], afterCall: [], onStop: [] })
    if (errs.length > 0) {
      for (const e of errs) addLog('err', `${name}：${e}`, 'agent')
      return
    }
    try {
      await invoke('agent_run_steps', { steps: [{ type: 'dial', code, label: name }] })
      addLog('info', `手動撥碼「${name}」（${code}）已送出，已註冊的座席依註冊間隔逐一撥出`, 'agent')
    } catch (e) {
      addLog('err', `手動撥碼「${name}」失敗：${e}`, 'agent')
    }
  }

  async function exportWith(label: string, fn: () => Promise<string | null>) {
    try {
      const path = await fn()
      if (path) addLog('ok', `${label} 已匯出 → ${path}`)
    } catch (e) {
      addLog('err', `${label} 匯出失敗: ${e}`)
    }
  }

  function exportJson() {
    return exportWith('JSON 報告', () => {
      const mode = config.value.mode
      const r    = view.value
      const body = JSON.stringify({ mode, target:config.value.server, elapsed:r.elapsedSec, config:config.value, metrics:r.metrics, callStates:r.callStates, respCodes:r.respCodes, rtp:r.rtpMetrics, series:r.series }, null, 2)
      return saveTextFile(`sipress_${mode}_${fileTs()}.json`, 'json', 'JSON', body)
    })
  }

  function exportCsv() {
    return exportWith('CSV 報告', () => {
      const s = view.value.series
      const rows = [['sec','cps','conc','asr','ccr','fail']]
      for (let i = 0; i < s.cps.length; i++)
        rows.push([String(view.value.seriesOffset.cps + i), s.cps[i]?.toFixed(2) ?? '0', String(s.conc[i] ?? 0), s.asr[i]?.toFixed(2) ?? '0', s.ccr[i]?.toFixed(2) ?? '0', String(s.fail[i] ?? 0)])
      return saveTextFile(`sipress_${config.value.mode}_${fileTs()}.csv`, 'csv', 'CSV', rows.map(r => r.join(',')).join('\n'))
    })
  }

  function exportAccountsCsv() {
    return exportWith('帳號清單', () => {
      const rows = [['extension','username','password','domain'], ...config.value.agent.accounts.map(a => [a.extension, a.username, a.password, a.domain])]
      return saveTextFile('sipress_accounts.csv', 'csv', 'CSV', rows.map(r => r.join(',')).join('\n'))
    })
  }

  /** 端到端對照：民眾端完成、座席端執行過（可仍在執行）時可匯出 */
  const canExportE2e = computed(() =>
    sessions.value.caller.status === 'done' && sessions.value.agent.status !== 'idle')

  function exportE2eHtml() {
    return exportWith('端到端對照報告', async () => {
      const ts   = fileTs()
      const html = await invoke<string>('get_correlation_html', { serverAddr: config.value.server, timestamp: ts })
      return saveTextFile(`sipress_e2e_${ts}.html`, 'html', 'HTML', html)
    })
  }

  function exportHtml() {
    return exportWith('HTML 報告', async () => {
      const ts   = fileTs()
      const mode = config.value.mode
      const html = await invoke<string>('get_html_report', { mode, serverAddr: config.value.server, timestamp: ts })
      return saveTextFile(`sipress_${mode}_${ts}.html`, 'html', 'HTML', html)
    })
  }

  return {
    config, sessions, status, isRunning, anyRunning, stopping, elapsedSec, metrics, series, seriesOffset, seriesTick,
    callStates, respCodes, logs, agents,
    rtpMetrics, mosRating, codecLabel,
    accountImportError, registeredCount,
    validationErrors, bothValidationErrors, cliCommand, progressPct,
    addLog, clearLog,
    generateAccounts, removeAccount, clearAccounts, updateAccount,
    importAccountText, parseAccountText,
    registerAccount, registerAll,
    startTest, stopTest, startBoth, stopAll,
    exportJson, exportCsv, exportHtml, exportAccountsCsv, exportE2eHtml, canExportE2e,
    exportFlowJson, runAgentCode,
  }
})
