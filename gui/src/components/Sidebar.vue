<script setup lang="ts">
import { ref, computed } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { useTestStore } from '../stores/testStore'
import GenerateDialog from './GenerateDialog.vue'

const store = useTestStore()
const copied = ref(false)
const fileInputRef = ref<HTMLInputElement | null>(null)
const dragOver = ref(false)
const showGenDialog = ref(false)

function handleGenConfirm(opts: Parameters<typeof store.generateAccounts>[0]) {
  store.generateAccounts(opts)
  showGenDialog.value = false
}

// Section collapse state（民眾端 / 座席端各自記憶）
const sec = ref({ target:true, caller:true, agent:true, cliOpts:false })

const locked = computed(() => store.isRunning)
const serverInvalid = computed(() => !/^[^\s:]+:\d{1,5}$/.test(store.config.server.trim()))
const audioMissing  = computed(() => store.config.caller.enableAudio && !store.config.caller.audioFile.trim())

// 清除全部帳號：兩段式確認
const confirmClearAcc = ref(false)
let clearTimer: ReturnType<typeof setTimeout> | null = null
function onClearAccounts() {
  if (!confirmClearAcc.value) {
    confirmClearAcc.value = true
    clearTimer = setTimeout(() => (confirmClearAcc.value = false), 3000)
    return
  }
  if (clearTimer) clearTimeout(clearTimer)
  confirmClearAcc.value = false
  store.clearAccounts()
}
function toggleSec(k: keyof typeof sec.value) { sec.value[k] = !sec.value[k] }

// Copy CLI
async function copyCli() {
  await navigator.clipboard.writeText(store.cliCommand)
  copied.value = true
  setTimeout(() => (copied.value = false), 2000)
}

async function pickAudioFile() {
  const path = await open({ multiple: false, filters: [{ name: 'Audio', extensions: ['wav', 'pcm', 'raw', 'mp3'] }] })
  if (path && typeof path === 'string') store.config.caller.audioFile = path
}

// Account file upload
function triggerFileInput() { if (!locked.value) fileInputRef.value?.click() }

function onFileChange(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0]
  if (!file) return
  readFile(file)
  ;(e.target as HTMLInputElement).value = ''
}

function onDrop(e: DragEvent) {
  dragOver.value = false
  e.preventDefault()
  if (locked.value) return
  const file = e.dataTransfer?.files[0]
  if (file) readFile(file)
}

function readFile(file: File) {
  const reader = new FileReader()
  reader.onload = ev => {
    const text = ev.target?.result as string
    store.importAccountText(text)
  }
  reader.readAsText(file)
}

// Generate blank accounts
// Status badge color
function statusColor(s: string) {
  return s === 'registered' ? 'var(--accent)'
       : s === 'registering' ? 'var(--warn)'
       : s === 'failed' ? 'var(--danger)'
       : 'var(--text2)'
}
function statusLabel(s: string) {
  return s === 'registered' ? 'REG' : s === 'registering' ? '…' : s === 'failed' ? 'ERR' : '—'
}
function statusTitle(s: string) {
  return s === 'registered' ? '已註冊' : s === 'registering' ? '註冊中' : s === 'failed' ? '註冊失敗（見事件記錄）' : '未註冊'
}


</script>

