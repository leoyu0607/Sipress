<script setup lang="ts">
import { computed, inject, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useTestStore, MODE_LABEL } from '../stores/testStore'
import IntegratedReportDialog from './IntegratedReportDialog.vue'
import ExportMenu from './ExportMenu.vue'
import Icon from './Icon.vue'

const store = useTestStore()
const appWindow = getCurrentWindow()
const reportOpen = ref(false)

// Theme (provided by App.vue)
const isDark      = inject<ReturnType<typeof ref<boolean>>>('isDark')!
const toggleTheme = inject<() => void>('toggleTheme')!

async function closeWindow()    { await appWindow.close() }
async function minimizeWindow() { await appWindow.minimize() }
async function maximizeWindow() { await appWindow.toggleMaximize() }

const timerDisplay = computed(() => {
  const s   = store.elapsedSec
  const m   = String(Math.floor(s / 60)).padStart(2, '0')
  const sec = String(s % 60).padStart(2, '0')
  return `${m}:${sec}`
})

const STATUS_LABEL: Record<string, string> = {
  idle: '待命', running: '執行中', done: '已完成', error: '錯誤',
}

const btnLabel = computed(() =>
  store.stopping ? '停止中…'
  : store.status === 'running' ? '停止測試' : '開始測試'
)
const startBlocked = computed(() =>
  store.status !== 'running' && store.validationErrors.length > 0
)

// 雙端：同時跑民眾端 + 座席端（先座席 REGISTER，再民眾撥號）
const bothRunning = computed(() =>
  store.sessions.caller.status === 'running' && store.sessions.agent.status === 'running'
)
const bothLabel = computed(() => store.anyRunning ? '全部停止' : '雙端啟動')
const bothBlocked = computed(() => !store.anyRunning && store.bothValidationErrors.length > 0)
const bothTitle = computed(() =>
  bothBlocked.value ? ['無法雙端啟動：', ...store.bothValidationErrors.map(e => `• ${e}`)].join('\n')
  : store.anyRunning ? '停止民眾端與座席端'
  : '先啟動座席端註冊，再啟動民眾端撥號'
)
function toggleBoth() {
  if (store.anyRunning) store.stopAll()
  else store.startBoth()
}
const btnTitle = computed(() =>
  startBlocked.value ? ['無法開始：', ...store.validationErrors.map(e => `• ${e}`)].join('\n') : ''
)
// 主要按鈕：待命時實心 accent、執行中實心 danger
const btnClass = computed(() =>
  store.status === 'running' ? 'btn btn-primary is-danger start-btn' : 'btn btn-primary start-btn'
)

function toggleTest() {
  if (store.status === 'running') store.stopTest()
  else store.startTest()
}
</script>

<template>
  <header class="titlebar" data-tauri-drag-region>

    <div class="logo">sip<em>ress</em></div>
    <div class="sep-v"></div>

    <!-- Status badge -->
    <div class="status-badge" :class="store.status">
      <div class="dot" :class="{ pulse: store.status === 'running' }"></div>
      <span>{{ MODE_LABEL[store.config.mode] }} · {{ STATUS_LABEL[store.status] }}</span>
    </div>
    <div class="status-badge running both-badge" v-if="bothRunning" title="民眾端與座席端同時執行中">
      <span>雙端執行中</span>
    </div>

    <!-- Target -->
    <div class="target-info" v-if="store.config.server" :title="`${store.config.server} (${store.config.transport})`">
      → <strong>{{ store.config.server }}</strong>
      <span class="transport-tag">{{ store.config.transport }}</span>
    </div>

    <!-- Progress bar -->
    <div class="progress-wrap" v-if="store.status === 'running'">
      <div class="progress-bar" :style="{ width: store.progressPct + '%' }"></div>
    </div>

    <!-- Timer -->
    <div class="timer">{{ timerDisplay }}</div>

    <!-- Actions -->
    <div class="actions">
      <ExportMenu />
      <button class="btn btn-sm"
              @click="reportOpen = true"
              title="整合報告：結論、通話量與聲音品質圖表、受測主機資源，依門檻判定；可匯出 PDF">
        <Icon name="file-text" :size="13" />整合報告
      </button>

      <div class="sep-v"></div>

      <!-- Theme toggle -->
      <button class="btn btn-sm theme-btn" @click="toggleTheme"
              :title="isDark ? '切換亮色模式' : '切換暗色模式'"
              :aria-label="isDark ? '切換亮色模式' : '切換暗色模式'">
        <Icon :name="isDark ? 'sun' : 'moon'" :size="14" />
      </button>

      <div class="sep-v"></div>

      <span class="start-wrap" :title="bothTitle">
        <button class="btn" :disabled="bothBlocked" @click="toggleBoth">
          <Icon :name="store.anyRunning ? 'stop' : 'arrow-left-right'" :size="13" />{{ bothLabel }}
        </button>
      </span>
      <span class="start-wrap" :title="btnTitle">
        <button :class="btnClass" :disabled="startBlocked || store.stopping" @click="toggleTest">
          <Icon :name="store.status === 'running' ? 'stop' : 'play'" :size="13" />{{ btnLabel }}
        </button>
      </span>
    </div>

    <!-- Window controls (Windows style) -->
    <div class="wc-group">
      <button class="wc" title="最小化" aria-label="最小化" @click="minimizeWindow">
        <svg width="10" height="10" viewBox="0 0 10 10"><line x1="0" y1="5" x2="10" y2="5" stroke="currentColor"/></svg>
      </button>
      <button class="wc" title="最大化 / 還原" aria-label="最大化" @click="maximizeWindow">
        <svg width="10" height="10" viewBox="0 0 10 10"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor"/></svg>
      </button>
      <button class="wc wc-close" title="關閉" aria-label="關閉" @click="closeWindow">
        <svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 0L10 10M10 0L0 10" stroke="currentColor"/></svg>
      </button>
    </div>
    <IntegratedReportDialog :visible="reportOpen" @close="reportOpen = false" />
  </header>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 0 0 14px;
  height: 44px;
  background: var(--bg1);
  border-bottom: 1px solid var(--border);
  user-select: none;
  flex-shrink: 0;
  transition: background 0.25s, border-color 0.25s;
}

