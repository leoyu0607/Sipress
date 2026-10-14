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

export type CpsPatternType = 'flat' | 'ramp' | 'step' | 'spike'

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
}

export interface CallerProfile {
  accessNumber:  string
  calleeFixed:   string   // '' = 用 prefix+range 隨機產生
  calleePrefix:  string
  calleeRange:   number
  concurrency:   number
  cps:           number
  cpsPattern:    CpsPatternConfig
  totalCalls:    number   // 0 = unlimited
  callDuration:  number   // per-call duration seconds (0 = unlimited)
  enableAudio:   boolean
  audioFile:     string
  dtmfSequence:  string
  codec:         'pcma' | 'pcmu'
}

export interface AgentProfile {
  count: number
  accounts: SipAccount[]
  defaultDomain: string
  ackTimeout: number
  byeTimeout: number
}

export interface TestConfig {
  server: string
  transport: 'UDP' | 'TCP'
  localPort: number
  duration: number
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
}

export type TestStatus = 'idle' | 'running' | 'done' | 'error'

export type SeriesKey = 'cps' | 'conc' | 'asr' | 'ccr' | 'pdd' | 'fail' | 'mos'

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
  // 目前為止的平均 PDD（null = 尚無樣本）
  pdd_avg_ms:        number | null
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
}

// ── Constants ─────────────────────────────────────────────────────────────────
export const MAX_SERIES  = 90
const MAX_LOG     = 300
const STORAGE_KEY = 'sipress-config'

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

/** MOS 分級（全 App 共用）：≥4.0 優 / ≥3.0 普通 / 其餘 差 */
export function mosGrade(m: number): { key: 'excellent' | 'fair' | 'poor' | 'none'; label: string; color: string } {
  if (m >= 4.0) return { key: 'excellent', label: '優',   color: 'var(--accent)' }
  if (m >= 3.0) return { key: 'fair',      label: '普通', color: 'var(--warn)' }
  if (m > 0)    return { key: 'poor',      label: '差',   color: 'var(--danger)' }
  return { key: 'none', label: '—', color: 'var(--text2)' }
}

function defaultConfig(): TestConfig {
  return {
    server: '192.168.166.60:5066',
    transport: 'UDP',
    localPort: 5070,
    duration: 60,
    cliFormat: 'tui',
    cliVerbose: false,
    mode: 'caller',
    caller: {
      accessNumber: '0938530031',
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
      },
      totalCalls:   0,
      callDuration: 30,
      enableAudio:  false,
      audioFile:    '',
      dtmfSequence: '',
      codec:        'pcma',
    },
    agent: {
      count: 0,
      accounts: [],
      defaultDomain: '192.168.166.60',
      ackTimeout: 10,
      byeTimeout: 150,
    },
  }
}