<template>
  <aside class="sidebar">
    <div class="lock-banner" v-if="locked">● 測試執行中，設定已鎖定</div>
    <fieldset class="lock-wrap" :disabled="locked">

    <!-- ── TARGET ─────────────────────────────────────────── -->
    <div class="section">
      <div class="section-hdr" @click="toggleSec('target')">
        <span class="section-label">連線目標</span>
        <span class="arrow" :class="{ open: sec.target }">▶</span>
      </div>
      <div class="section-body" v-show="sec.target">
        <div class="field">
          <label>SIP 伺服器 <span class="tag">-s</span></label>
          <input v-model="store.config.server" type="text" placeholder="host:port" :class="{ invalid: serverInvalid }">
          <div class="field-err" v-if="serverInvalid">格式需為 host:port，例：192.168.1.10:5060</div>
        </div>
        <div class="field-row">
          <div class="field">
            <label>傳輸協定</label>
            <select v-model="store.config.transport">
              <option>UDP</option><option>TCP</option>
            </select>
          </div>
          <div class="field">
            <label>本地埠</label>
            <input v-model.number="store.config.localPort" type="number" min="1024" max="65535">
          </div>
        </div>
        <div class="field">
          <label>持續時間 (s) <span class="tag">--duration</span></label>
          <div class="slider-row">
            <input type="range" min="0" max="3600" v-model.number="store.config.duration">
            <input type="number" min="0" v-model.number="store.config.duration" class="num-input">
          </div>
          <div class="field-hint">{{ store.config.duration === 0 ? '0 = 不限時間（需搭配總通數或手動停止）' : `${store.config.duration} 秒後自動停止` }}</div>
        </div>
      </div>
    </div>

    <!-- ── MODE TABS ──────────────────────────────────────── -->
    <div class="mode-tabs">
      <button
        class="mode-tab"
        :class="{ active: store.config.mode === 'caller' }"
        @click="store.config.mode = 'caller'"
      >
        <svg class="mode-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.13.96.36 1.9.7 2.81a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45c.91.34 1.85.57 2.81.7A2 2 0 0 1 22 16.92z"/></svg>
        <span class="mode-name">民眾端<span v-if="store.sessions.caller.status === 'running'" class="run-dot" title="執行中"></span></span>
        <span class="mode-sub">主動撥出</span>
      </button>
      <button
        class="mode-tab"
        :class="{ active: store.config.mode === 'agent' }"
        @click="store.config.mode = 'agent'"
      >
        <svg class="mode-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 18v-6a9 9 0 0 1 18 0v6"/><path d="M21 19a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3zM3 19a2 2 0 0 0 2 2h1a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2H3z"/></svg>
        <span class="mode-name">座席端<span v-if="store.sessions.agent.status === 'running'" class="run-dot" title="執行中"></span></span>
        <span class="mode-sub">SIP 話機</span>
      </button>
    </div>

    <!-- ── CALLER PROFILE ─────────────────────────────────── -->
    <div class="section" v-if="store.config.mode === 'caller'">
      <div class="section-hdr" @click="toggleSec('caller')">
        <span class="section-label">民眾端設定</span>
        <span class="arrow" :class="{ open: sec.caller }">▶</span>
      </div>
      <div class="section-body" v-show="sec.caller">

        <div class="field">
          <label>主叫號碼</label>
          <input v-model="store.config.caller.accessNumber" type="text" placeholder="例：4008001234" :class="{ invalid: !store.config.caller.accessNumber.trim() }">
          <div class="field-hint">From 標頭中的號碼，告訴交換機「這通是誰打的」</div>
        </div>

        <div class="field">
          <label>被叫號碼 <span class="tag">--to</span></label>
          <input v-model="store.config.caller.calleeFixed" type="text" placeholder="例：8001（留空 = 隨機）">
          <div class="field-hint">{{ store.config.caller.calleeFixed ? `所有通話都打給 ${store.config.caller.calleeFixed}` : '留空時使用前綴+隨機尾數（壓測分散負載用）' }}</div>
        </div>

        <div class="field" v-show="!store.config.caller.calleeFixed">
          <label>被叫前綴 / 尾數範圍 <span class="tag">--to-prefix / --to-range</span></label>
          <div class="slider-row" style="gap:6px">
            <input v-model="store.config.caller.calleePrefix" type="text" placeholder="2" style="flex:1">
            <input type="number" min="0" v-model.number="store.config.caller.calleeRange" class="num-input">
          </div>
          <div class="field-hint">例：前綴 80 + 尾數 99 → 隨機產生 80~8099</div>
        </div>

        <div class="field">
          <label>併發數量 <span class="tag">-c</span></label>
          <div class="slider-row">
            <input type="range" min="1" max="1000" v-model.number="store.config.caller.concurrency">
            <input type="number" min="1" max="10000" v-model.number="store.config.caller.concurrency" class="num-input">
          </div>
        </div>

        <!-- CPS 負載模式 -->
        <div class="field">
          <label>CPS 模式</label>
          <select v-model="store.config.caller.cpsPattern.type">
            <option value="flat">固定</option>
            <option value="ramp">漸增 Ramp</option>
            <option value="step">階梯 Step</option>
            <option value="spike">尖峰 Spike</option>
          </select>
          <div class="field-hint" v-if="store.config.caller.cpsPattern.type !== 'flat'">
            {{ store.config.caller.cpsPattern.type === 'step' ? '依下列階層依序執行' : '需設定持續時間，CPS 依時間比例變化' }}
          </div>
        </div>

        <div class="field" v-if="store.config.caller.cpsPattern.type === 'flat'">
          <label>呼叫頻率 CPS <span class="tag">--cps</span></label>
          <div class="slider-row">
            <input type="range" min="1" max="200" v-model.number="store.config.caller.cps">
            <input type="number" min="1" max="1000" v-model.number="store.config.caller.cps" class="num-input">
          </div>
        </div>

        <!-- Ramp config -->
        <div class="field-row" v-if="store.config.caller.cpsPattern.type === 'ramp'">
          <div class="field" style="flex:1">
            <label>起始 CPS</label>
            <input type="number" min="1" max="500" v-model.number="store.config.caller.cpsPattern.rampStart">
          </div>
          <div class="field" style="flex:1">
            <label>終止 CPS</label>
            <input type="number" min="1" max="500" v-model.number="store.config.caller.cpsPattern.rampEnd">
          </div>
        </div>

        <!-- Step config -->
        <div v-if="store.config.caller.cpsPattern.type === 'step'">
          <div class="field-row step-row" v-for="(step, idx) in store.config.caller.cpsPattern.steps" :key="idx">
            <div class="field" style="flex:1">
              <label v-if="idx === 0">CPS</label>
              <input type="number" min="1" max="500" v-model.number="step.cps">
            </div>
            <div class="field" style="flex:1">
              <label v-if="idx === 0">秒數</label>
              <input type="number" min="1" max="3600" v-model.number="step.duration_secs">
            </div>
            <button class="del-btn" @click="store.config.caller.cpsPattern.steps.splice(idx, 1)" style="margin-bottom:6px" title="刪除此階層" aria-label="刪除此階層">×</button>
          </div>
          <button class="btn btn-sm" @click="store.config.caller.cpsPattern.steps.push({ cps: 10, duration_secs: 30 })" style="margin-top:2px">+ 新增階層</button>
        </div>

        <!-- Spike config -->
        <div v-if="store.config.caller.cpsPattern.type === 'spike'">
          <div class="field-row">
            <div class="field" style="flex:1">
              <label>基準 CPS</label>
              <input type="number" min="1" max="500" v-model.number="store.config.caller.cpsPattern.spikeBase">
            </div>
            <div class="field" style="flex:1">
              <label>尖峰 CPS</label>
              <input type="number" min="1" max="500" v-model.number="store.config.caller.cpsPattern.spikePeak">
            </div>
          </div>
          <div class="field-row">
            <div class="field" style="flex:1">
              <label>觸發位置 (%)</label>
              <input type="number" min="0" max="100" v-model.number="store.config.caller.cpsPattern.spikeAtPct">
            </div>
            <div class="field" style="flex:1">
              <label>持續秒數</label>
              <input type="number" min="1" max="300" v-model.number="store.config.caller.cpsPattern.spikeDurationSecs">
            </div>
          </div>
        </div>

        <div class="field">
          <label>總測試通數 <span class="tag">--max-calls</span></label>
          <div class="slider-row">
            <input type="range" min="0" max="10000" step="100" v-model.number="store.config.caller.totalCalls">
            <input type="number" min="0" v-model.number="store.config.caller.totalCalls" class="num-input">
          </div>
          <div class="field-hint">{{ store.config.caller.totalCalls === 0 ? '0 = 不限（依測試時長）' : `達 ${store.config.caller.totalCalls} 通後自動停止` }}</div>
        </div>

        <div class="field">
          <label>單通時長 (s) <span class="tag">--call-duration</span></label>
          <div class="slider-row">
            <input type="range" min="0" max="300" v-model.number="store.config.caller.callDuration">
            <input type="number" min="0" v-model.number="store.config.caller.callDuration" class="num-input">
          </div>
          <div class="field-hint">{{ store.config.caller.callDuration === 0 ? '0 = 不限（直到對方掛斷）' : `接通後約 ${store.config.caller.callDuration} 秒掛斷` }}</div>
        </div>

        <!-- Audio -->
        <div class="field">
          <div class="toggle-row">
            <span>播放音檔</span>
            <label class="toggle">
              <input type="checkbox" v-model="store.config.caller.enableAudio">
              <span class="track"><span class="thumb"></span></span>
            </label>
          </div>
        </div>

        <div class="field" v-if="store.config.caller.enableAudio">
          <label>音檔路徑</label>
          <div class="file-pick-row">
            <input v-model="store.config.caller.audioFile" type="text" placeholder="audio.wav" class="file-path-input" :class="{ invalid: audioMissing }" :title="store.config.caller.audioFile">
            <button class="btn btn-sm" @click="pickAudioFile">瀏覽</button>
          </div>
          <div class="field-err" v-if="audioMissing">請選擇音檔，否則無法開始測試</div>
          <div class="field-hint" v-else>支援 WAV / MP3 / PCM（8kHz/16bit 建議）</div>
        </div>

        <!-- Codec -->
        <div class="field" v-if="store.config.caller.enableAudio">
          <label>Codec</label>
          <select v-model="store.config.caller.codec">
            <option value="pcma">G.711A (PCMA)</option>
            <option value="pcmu">G.711U (PCMU)</option>
          </select>
        </div>

        <!-- DTMF -->
        <div class="field">
          <label>DTMF 序列</label>
          <input v-model="store.config.caller.dtmfSequence" type="text" placeholder="1,w2000,2,w500,#">
          <div class="field-hint">接通後自動送出（w=等待ms，如 1,w2000,# 表示按1→等2秒→按#）</div>
        </div>

      </div>
    </div>

    <!-- ── AGENT PROFILE ──────────────────────────────────── -->
    <div class="section agent-section" v-if="store.config.mode === 'agent'">
      <div class="section-hdr" @click="toggleSec('agent')">
        <span class="section-label">座席端設定</span>
        <div class="section-hdr-right">
          <span class="acc-count-badge">
            {{ store.config.agent.accounts.length }} 帳號
          </span>
          <span class="arrow" :class="{ open: sec.agent }">▶</span>
        </div>
      </div>
      <div class="section-body agent-body" v-show="sec.agent">

        <!-- Default domain -->
        <div class="field">
          <label>預設 SIP Domain</label>
          <input v-model="store.config.agent.defaultDomain" type="text" placeholder="192.168.1.100">
        </div>

        <!-- Timeouts -->
        <div class="field-row">
          <div class="field" style="flex:1">
            <label>ACK 逾時 (秒)</label>
            <input v-model.number="store.config.agent.ackTimeout" type="number" min="1" max="60" placeholder="10">
          </div>
          <div class="field" style="flex:1">
            <label>BYE 逾時 (秒)</label>
            <input v-model.number="store.config.agent.byeTimeout" type="number" min="1" max="600" placeholder="150">
          </div>
        </div>

        <!-- Upload -->
        <div
          class="drop-zone"
          :class="{ over: dragOver, disabled: locked }"
          role="button"
          :tabindex="locked ? -1 : 0"
          @keydown.enter.prevent="triggerFileInput"
          @keydown.space.prevent="triggerFileInput"
          @dragover.prevent="dragOver = !locked"
          @dragleave="dragOver = false"
          @drop="onDrop"
          @click="triggerFileInput"
        >
          <div class="drop-icon">↑</div>
          <div class="drop-main">上傳帳號清單</div>
          <div class="drop-sub">CSV / TXT · 拖曳或點擊</div>
          <input ref="fileInputRef" type="file" accept=".csv,.txt" style="display:none" @change="onFileChange">
        </div>

        <!-- Format hint -->
        <div class="format-hint">
          <div class="hint-title">支援格式</div>
          <div class="hint-line"><span class="hint-code">extension,username,password[,domain]</span></div>
          <div class="hint-line"><span class="hint-code">username:password[:domain]</span></div>
          <div class="hint-line"><span class="hint-code">ext user pass [domain]</span></div>
          <div class="hint-line muted">首行若含標頭會自動跳過</div>
        </div>

        <!-- Import error -->
        <div class="import-error" v-if="store.accountImportError">
          ⚠ {{ store.accountImportError }}
        </div>

        <!-- Generate via dialog -->
        <button class="btn btn-sm btn-accent gen-btn" @click="showGenDialog = true">
          + 快速生成帳號
        </button>

        <!-- Account table -->
        <div class="acc-table-wrap" v-if="store.config.agent.accounts.length > 0">
          <div class="acc-table-hdr">
            <div class="acc-col ext">分機</div>
            <div class="acc-col user">帳號</div>
            <div class="acc-col pass">密碼</div>
            <div class="acc-col stat">狀態</div>
            <div class="acc-col del"></div>
          </div>
          <div class="acc-table-body">
            <div
              v-for="acc in store.config.agent.accounts"
              :key="acc.id"
              class="acc-row"
            >
              <div class="acc-col ext">
                <input
                  :value="acc.extension"
                  @input="store.updateAccount(acc.id, { extension: ($event.target as HTMLInputElement).value })"
                  class="acc-input"
                  placeholder="1001"
                >
              </div>
              <div class="acc-col user">
                <input
                  :value="acc.username"
                  @input="store.updateAccount(acc.id, { username: ($event.target as HTMLInputElement).value })"
                  class="acc-input"
                  placeholder="user"
                >
              </div>
              <div class="acc-col pass">
                <input
                  :value="acc.password"
                  @input="store.updateAccount(acc.id, { password: ($event.target as HTMLInputElement).value })"
                  class="acc-input pass-input"
                  type="password"
                  placeholder="••••"
                >
              </div>
              <div class="acc-col stat">
                <span class="status-dot" :style="{ color: statusColor(acc.status) }" :title="statusTitle(acc.status)">
                  {{ statusLabel(acc.status) }}
                </span>
              </div>
              <div class="acc-col del">
                <button class="del-btn" @click="store.removeAccount(acc.id)" title="刪除帳號" aria-label="刪除帳號">×</button>
              </div>
            </div>
          </div>
        </div>

        <!-- Account actions -->
        <div class="acc-actions" v-if="store.config.agent.accounts.length > 0">
          <div class="reg-summary">
            <span class="reg-ok">{{ store.registeredCount }}</span>
            <span class="reg-sep">/</span>
            <span>{{ store.config.agent.accounts.length }}</span>
            <span class="reg-label">已註冊</span>
          </div>
          <div style="display:flex;gap:4px">
            <button class="btn btn-sm" @click="store.registerAll()" title="重新對所有未註冊帳號發 REGISTER">重新註冊</button>
            <button class="btn btn-sm" @click="store.exportAccountsCsv()">匯出 CSV</button>
            <button class="btn btn-sm" :class="confirmClearAcc ? 'btn-danger' : 'danger-text'" @click="onClearAccounts">{{ confirmClearAcc ? '確定清除？' : '清除全部' }}</button>
          </div>
        </div>

        <!-- Empty state -->
        <div class="acc-empty" v-else>
          上傳帳號清單或使用快速生成新增座席帳號
        </div>

      </div>
    </div>

    </fieldset>

    <!-- ── CLI Preview ────────────────────────────────────── -->
    <div class="cli-section">
      <div class="cli-label-row">
        <span class="cli-label">CLI 指令預覽</span>
        <button class="cli-opts-btn" @click="sec.cliOpts = !sec.cliOpts" :class="{ open: sec.cliOpts }">選項 <span class="arrow" :class="{ open: sec.cliOpts }">▶</span></button>
      </div>
      <div class="cli-opts" v-show="sec.cliOpts">
        <div class="field">
          <label>報告格式 <span class="tag">--format / --tui</span></label>
          <select v-model="store.config.cliFormat">
            <option value="tui">TUI 儀表板</option>
            <option value="table">Table</option>
            <option value="json">JSON</option>
            <option value="csv">CSV</option>
          </select>
        </div>
        <div class="toggle-row field">
          <span>詳細記錄 <span class="tag">--verbose</span></span>
          <label class="toggle">
            <input type="checkbox" v-model="store.config.cliVerbose">
            <span class="track"><span class="thumb"></span></span>
          </label>
        </div>
      </div>
      <div class="cli-box" role="button" tabindex="0" @click="copyCli" @keydown.enter="copyCli" :title="copied ? '已複製' : '點擊複製'">
        <span class="cli-text">{{ store.cliCommand }}</span>
        <span class="cli-copy">{{ copied ? '✓' : '⎘' }}</span>
      </div>
      <div class="cli-warn" v-if="store.cliUnsupported.length > 0 && store.config.mode === 'caller'">
        CLI 不支援：{{ store.cliUnsupported.join('、') }}（僅 GUI 生效）
      </div>
    </div>