.wc-group { display: flex; align-self: stretch; flex-shrink: 0; margin-left: 4px; }
.wc {
  width: 44px;
  display: flex; align-items: center; justify-content: center;
  border: none;
  background: transparent;
  color: var(--text1);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.wc:hover { background: var(--bg3); color: var(--text0); }
.wc-close:hover { background: #e81123; color: #fff; }

.start-wrap { display: inline-flex; }

.logo {
  font-family: var(--mono);
  font-size: 14px;
  font-weight: 700;
  color: var(--accent);
  letter-spacing: -0.5px;
}
.logo em { color: var(--text1); font-style: normal; font-weight: 300; }

.sep-v {
  width: 1px;
  height: 16px;
  background: var(--border2);
  flex-shrink: 0;
}

.status-badge {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 3px 9px;
  border-radius: 99px;
  font-family: var(--mono);
  font-size: 11px;
  font-weight: 500;
  border: 1px solid;
  white-space: nowrap;   /* 窄視窗時不可被擠成直排；改由右側目標位址截斷 */
  flex-shrink: 0;
  transition: all 0.3s;
}
.status-badge.idle    { color: var(--text1); border-color: var(--border2); }
.status-badge.running { color: var(--accent); border-color: rgba(var(--accent-rgb),0.35); background: rgba(var(--accent-rgb),0.08); }
.status-badge.done    { color: var(--blue);   border-color: rgba(var(--blue-rgb),0.35);   background: rgba(var(--blue-rgb),0.08); }
.status-badge.error   { color: var(--danger); border-color: rgba(var(--danger-rgb),0.35); background: rgba(var(--danger-rgb),0.08); }

.dot {
  width: 6px; height: 6px;
  border-radius: 50%;
  background: currentColor;
  flex-shrink: 0;
}
.dot.pulse { animation: pulse 1.5s ease-in-out infinite; }
@keyframes pulse {
  0%,100% { opacity: 1; transform: scale(1); }
  50%      { opacity: 0.3; transform: scale(0.8); }
}

.target-info {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}
.target-info strong { color: var(--text0); font-weight: 500; }
.transport-tag {
  margin-left: 4px;
  font-size: 11px;
  color: var(--text2);
  background: var(--bg3);
  padding: 1px 5px;
  border-radius: 3px;
}

.progress-wrap {
  flex: 1 1 60px;
  min-width: 48px;           /* 窄視窗時目標位址先縮（省略號），進度條不可被擠到看不見 */
  max-width: 120px;
  height: 3px;
  background: var(--bg3);
  border-radius: 2px;
  overflow: hidden;
}
.progress-bar {
  height: 100%;
  background: var(--accent);
  border-radius: 2px;
  transition: width 1s linear;
}

.timer {
  font-family: var(--mono);
  font-size: 12px;
  font-weight: 500;
  color: var(--text0);
  background: var(--bg3);
  border: 1px solid var(--border2);
  padding: 3px 10px;
  border-radius: var(--radius);
  min-width: 56px;
  text-align: center;
  transition: background 0.25s, border-color 0.25s;
}

.actions {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
}

/* Theme button：只有圖示，寬高一致 */
.theme-btn { padding: 4px 7px; }
.theme-btn .icon { transition: transform 0.3s ease; }
.theme-btn:hover .icon { transform: rotate(20deg); }

.start-btn { min-width: 96px; justify-content: center; }

/* 窄視窗（最小 960px）：收緊間距；「雙端執行中」在側欄模式分頁的執行點也看得到 */
@media (max-width: 1100px) {
  .titlebar { gap: 6px; }
  .actions { gap: 4px; }
  .both-badge, .transport-tag { display: none; }
}
</style>
