import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'

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

export interface CallerProfile {
  accessNumber:  string
  calleeFixed:   string   // '' = 用 prefix+range 隨機產生
  calleePrefix:  string
  calleeRange:   number
  concurrency:   number
  cps:           number
  totalCalls:    number   // 0 = unlimited
  callDuration:  number   // per-call duration seconds (0 = unlimited)
  enableAudio:   boolean
  audioFile:     string
}

export interface AgentProfile {
  count: number
  accounts: SipAccount[]
  defaultDomain: string
}

export interface TestConfig {
  server: string
  transport: 'UDP' | 'TCP' | 'TLS'
  localPort: number
  duration: number
  outputFormat: 'tui' | 'json' | 'csv' | 'table'
  verbose: boolean
  mode: CallMode
  caller: CallerProfile
  agent: AgentProfile
}

export interface Metrics {
  cps: number
  concurrency: number
  succeeded: number
  failed: number
  queued: number
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
  invite: number
  trying: number
  ringing: number
  ok: number
  ack: number
  bye: number
  error: number
}

export interface LogEntry {
  ts: string
  level: 'ok' | 'info' | 'warn' | 'err' | 'dbg'
  msg: string
}

export type TestStatus = 'idle' | 'running' | 'done' | 'error'

// Rust StatsSnapshot shape
interface RustSnapshot {
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
  // 引擎是否已結束
  finished:          boolean
}