<!-- Generate Dialog -->
<GenerateDialog
  :visible="showGenDialog"
  :default-domain="store.config.agent.defaultDomain"
  @confirm="handleGenConfirm"
  @cancel="showGenDialog = false"
/>

  </aside>
</template>

<style scoped>
.sidebar {
  background: var(--bg1);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  overflow-x: hidden;
  width: 260px;
  flex-shrink: 0;
}

/* Lock while running */
.lock-wrap { display: contents; border: 0; }
.lock-banner {
  font-size: 11px;
  color: var(--accent);
  background: rgba(var(--accent-rgb),0.08);
  border-bottom: 1px solid rgba(var(--accent-rgb),0.25);
  padding: 6px 14px;
  position: sticky;
  top: 0;
  z-index: 1;
}
fieldset:disabled input,
fieldset:disabled select,
fieldset:disabled .btn,
fieldset:disabled .del-btn { opacity: 0.55; cursor: not-allowed; }

/* Section */
.section { border-bottom: 1px solid var(--border); }

.section-hdr {
  display: flex;
  align-items: center;
  padding: 9px 14px 7px;
  cursor: pointer;
  user-select: none;
}
.section-hdr:hover { background: var(--bg2); }

.section-label {
  flex: 1;
  font-size: 10px;
  font-family: var(--mono);
  font-weight: 500;
  letter-spacing: 0.08em;
  color: var(--text2);
  text-transform: uppercase;
}

