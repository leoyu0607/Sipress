<script setup lang="ts">
import { computed, inject, type ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useTestStore, MODE_LABEL, type CallMode } from '../stores/testStore'
import ExportMenu from './ExportMenu.vue'
import TemplateMenu from './TemplateMenu.vue'
import StartButton from './StartButton.vue'
import StageNav from './StageNav.vue'
import Icon from './Icon.vue'

const store = useTestStore()
const appWindow = getCurrentWindow()

// Theme (provided by App.vue)
const isDark      = inject<ReturnType<typeof ref<boolean>>>('isDark')!
const toggleTheme = inject<() => void>('toggleTheme')!

async function closeWindow()    { await appWindow.close() }
async function minimizeWindow() { await appWindow.minimize() }
async function maximizeWindow() { await appWindow.toggleMaximize() }

const MODES: CallMode[] = ['caller', 'agent']

const STATUS_LABEL: Record<string, string> = {
  idle: '待命', running: '執行中', done: '已完成', error: '錯誤',
}

// 雙端同時執行時另外標示（側欄模式分頁的執行點也看得到）
const bothRunning = computed(() =>
  store.sessions.caller.status === 'running' && store.sessions.agent.status === 'running'
)
</script>

<template>
  <header class="titlebar" data-tauri-drag-region>

    <div class="logo">sip<em>ress</em></div>
    <div class="sep-v"></div>

    <!-- 設定階段：側欄上方就有民眾端 / 座席端分頁；其他階段側欄收起，改在這裡切換 -->
    <div v-if="store.stage !== 'setup'" class="mode-switch" role="tablist" aria-label="檢視端">
      <button v-for="m in MODES" :key="m" type="button" role="tab" class="ms-btn"
              :class="{ active: store.config.mode === m }" :aria-selected="store.config.mode === m"
              @click="store.config.mode = m">
        {{ MODE_LABEL[m] }}<span v-if="store.sessions[m].status === 'running'" class="ms-dot" title="執行中"></span>
      </button>
    </div>

    <!-- Status badge -->
    <div v-else class="status-badge" :class="store.status">
      <div class="dot" :class="{ pulse: store.status === 'running' }"></div>
      <span>{{ MODE_LABEL[store.config.mode] }} · {{ STATUS_LABEL[store.status] }}</span>
    </div>
    <div class="status-badge running both-badge" v-if="bothRunning" title="民眾端與座席端同時執行中">
      <span>雙端執行中</span>
    </div>

    <!-- 階段：設定 → 執行中 → 結果（進度與時間在執行畫面上方） -->
    <div class="stage-wrap"><StageNav /></div>

    <!-- Actions -->
    <div class="actions">
      <TemplateMenu />
      <ExportMenu />

      <div class="sep-v"></div>

      <!-- Theme toggle -->
      <button class="btn btn-sm theme-btn" @click="toggleTheme"
              :title="isDark ? '切換亮色模式' : '切換暗色模式'"
              :aria-label="isDark ? '切換亮色模式' : '切換暗色模式'">
        <Icon :name="isDark ? 'sun' : 'moon'" :size="14" />
      </button>

      <!-- 設定階段的開始按鈕在右側「準備開始」；結果階段改用「用同樣設定再跑一次」 -->
      <template v-if="store.stage === 'running'">
        <div class="sep-v"></div>
        <StartButton />
      </template>
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

.stage-wrap { flex: 1 1 auto; display: flex; justify-content: center; min-width: 0; }

.mode-switch {
  display: flex; flex-shrink: 0; padding: 2px; gap: 2px;
  background: var(--bg2); border: 1px solid var(--border2); border-radius: var(--radius);
  -webkit-app-region: no-drag;
}
.ms-btn {
  display: inline-flex; align-items: center; gap: 5px;
  padding: 3px 10px; border: none; border-radius: var(--radius-sm);
  background: transparent; color: var(--text1); font-family: var(--sans); font-size: 12px; cursor: pointer;
}
.ms-btn:hover { color: var(--text0); }
.ms-btn.active { background: var(--bg4); color: var(--text0); }
.ms-btn:focus-visible { outline: 2px solid rgba(var(--accent-rgb),0.6); outline-offset: 1px; }
.ms-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); animation: pulse 1.5s ease-in-out infinite; }

.actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

/* Theme button：只有圖示，寬高一致 */
.theme-btn { padding: 4px 7px; }
.theme-btn .icon { transition: transform 0.3s ease; }
.theme-btn:hover .icon { transform: rotate(20deg); }

/* 窄視窗（最小 960px）：收緊間距；「雙端執行中」在側欄模式分頁的執行點也看得到 */
@media (max-width: 1100px) {
  .titlebar { gap: 6px; }
  .actions { gap: 4px; }
  .both-badge, .transport-tag { display: none; }
}
</style>