// Rust FinalReport shape (subset we use)
interface RustReport {
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
const MAX_SERIES = 90
const MAX_LOG    = 300

// ── Helpers ───────────────────────────────────────────────────────────────────
function uid()    { return Math.random().toString(36).slice(2, 8) }

function nowTs() {
  const d = new Date()
  return [d.getHours(), d.getMinutes(), d.getSeconds()]
    .map(n => String(n).padStart(2, '0')).join(':')
    + '.' + String(d.getMilliseconds()).padStart(3, '0')
}

// ── Store ─────────────────────────────────────────────────────────────────────
export const useTestStore = defineStore('test', () => {

  const config = ref<TestConfig>({
    server: '192.168.166.60:5066',
    transport: 'UDP',
    localPort: 5070,
    duration: 60,
    outputFormat: 'tui',
    verbose: false,
    mode: 'caller',
    caller: {
      accessNumber: '0938530031',
      calleeFixed:  '',
      calleePrefix: '2',
      calleeRange:  9999,
      concurrency:  100,
      cps:          10,
      totalCalls:   0,
      callDuration: 30,
      enableAudio:  false,
      audioFile:    '',
    },
    agent: {
      count: 0,
      accounts: [],
      defaultDomain: '192.168.166.60',
    },
  })

  const status      = ref<TestStatus>('idle')
  const elapsedSec  = ref(0)
  const metrics     = ref<Metrics>({
    cps: 0, concurrency: 0, succeeded: 0, failed: 0, queued: 0,
    asr: 0, ccr: 0, errorRate: 0, pdd: 0, acd: 0,
  })
  const series      = ref({
    cps:  [] as number[],
    conc: [] as number[],
    asr:  [] as number[],
    ccr:  [] as number[],
    pdd:  [] as number[],
    fail: [] as number[],
    mos:  [] as number[],
  })
  const callStates  = ref<CallStates>({ invite:0, trying:0, ringing:0, ok:0, ack:0, bye:0, error:0 })
  const respCodes   = ref<Record<string, number>>({ '100':0,'180':0,'200':0,'486':0,'404':0,'503':0,'408':0 })
  const logs        = ref<LogEntry[]>([])
  const flowTimes   = ref({ invite:'—', trying:'—', ringing:'—', ok:'—', ack:'—', bye:'—', done:'—' })
  const rtpMetrics  = ref<RtpMetrics>({ enabled:false, mos:0, packetLoss:0, jitter:0, packetsSent:0, packetsRecv:0, outOfOrder:0 })
  const accountImportError = ref('')

  let pollTimer:  ReturnType<typeof setInterval> | null = null
  let clockTimer: ReturnType<typeof setInterval> | null = null
  let stopping = false        // 已送出停止、等待引擎收尾中（防止重複停止）
  let prevInitiated = 0
  let rtpLoggedOnce = false   // 防止重複寫 RTP session 啟動 log

  // ── Computed ──────────────────────────────────────────────────────────────────
  const mosRating = computed(() => {
    const m = rtpMetrics.value.mos
    if (m >= 4.0) return 'excellent'
    if (m >= 3.0) return 'good'
    if (m >= 2.5) return 'fair'
    return 'poor'
  })

  const activeConcurrency = computed(() =>
    config.value.mode === 'caller'
      ? config.value.caller.concurrency
      : config.value.agent.accounts.length
  )

  const activeCps = computed(() =>
    config.value.mode === 'caller' ? config.value.caller.cps : 1
  )

  const registeredCount = computed(() =>
    config.value.agent.accounts.filter(a => a.status === 'registered').length
  )

  const cliCommand = computed(() => {
    const c = config.value
    const tr = c.transport.toLowerCase()
    if (c.mode === 'caller') {
      const audio    = c.caller.enableAudio && c.caller.audioFile ? ` --rtp --audio "${c.caller.audioFile}"` : ''
      const total    = c.caller.totalCalls > 0 ? ` --max-calls ${c.caller.totalCalls}` : ''
      const callDur  = c.caller.callDuration > 0 ? ` --call-duration ${c.caller.callDuration}` : ''
      const callee   = c.caller.calleeFixed
        ? ` --to ${c.caller.calleeFixed}`
        : ` --to-prefix ${c.caller.calleePrefix} --to-range ${c.caller.calleeRange}`
      return `./sipress -s ${c.server} --mode caller --number ${c.caller.accessNumber}${callee} -c ${c.caller.concurrency} --cps ${c.caller.cps} --duration ${c.duration} --transport ${tr}${total}${callDur}${audio}`
    } else {
      const n = c.agent.accounts.length
      return `./sipress -s ${c.server} --mode agent --accounts accounts.csv (${n} 個帳號) --duration ${c.duration} --transport ${tr}`
    }
  })

  const progressPct = computed(() =>
    status.value === 'running' && config.value.duration > 0
      ? Math.min(100, (elapsedSec.value / config.value.duration) * 100)
      : 0  // duration=0 means unlimited → no progress bar
  )

  // ── Log ───────────────────────────────────────────────────────────────────────
  function addLog(level: LogEntry['level'], msg: string) {
    logs.value.push({ ts: nowTs(), level, msg })
    if (logs.value.length > MAX_LOG) logs.value.shift()
  }
  function clearLog() { logs.value = [] }

  // ── Series ────────────────────────────────────────────────────────────────────
  function pushSeries(key: keyof typeof series.value, val: number) {
    series.value[key].push(val)
    if (series.value[key].length > MAX_SERIES) series.value[key].shift()
  }

  // ── Rust config builder ───────────────────────────────────────────────────────
  function buildRustConfig() {
    const c = config.value
    const transportMap: Record<string, string> = { UDP: 'udp', TCP: 'tcp', TLS: 'tcp' }
    return {
      server_addr:          c.server,
      local_addr:           c.localPort ? `0.0.0.0:${c.localPort}` : null,
      local_domain:         null,
      caller_number:        c.caller.accessNumber,
      callee_prefix:        c.caller.calleePrefix || '2',
      callee_range:         c.caller.calleeRange  || 9999,
      callee_fixed:         c.caller.calleeFixed ? c.caller.calleeFixed : null,
      cps:                  c.caller.cps,
      max_concurrent_calls: c.caller.concurrency,
      max_total_calls:      c.caller.totalCalls > 0 ? c.caller.totalCalls : null,
      duration_secs:        c.duration,
      call_duration_secs:   c.caller.callDuration,
      invite_timeout_secs:  8,
      transport:            transportMap[c.transport] ?? 'udp',
      logs_dir:             'logs',
      rtp_base_port:        16000,
      audio_file:           c.caller.enableAudio && c.caller.audioFile ? c.caller.audioFile : null,
      enable_rtp:           c.caller.enableAudio && !!c.caller.audioFile,
      mode:                 c.mode,                                          // 'caller' | 'agent'
      agent_accounts:       c.mode === 'agent'
        ? c.agent.accounts.map(a => ({
            extension: a.extension,
            username:  a.username,
            password:  a.password,
            domain:    a.domain || c.agent.defaultDomain,
          }))
        : [],
    }
  }

  // ── Snapshot → store ─────────────────────────────────────────────────────────
  function applySnapshot(snap: RustSnapshot) {
    const cps = Math.max(0, snap.calls_initiated - prevInitiated)
    prevInitiated = snap.calls_initiated

    metrics.value = {
      cps,
      concurrency: snap.calls_concurrent,
      succeeded:   snap.calls_answered,
      failed:      snap.calls_failed + snap.calls_timeout,
      queued:      snap.calls_concurrent,
      asr:         snap.asr,
      ccr:         snap.calls_initiated > 0
        ? snap.calls_completed / snap.calls_initiated * 100
        : 0,
      errorRate:   snap.error_rate,
      pdd:         0,
      acd:         0,
    }

    pushSeries('cps',  cps)
    pushSeries('conc', snap.calls_concurrent)
    pushSeries('asr',  snap.asr)
    pushSeries('ccr',  metrics.value.ccr)
    pushSeries('fail', snap.calls_failed + snap.calls_timeout)

    // Approximate live respCodes from snapshot counters
    respCodes.value['200'] = snap.calls_answered
    respCodes.value['408'] = snap.calls_timeout

    if (snap.calls_failed + snap.calls_timeout > 0)
      addLog('warn', `失敗 ${snap.calls_failed} 逾時 ${snap.calls_timeout}  發起 ${snap.calls_initiated}  ASR ${snap.asr.toFixed(1)}%`)

    // RTP session 啟動後只寫一次 log
    if ((snap.rtp_sessions ?? 0) > 0 && !rtpLoggedOnce) {
      rtpLoggedOnce = true
      addLog('ok', `RTP G.711A 音訊傳送中（${snap.rtp_sessions} session${snap.rtp_sessions > 1 ? 's' : ''} active）`)
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
        asr:       report.asr,
        ccr:       report.ccr,
        errorRate: report.calls_initiated > 0
          ? (report.calls_failed + report.calls_timeout) / report.calls_initiated * 100
          : 0,
        pdd:       report.pdd_p50_ms,
        acd:       report.acd_secs,
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
      addLog('ok', `測試完成  發起 ${report.calls_initiated}  接通 ${report.calls_answered}  失敗 ${report.calls_failed + report.calls_timeout}  ASR ${report.asr.toFixed(1)}%  CPS ${report.actual_cps.toFixed(1)}`)
      return true
    } catch (e) {
      addLog('err', `壓測執行失敗: ${e}`)
      return false
    }
  }

  function _finishTest() {
    if (pollTimer)  { clearInterval(pollTimer);  pollTimer  = null }
    if (clockTimer) { clearInterval(clockTimer); clockTimer = null }
    stopping = false
    _tryFetchReport().then(ok => { status.value = ok ? 'done' : 'error' })
  }

  // ── Start / Stop ──────────────────────────────────────────────────────────────
  async function startTest() {
    if (status.value === 'running') return
    status.value = 'running'
    stopping = false
    elapsedSec.value = 0
    prevInitiated = 0
    rtpLoggedOnce = false
    metrics.value  = { cps:0, concurrency:0, succeeded:0, failed:0, queued:0, asr:0, ccr:0, errorRate:0, pdd:0, acd:0 }
    series.value   = { cps:[], conc:[], asr:[], ccr:[], pdd:[], fail:[], mos:[] }
    respCodes.value = { '100':0,'180':0,'200':0,'486':0,'404':0,'503':0,'408':0 }
    callStates.value = { invite:0, trying:0, ringing:0, ok:0, ack:0, bye:0, error:0 }
    rtpMetrics.value = { enabled:false, mos:0, packetLoss:0, jitter:0, packetsSent:0, packetsRecv:0, outOfOrder:0 }

    try {
      // 座席模式檢查：必須有帳號
      if (config.value.mode === 'agent' && config.value.agent.accounts.length === 0) {
        addLog('err', '座席模式需要至少一個帳號，請先新增')
        status.value = 'error'
        return
      }

      await invoke('start_test', { config: buildRustConfig() })
      addLog('ok',   `test started → ${config.value.server}  [${config.value.mode} mode]`)
      if (config.value.mode === 'caller') {
        addLog('info', `duration: ${config.value.duration > 0 ? config.value.duration + 's' : '不限'}  cps: ${config.value.caller.cps}  concur: ${config.value.caller.concurrency}${config.value.caller.totalCalls > 0 ? `  max-calls: ${config.value.caller.totalCalls}` : ''}`)
        // RTP 設定狀態
        if (config.value.caller.enableAudio && config.value.caller.audioFile) {
          const fname = config.value.caller.audioFile.replace(/\\/g, '/').split('/').pop() ?? config.value.caller.audioFile
          addLog('info', `RTP codec=G.711A(PCMA)  audio="${fname}"  port=${buildRustConfig().rtp_base_port}`)
        } else if (config.value.caller.enableAudio) {
          addLog('warn', `RTP 已啟用但未選音檔，將送靜音 (0xD5)`)
        } else {
          addLog('info', `RTP 未啟用（僅 SIP signaling）`)
        }
      } else {
        // agent mode
        const total = config.value.agent.accounts.length
        addLog('info', `座席端：${total} 個帳號待註冊；duration: ${config.value.duration > 0 ? config.value.duration + 's' : '不限'}`)
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
    if (stopping) return
    stopping = true
    if (clockTimer) { clearInterval(clockTimer); clockTimer = null }
    addLog('warn', `test stopped  elapsed: ${elapsedSec.value}s`)
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
    const created: SipAccount[] = []
    for (let i = 0; i < opts.count; i++) {
      const ext  = String(opts.startExt + i)
      const user = opts.usernamePrefix ? `${opts.usernamePrefix}${ext}` : ext
      const pass = opts.passwordMode === 'ext'    ? ext
                 : opts.passwordMode === 'custom' ? opts.customPassword.replace('{ext}', ext)
                 : opts.samePassword
      const acc: SipAccount = {
        id: uid(), extension: ext, username: user,
        password: pass, domain: opts.domain, status: 'idle',
      }
      config.value.agent.accounts.push(acc)
      created.push(acc)
    }
    config.value.agent.count = config.value.agent.accounts.length
    addLog('ok', `生成 ${opts.count} 個座席帳號（分機 ${opts.startExt}–${opts.startExt + opts.count - 1}）`)
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
    const lines  = text.split('\n').map(l => l.trim()).filter(l => l && !l.startsWith('#'))
    const result: SipAccount[] = []

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i]
      if (i === 0 && /^(extension|ext|username|user)/i.test(line)) continue

      if (line.includes(',')) {
        const p = line.split(',').map(s => s.trim())
        if (p.length < 3) { accountImportError.value = `Line ${i+1}: need extension,username,password`; continue }
        result.push({ id:uid(), extension:p[0], username:p[1], password:p[2], domain:p[3]||domain, status:'idle' })
      } else if (line.includes(':')) {
        const p = line.split(':')
        result.push({ id:uid(), extension:p[0].trim(), username:p[0].trim(), password:p[1]?.trim()??'', domain:p[2]?.trim()||domain, status:'idle' })
      } else {
        const p = line.split(/\s+/)
        if (p.length < 3) { accountImportError.value = `Line ${i+1}: unrecognized format`; continue }
        result.push({ id:uid(), extension:p[0], username:p[1], password:p[2], domain:p[3]||domain, status:'idle' })
      }
    }
    return result
  }

  function importAccountText(text: string) {
    const parsed = parseAccountText(text)
    if (parsed.length > 0) {
      config.value.agent.accounts.push(...parsed)
      config.value.agent.count = config.value.agent.accounts.length
      addLog('ok', `imported ${parsed.length} SIP accounts`)
      // 匯入後立即註冊
      for (const a of parsed) registerAccount(a.id)
    }
  }

  // ── Export ────────────────────────────────────────────────────────────────────
  function exportJson() {
    const blob = new Blob([JSON.stringify({ mode:config.value.mode, target:config.value.server, elapsed:elapsedSec.value, config:config.value, metrics:metrics.value, respCodes:respCodes.value, series:series.value }, null, 2)], { type:'application/json' })
    const a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = `sipress_${Date.now()}.json`; a.click()
    addLog('ok', 'report exported → sipress.json')
  }

  function exportCsv() {
    const rows = [['tick','cps','conc','asr','fail']]
    for (let i = 0; i < series.value.cps.length; i++)
      rows.push([String(i), series.value.cps[i]?.toFixed(2)?? '0', String(series.value.conc[i]??0), series.value.asr[i]?.toFixed(2)?? '0', String(series.value.fail[i]??0)])
    const blob = new Blob([rows.map(r => r.join(',')).join('\n')], { type:'text/csv' })
    const a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = `sipress_${Date.now()}.csv`; a.click()
    addLog('ok', 'report exported → sipress.csv')
  }

  function exportAccountsCsv() {
    const rows = [['extension','username','password','domain'], ...config.value.agent.accounts.map(a => [a.extension, a.username, a.password, a.domain])]
    const blob = new Blob([rows.map(r => r.join(',')).join('\n')], { type:'text/csv' })
    const a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = 'sipress_accounts.csv'; a.click()
  }

  async function exportHtml() {
    try {
      const d   = new Date()
      const pad = (n: number) => String(n).padStart(2, '0')
      const ts  = `${d.getFullYear()}${pad(d.getMonth()+1)}${pad(d.getDate())}_${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`
      const html = await invoke<string>('get_html_report', {
        serverAddr: config.value.server,
        timestamp:  ts,
      })
      const blob = new Blob([html], { type: 'text/html' })
      const a = document.createElement('a')
      a.href = URL.createObjectURL(blob)
      a.download = `sipress_${ts}.html`
      a.click()
      addLog('ok', `HTML 報告已匯出 → sipress_${ts}.html`)
    } catch (e) {
      addLog('err', `HTML 報告匯出失敗: ${e}`)
    }
  }

  return {
    config, status, elapsedSec, metrics, series,
    callStates, respCodes, logs, flowTimes,
    rtpMetrics, mosRating,
    accountImportError, registeredCount,
    activeConcurrency, activeCps, cliCommand, progressPct,
    addLog, clearLog,
    generateAccounts, removeAccount, clearAccounts, updateAccount,
    importAccountText, parseAccountText,
    registerAccount, registerAll,
    startTest, stopTest,
    exportJson, exportCsv, exportHtml, exportAccountsCsv,
  }
})