.section-hdr-right {
  display: flex;
  align-items: center;
  gap: 6px;
}

.acc-count-badge {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--accent);
  background: rgba(var(--accent-rgb),0.1);
  padding: 1px 6px;
  border-radius: 3px;
}

.arrow {
  font-size: 8px;
  color: var(--text2);
  transition: transform 0.2s;
}
.arrow.open { transform: rotate(90deg); }

.section-body { padding: 2px 12px 10px; }

/* Mode tabs */
.mode-tabs {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg0);
}

.mode-tab {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  padding: 8px 6px;
  border-radius: var(--radius);
  border: 1px solid var(--border2);
  background: var(--bg2);
  cursor: pointer;
  transition: all 0.15s;
  font-family: var(--sans);
}
.mode-tab:hover { border-color: var(--border3); background: var(--bg3); }
.mode-tab.active {
  border-color: rgba(var(--accent-rgb),0.35);
  background: rgba(var(--accent-rgb),0.06);
}

.mode-icon { color: var(--text1); margin-bottom: 2px; }
.mode-tab.active .mode-icon { color: var(--accent); }
.mode-tab:disabled { cursor: not-allowed; opacity: 0.6; }
.mode-name {
  font-size: 12px;
  font-weight: 500;
  color: var(--text0);
}
.mode-sub {
  font-size: 10px;
  color: var(--text2);
  font-family: var(--mono);
}
.mode-tab.active .mode-name { color: var(--accent); }
.run-dot {
  display: inline-block;
  width: 6px; height: 6px;
  margin-left: 5px;
  vertical-align: middle;
  border-radius: 50%;
  background: var(--accent);
  animation: run-pulse 1.5s ease-in-out infinite;
}
@keyframes run-pulse {
  0%,100% { opacity: 1; }
  50%      { opacity: 0.3; }
}