/** 從 localStorage 載入設定，與預設值合併（新版本新增的欄位會取預設值） */
function loadConfig(): TestConfig {
  const def = defaultConfig()
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return def
    const saved = JSON.parse(raw) as Partial<TestConfig>
    const cfg: TestConfig = {
      ...def,
      ...saved,
      caller: {
        ...def.caller,
        ...saved.caller,
        cpsPattern: { ...def.caller.cpsPattern, ...saved.caller?.cpsPattern },
      },
      agent: { ...def.agent, ...saved.agent },
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
export const useTestStore = defineStore('test', () => {

  const config = ref<TestConfig>(loadConfig())

  watch(config, c => {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(c)) } catch { /* quota / private mode */ }
  }, { deep: true })

  const status      = ref<TestStatus>('idle')
  const elapsedSec  = ref(0)
  const metrics     = ref<Metrics>({
    cps: 0, concurrency: 0, succeeded: 0, failed: 0, initiated: 0,
    asr: 0, ccr: 0, errorRate: 0, pdd: 0, acd: 0,
  })
  const series      = ref<Record<SeriesKey, number[]>>({
    cps: [], conc: [], asr: [], ccr: [], pdd: [], fail: [], mos: [],
  })
  /** 每個系列已被捨棄（shift 掉）的筆數，用於 X 軸時間標籤 */
  const seriesOffset = ref<Record<SeriesKey, number>>({
    cps: 0, conc: 0, asr: 0, ccr: 0, pdd: 0, fail: 0, mos: 0,
  })
  /** 每次系列資料變動 +1；圖表監聽此值重繪（長度到上限後不再變化） */
  const seriesTick  = ref(0)
  const callStates  = ref<CallStates>({ initiated:0, answered:0, completed:0, failed:0, timeout:0 })
  const respCodes   = ref<Record<string, number>>({})
  const logs        = ref<LogEntry[]>([])
  const rtpMetrics  = ref<RtpMetrics>({ enabled:false, mos:0, packetLoss:0, jitter:0, packetsSent:0, packetsRecv:0, outOfOrder:0 })
  const accountImportError = ref('')
  /** 已送出停止、等待引擎收尾中（BYE/CANCEL）；防止重複停止 */
  const stopping    = ref(false)

  let pollTimer:  ReturnType<typeof setInterval> | null = null
  let clockTimer: ReturnType<typeof setInterval> | null = null
  let prevInitiated = 0
  let rtpLoggedOnce = false   // 防止重複寫 RTP session 啟動 log
  let logSeq = 0

  // ── Computed ──────────────────────────────────────────────────────────────────
  const isRunning = computed(() => status.value === 'running')

  const mosRating = computed(() => mosGrade(rtpMetrics.value.mos).key)

  const codecLabel = computed(() => CODEC_LABEL[config.value.caller.codec] ?? CODEC_LABEL.pcma)

  const registeredCount = computed(() =>
    config.value.agent.accounts.filter(a => a.status === 'registered').length
  )

  /** 啟動前驗證；非空代表無法開始測試 */
  const validationErrors = computed(() => {
    const c = config.value
    const errs: string[] = []
    if (!c.server.trim()) errs.push('SIP server 不可空白')
    else if (!/^[^\s:]+:\d{1,5}$/.test(c.server.trim())) errs.push('SIP server 格式需為 host:port')
    if (c.mode === 'caller') {
      const p = c.caller
      if (!p.accessNumber.trim()) errs.push('主叫號碼不可空白')
      if (!(p.concurrency >= 1)) errs.push('併發數量需 ≥ 1')
      if (p.cpsPattern.type === 'flat' && !(p.cps > 0)) errs.push('CPS 需 > 0')
      if (p.cpsPattern.type === 'step' && p.cpsPattern.steps.length === 0) errs.push('階梯模式至少需要一個階層')
      if ((p.cpsPattern.type === 'ramp' || p.cpsPattern.type === 'spike') && c.duration === 0)
        errs.push(`${p.cpsPattern.type === 'ramp' ? '漸增' : '尖峰'}模式需設定持續時間（> 0）`)
      if (p.enableAudio && !p.audioFile.trim()) errs.push('已啟用播放音檔但尚未選擇檔案')
    } else if (c.agent.accounts.length === 0) {
      errs.push('座席模式需要至少一個帳號')
    }
    return errs
  })

  /** 目前設定中 CLI 不支援、因此不會出現在 CLI 預覽中的項目 */
  const cliUnsupported = computed(() => {
    const c = config.value
    if (c.mode === 'agent') return ['座席模式']
    const out: string[] = []
    if (c.caller.cpsPattern.type !== 'flat') out.push('CPS 模式')
    if (c.caller.enableAudio && c.caller.codec !== 'pcma') out.push('Codec')
    if (c.caller.dtmfSequence) out.push('DTMF')
    return out
  })

  const cliCommand = computed(() => {
    const c = config.value
    if (c.mode === 'agent') return '# 座席模式目前僅 GUI 支援，CLI 無對應指令'
    const p      = c.caller
    const parts  = [`./sipress -s ${c.server}`, `--from ${p.accessNumber}`]
    parts.push(p.calleeFixed ? `--to ${p.calleeFixed}` : `--to-prefix ${p.calleePrefix} --to-range ${p.calleeRange}`)
    parts.push(`-c ${p.concurrency}`, `--cps ${p.cps}`, `-d ${c.duration}`, `--call-duration ${p.callDuration}`)
    if (p.totalCalls > 0) parts.push(`--max-calls ${p.totalCalls}`)
    parts.push(`--transport ${c.transport.toLowerCase()}`)
    if (c.localPort) parts.push(`--local 0.0.0.0:${c.localPort}`)
    if (p.enableAudio && p.audioFile) parts.push(`--rtp --rtp-port 16000 --audio "${p.audioFile}"`)
    parts.push(c.cliFormat === 'tui' ? '--tui' : `--format ${c.cliFormat}`)
    if (c.cliVerbose) parts.push('--verbose')
    return parts.join(' ')
  })

  const progressPct = computed(() =>
    status.value === 'running' && config.value.duration > 0
      ? Math.min(100, (elapsedSec.value / config.value.duration) * 100)
      : 0  // duration=0 means unlimited → no progress bar
  )

  // ── Log ───────────────────────────────────────────────────────────────────────
  function addLog(level: LogEntry['level'], msg: string) {
    logs.value.push({ id: ++logSeq, ts: nowTs(), level, msg })
    if (logs.value.length > MAX_LOG) logs.value.shift()
  }
  function clearLog() { logs.value = [] }

  // ── Series ────────────────────────────────────────────────────────────────────
  function pushSeries(key: SeriesKey, val: number) {
    series.value[key].push(val)
    if (series.value[key].length > MAX_SERIES) {
      series.value[key].shift()
      seriesOffset.value[key]++
    }
    seriesTick.value++
  }

  // ── CPS pattern builder ──────────────────────────────────────────────────────
  function buildCpsPattern(p: CpsPatternConfig) {
    switch (p.type) {
      case 'ramp':  return { type: 'ramp',  start: p.rampStart, end: p.rampEnd }
      case 'step':  return { type: 'step',  levels: p.steps }
      case 'spike': return { type: 'spike', base: p.spikeBase, peak: p.spikePeak, spike_at_pct: p.spikeAtPct, spike_duration_secs: p.spikeDurationSecs }
      default:      return { type: 'flat' }
    }
  }

  // ── Rust config builder ───────────────────────────────────────────────────────
  function buildRustConfig() {
    const c = config.value
    return {
      server_addr:          c.server.trim(),
      local_addr:           c.localPort ? `0.0.0.0:${c.localPort}` : null,
      local_domain:         null,
      caller_number:        c.caller.accessNumber,
      callee_prefix:        c.caller.calleePrefix || '2',
      callee_range:         c.caller.calleeRange  || 9999,
      callee_fixed:         c.caller.calleeFixed ? c.caller.calleeFixed : null,
      cps:                  c.caller.cps,
      cps_pattern:          buildCpsPattern(c.caller.cpsPattern),
      max_concurrent_calls: c.caller.concurrency,
      max_total_calls:      c.caller.totalCalls > 0 ? c.caller.totalCalls : null,
      duration_secs:        c.duration,
      call_duration_secs:   c.caller.callDuration,
      invite_timeout_secs:  8,
      transport:            c.transport.toLowerCase(),
      logs_dir:             'logs',
      rtp_base_port:        16000,
      audio_file:           c.caller.enableAudio && c.caller.audioFile ? c.caller.audioFile : null,
      codec:                c.caller.codec,
      enable_rtp:           c.caller.enableAudio && !!c.caller.audioFile,
      dtmf_sequence:        c.caller.dtmfSequence || null,
      mode:                 c.mode,                                          // 'caller' | 'agent'
      agent_accounts:       c.mode === 'agent'
        ? c.agent.accounts.map(a => ({
            extension: a.extension,
            username:  a.username,
            password:  a.password,
            domain:    a.domain || c.agent.defaultDomain,
          }))
        : [],
      agent_ack_timeout_secs: c.agent.ackTimeout,
      agent_bye_timeout_secs: c.agent.byeTimeout,
    }
  }

  // ── Snapshot → store ─────────────────────────────────────────────────────────
  function applySnapshot(snap: RustSnapshot) {
    const cps = Math.max(0, snap.calls_initiated - prevInitiated)
    prevInitiated = snap.calls_initiated
    const failed = snap.calls_failed + snap.calls_timeout

    metrics.value = {
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
      pdd:         snap.pdd_avg_ms ?? 0,
      acd:         0,
    }

    callStates.value = {
      initiated: snap.calls_initiated,
      answered:  snap.calls_answered,
      completed: snap.calls_completed,
      failed:    snap.calls_failed,
      timeout:   snap.calls_timeout,
    }

    pushSeries('cps',  cps)
    pushSeries('conc', snap.calls_concurrent)
    pushSeries('asr',  snap.asr)
    pushSeries('ccr',  metrics.value.ccr)
    pushSeries('fail', failed)
    if (snap.pdd_avg_ms != null) pushSeries('pdd', snap.pdd_avg_ms)

    // Approximate live respCodes from snapshot counters
    respCodes.value['200'] = snap.calls_answered
    respCodes.value['408'] = snap.calls_timeout

    if (failed > 0)
      addLog('warn', `失敗 ${snap.calls_failed} 逾時 ${snap.calls_timeout}  發起 ${snap.calls_initiated}  ASR ${snap.asr.toFixed(1)}%`)

    // RTP session 啟動後只寫一次 log
    if ((snap.rtp_sessions ?? 0) > 0 && !rtpLoggedOnce) {
      rtpLoggedOnce = true
      addLog('ok', `RTP ${codecLabel.value.short} 音訊傳送中（${snap.rtp_sessions} session${snap.rtp_sessions > 1 ? 's' : ''} active）`)
    }

    // 即時 RTP 品質更新
    if (snap.rtp_mos !== null && snap.rtp_mos !== undefined) {
      rtpMetrics.value = {
        enabled:     true,
        mos:         snap.rtp_mos,
        packetLoss:  snap.rtp_loss_pct ?? 0,
        jitter:      snap.rtp_jitter_ms ?? 0,
        packetsSent: snap.rtp_sent_packets ?? 0,
        packetsRecv: snap.rtp_recv_packets ?? 0,
        outOfOrder:  0,
      }
      pushSeries('mos', snap.rtp_mos)
    }
  }

  /** 取得最終報告；引擎執行失敗時回傳 false（get_report 會丟出錯誤原因） */
  async function _tryFetchReport(): Promise<boolean> {
    try {
      const report = await invoke<RustReport | null>('get_report')
      if (!report) return true
      metrics.value = {
        ...metrics.value,
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
      callStates.value = {
        initiated: report.calls_initiated,
        answered:  report.calls_answered,
        completed: report.calls_completed,
        failed:    report.calls_failed,
        timeout:   report.calls_timeout,
      }
      if (report.mos !== null) {
        rtpMetrics.value = {
          enabled:     true,
          mos:         report.mos ?? 0,
          packetLoss:  report.loss_rate_pct ?? 0,
          jitter:      report.jitter_ms ?? 0,
          packetsSent: report.rtp_sent ?? 0,
          packetsRecv: report.rtp_recv ?? 0,
          outOfOrder:  report.rtp_out_of_order ?? 0,
        }
        pushSeries('mos', report.mos ?? 0)
      }
      // Populate respCodes from final per-code breakdown
      if (report.fail_codes) {
        for (const [code, count] of Object.entries(report.fail_codes)) {
          respCodes.value[code] = count
        }
      }
      // 200 and 408 come from summary counters (may not appear in fail_codes)
      respCodes.value['200'] = report.calls_answered
      respCodes.value['408'] = report.calls_timeout
      addLog('ok', `測試完成  發起 ${report.calls_initiated}  接通 ${report.calls_answered}  失敗 ${report.calls_failed + report.calls_timeout}  ASR ${report.asr.toFixed(1)}%  CPS ${report.actual_cps.toFixed(1)}  PDD p50 ${report.pdd_p50_ms.toFixed(0)}ms`)
      return true
    } catch (e) {
      addLog('err', `壓測執行失敗: ${e}`)
      return false
    }
  }

  function _finishTest() {
    if (pollTimer)  { clearInterval(pollTimer);  pollTimer  = null }
    if (clockTimer) { clearInterval(clockTimer); clockTimer = null }
    stopping.value = false
    _tryFetchReport().then(ok => { status.value = ok ? 'done' : 'error' })
  }

  // ── Start / Stop ──────────────────────────────────────────────────────────────
  async function startTest() {
    if (status.value === 'running') return
    if (validationErrors.value.length > 0) {
      for (const e of validationErrors.value) addLog('err', e)
      return
    }
    status.value = 'running'
    stopping.value = false
    elapsedSec.value = 0
    prevInitiated = 0
    rtpLoggedOnce = false
    metrics.value  = { cps:0, concurrency:0, succeeded:0, failed:0, initiated:0, asr:0, ccr:0, errorRate:0, pdd:0, acd:0 }
    series.value   = { cps:[], conc:[], asr:[], ccr:[], pdd:[], fail:[], mos:[] }
    seriesOffset.value = { cps:0, conc:0, asr:0, ccr:0, pdd:0, fail:0, mos:0 }
    seriesTick.value++
    respCodes.value = {}
    callStates.value = { initiated:0, answered:0, completed:0, failed:0, timeout:0 }
    rtpMetrics.value = { enabled:false, mos:0, packetLoss:0, jitter:0, packetsSent:0, packetsRecv:0, outOfOrder:0 }

    try {
      await invoke('start_test', { config: buildRustConfig() })
      addLog('ok',   `測試開始 → ${config.value.server}  [${config.value.mode === 'caller' ? '民眾端' : '座席端'}]`)
      if (config.value.mode === 'caller') {
        addLog('info', `持續時間: ${config.value.duration > 0 ? config.value.duration + 's' : '不限'}  cps: ${config.value.caller.cps}  併發: ${config.value.caller.concurrency}${config.value.caller.totalCalls > 0 ? `  總通數: ${config.value.caller.totalCalls}` : ''}`)
        if (config.value.caller.enableAudio && config.value.caller.audioFile) {
          const fname = config.value.caller.audioFile.replace(/\\/g, '/').split('/').pop() ?? config.value.caller.audioFile
          addLog('info', `RTP codec=${codecLabel.value.full}  audio="${fname}"  port=${buildRustConfig().rtp_base_port}`)
        } else {
          addLog('info', `RTP 未啟用（僅 SIP signaling）`)
        }
      } else {
        const total = config.value.agent.accounts.length
        addLog('info', `座席端：${total} 個帳號待註冊；持續時間: ${config.value.duration > 0 ? config.value.duration + 's' : '不限'}`)
        addLog('info', `行為：每個帳號 REGISTER → 等待 INVITE → 自動 200 OK → 等 BYE → 結束 deregister`)
      }
    } catch (e) {
      addLog('err', `啟動失敗: ${e}`)
      status.value = 'error'
      return
    }

    pollTimer = setInterval(async () => {
      try {
        const snap = await invoke<RustSnapshot | null>('get_snapshot')
        if (snap) {
          applySnapshot(snap)
          // 引擎已結束（duration 到 / max_total_calls 完成 / graceful stop）
          if (snap.finished) {
            _finishTest()
          }
        }
      } catch { /* ignore */ }
    }, 1000)

    clockTimer = setInterval(() => {
      elapsedSec.value++
    }, 1000)
  }

  async function stopTest() {
    if (stopping.value) return
    stopping.value = true
    if (clockTimer) { clearInterval(clockTimer); clockTimer = null }
    addLog('warn', `測試已手動停止  經過: ${elapsedSec.value}s`)
    try {
      await invoke('stop_test')
    } catch {
      // 引擎已結束（沒有可停止的壓測）→ 直接收尾取報告
      _finishTest()
      return
    }
    // 引擎會先對進行中的通話送 BYE/CANCEL 並等待回應，
    // 由輪詢偵測 snapshot.finished 後再取最終報告
    addLog('info', '等待進行中通話收尾（BYE/CANCEL）…')
    if (!pollTimer) _finishTest()
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
    for (const a of created) registerAccount(a.id)
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

  /// 重新註冊所有 idle/failed 帳號（手動觸發用）
  async function registerAll() {
    for (const a of config.value.agent.accounts) {
      if (a.status !== 'registered') {
        // 並行：每個帳號獨立 socket，互不干擾
        registerAccount(a.id)
      }
    }
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
      for (const a of fresh) registerAccount(a.id)
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
      const body = JSON.stringify({ mode:config.value.mode, target:config.value.server, elapsed:elapsedSec.value, config:config.value, metrics:metrics.value, callStates:callStates.value, respCodes:respCodes.value, rtp:rtpMetrics.value, series:series.value }, null, 2)
      return saveTextFile(`sipress_${fileTs()}.json`, 'json', 'JSON', body)
    })
  }

  function exportCsv() {
    return exportWith('CSV 報告', () => {
      const s = series.value
      const rows = [['sec','cps','conc','asr','ccr','fail']]
      for (let i = 0; i < s.cps.length; i++)
        rows.push([String(seriesOffset.value.cps + i), s.cps[i]?.toFixed(2) ?? '0', String(s.conc[i] ?? 0), s.asr[i]?.toFixed(2) ?? '0', s.ccr[i]?.toFixed(2) ?? '0', String(s.fail[i] ?? 0)])
      return saveTextFile(`sipress_${fileTs()}.csv`, 'csv', 'CSV', rows.map(r => r.join(',')).join('\n'))
    })
  }

  function exportAccountsCsv() {
    return exportWith('帳號清單', () => {
      const rows = [['extension','username','password','domain'], ...config.value.agent.accounts.map(a => [a.extension, a.username, a.password, a.domain])]
      return saveTextFile('sipress_accounts.csv', 'csv', 'CSV', rows.map(r => r.join(',')).join('\n'))
    })
  }

  function exportHtml() {
    return exportWith('HTML 報告', async () => {
      const ts   = fileTs()
      const html = await invoke<string>('get_html_report', { serverAddr: config.value.server, timestamp: ts })
      return saveTextFile(`sipress_${ts}.html`, 'html', 'HTML', html)
    })
  }

  return {
    config, status, isRunning, stopping, elapsedSec, metrics, series, seriesOffset, seriesTick,
    callStates, respCodes, logs,
    rtpMetrics, mosRating, codecLabel,
    accountImportError, registeredCount,
    validationErrors, cliUnsupported, cliCommand, progressPct,
    addLog, clearLog,
    generateAccounts, removeAccount, clearAccounts, updateAccount,
    importAccountText, parseAccountText,
    registerAccount, registerAll,
    startTest, stopTest,
    exportJson, exportCsv, exportHtml, exportAccountsCsv,
  }
})