/* Fields */
.field { margin-bottom: 8px; }
.field label {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: var(--text1);
  margin-bottom: 3px;
}
.tag {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--text2);
  background: var(--bg3);
  padding: 1px 5px;
  border-radius: 3px;
}
.field-hint {
  font-size: 10px;
  color: var(--text2);
  margin-top: 3px;
  line-height: 1.5;
}

.field input[type="text"],
.field input[type="number"],
.field select {
  width: 100%;
  background: var(--bg0);
  border: 1px solid var(--border2);
  border-radius: var(--radius-sm);
  padding: 5px 8px;
  color: var(--text0);
  font-family: var(--mono);
  font-size: 12px;
  outline: none;
  transition: border-color 0.15s;
}
.field input:hover, .field select:hover { border-color: var(--border3); }
.field input:focus, .field select:focus { border-color: rgba(var(--accent-rgb),0.6); box-shadow: 0 0 0 2px rgba(var(--accent-rgb),0.15); }
.field input.invalid { border-color: rgba(var(--danger-rgb),0.6); }
.field-err {
  font-size: 10px;
  color: var(--danger);
  margin-top: 3px;
}
.step-row { align-items: end; grid-template-columns: 1fr 1fr 16px; }

.field select {
  appearance: none;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='%23888c99'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 8px center;
  padding-right: 24px;
  cursor: pointer;
}

.field-row { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }

/* Slider */
.slider-row { display: flex; align-items: center; gap: 6px; }
.slider-row input[type="range"] {
  flex: 1; height: 3px; background: var(--bg3); border-radius: 2px;
  appearance: none; outline: none; cursor: pointer; border: none; padding: 0;
}
.slider-row input[type="range"]::-webkit-slider-thumb {
  appearance: none; width: 13px; height: 13px; border-radius: 50%;
  background: var(--accent); border: 2px solid var(--bg1); cursor: pointer; transition: transform 0.1s;
}
.slider-row input[type="range"]::-webkit-slider-thumb:hover { transform: scale(1.2); }
.num-input { width: 58px !important; flex-shrink: 0; text-align: right; }

/* Toggle */
.toggle-row {
  display: flex; align-items: center;
  justify-content: space-between;
  font-size: 11px; color: var(--text1);
}
.toggle { position:relative; display:inline-block; width:30px; height:17px; cursor:pointer; }
.toggle input { opacity:0; width:0; height:0; position:absolute; }
.track { position:absolute; inset:0; background:var(--bg3); border:1px solid var(--border2); border-radius:9px; transition:all .2s; }
.thumb { position:absolute; top:2px; left:2px; width:11px; height:11px; background:var(--text2); border-radius:50%; transition:all .2s; }
.toggle input:checked ~ .track { background:rgba(var(--accent-rgb),.2); border-color:rgba(var(--accent-rgb),.4); }
.toggle input:checked ~ .track .thumb { transform:translateX(13px); background:var(--accent); }

/* Audio file pick */
.file-pick-row { display: flex; gap: 6px; }
.file-path-input { flex: 1 !important; }

/* Drop zone */
.drop-zone {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  padding: 12px 8px;
  border: 1px dashed var(--border2);
  border-radius: var(--radius);
  cursor: pointer;
  transition: all 0.15s;
  margin-bottom: 8px;
  text-align: center;
}
.drop-zone.disabled { opacity: 0.5; cursor: not-allowed; }
.drop-zone:not(.disabled):hover, .drop-zone.over {
  border-color: rgba(var(--accent-rgb),0.4);
  background: rgba(var(--accent-rgb),0.04);
}
.drop-icon { font-size: 18px; color: var(--text2); line-height: 1; }
.drop-main { font-size: 12px; color: var(--text0); font-weight: 500; }
.drop-sub  { font-size: 10px; color: var(--text2); font-family: var(--mono); }

/* Format hint */
.format-hint {
  background: var(--bg0);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 6px 8px;
  margin-bottom: 8px;
}
.hint-title {
  font-size: 10px;
  font-family: var(--mono);
  color: var(--text2);
  text-transform: uppercase;
  letter-spacing: 0.07em;
  margin-bottom: 4px;
}
.hint-line { margin-bottom: 2px; }
.hint-code {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--accent);
}
.muted { font-family: var(--mono); font-size: 10px; color: var(--text2); }

/* Import error */
.import-error {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--danger);
  background: rgba(var(--danger-rgb),0.06);
  border: 1px solid rgba(var(--danger-rgb),0.2);
  border-radius: var(--radius-sm);
  padding: 5px 8px;
  margin-bottom: 8px;
}

/* Generate button */
.gen-btn { width: 100%; justify-content: center; margin-bottom: 8px; }

/* Account table */
.acc-table-wrap {
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  overflow: hidden;
  margin-bottom: 6px;
  max-height: 180px;
  display: flex;
  flex-direction: column;
}

.acc-table-hdr {
  display: flex;
  gap: 0;
  background: var(--bg3);
  border-bottom: 1px solid var(--border);
  padding: 3px 6px;
  flex-shrink: 0;
}

.acc-table-body {
  overflow-y: auto;
  flex: 1;
}

.acc-row {
  display: flex;
  align-items: center;
  border-bottom: 1px solid var(--border);
  padding: 2px 6px;
  transition: background 0.1s;
}
.acc-row:last-child { border-bottom: none; }
.acc-row:hover { background: var(--bg2); }

.acc-col {
  display: flex;
  align-items: center;
  flex-shrink: 0;
}
.acc-col.ext  { width: 44px; }
.acc-col.user { flex: 1; }
.acc-col.pass { width: 50px; }
.acc-col.stat { width: 26px; justify-content: center; }
.acc-col.del  { width: 18px; justify-content: center; }

/* header labels */
.acc-table-hdr .acc-col {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--text2);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}

.acc-input {
  width: 100%;
  background: transparent;
  border: none;
  outline: none;
  padding: 2px 3px;
  color: var(--text0);
  font-family: var(--mono);
  font-size: 11px;
}
.acc-input:focus { background: var(--bg3); border-radius: 3px; }
.pass-input { color: var(--text1); letter-spacing: 0.1em; }

.status-dot {
  font-family: var(--mono);
  font-size: 10px;
  font-weight: 600;
}

.del-btn {
  background: none;
  border: none;
  color: var(--text2);
  cursor: pointer;
  font-size: 14px;
  line-height: 1;
  padding: 0;
  transition: color 0.1s;
}
.del-btn:hover { color: var(--danger); }

/* Account actions */
.acc-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}

.reg-summary {
  display: flex;
  align-items: center;
  gap: 3px;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text1);
}
.reg-ok  { color: var(--accent); font-weight: 600; }
.reg-sep { color: var(--text2); }
.reg-label { margin-left: 3px; color: var(--text2); font-size: 10px; }

.acc-empty {
  font-size: 11px;
  color: var(--text2);
  text-align: center;
  padding: 12px 0 4px;
  font-family: var(--mono);
  line-height: 1.6;
}

.agent-section { flex: 1; display: flex; flex-direction: column; }
.agent-body { flex: 1; overflow-y: auto; }

/* CLI */
.cli-section {
  margin-top: auto;
  padding: 10px 12px;
  border-top: 1px solid var(--border);
}
.cli-label-row { display: flex; align-items: center; margin-bottom: 5px; }
.cli-label {
  flex: 1;
  font-size: 10px; font-weight: 500;
  color: var(--text2); letter-spacing: 0.07em;
}
.cli-opts-btn {
  background: none; border: none; cursor: pointer;
  font-size: 10px; color: var(--text2);
  display: inline-flex; align-items: center; gap: 4px;
}
.cli-opts-btn:hover, .cli-opts-btn.open { color: var(--text1); }
.cli-opts { margin-bottom: 6px; }
.cli-warn { margin-top: 5px; font-size: 10px; color: var(--warn); line-height: 1.5; }
.danger-text { color: var(--danger); }
.cli-box {
  display: flex; align-items: flex-start; gap: 6px;
  background: var(--bg0); border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 6px 8px; cursor: pointer; transition: border-color 0.15s;
}
.cli-box:hover { border-color: var(--border3); }
.cli-text { font-family: var(--mono); font-size: 10px; color: var(--text1); word-break: break-all; line-height: 1.6; flex: 1; }
.cli-copy { font-size: 12px; color: var(--text2); flex-shrink: 0; margin-top: 1px; transition: color 0.2s; }
.cli-box:hover .cli-copy { color: var(--accent); }
</style>
