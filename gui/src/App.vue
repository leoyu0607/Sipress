<script setup lang="ts">
import { ref, computed, provide, onMounted, onUnmounted } from 'vue'
import TitleBar    from './components/TitleBar.vue'
import Sidebar     from './components/Sidebar.vue'
import MetricStrip from './components/MetricStrip.vue'
import ChartPanel  from './components/ChartPanel.vue'
import RightPanel  from './components/RightPanel.vue'
import LogPanel    from './components/LogPanel.vue'
import SidebarResizer, { loadSidebarWidth, sidebarMaxFor } from './components/SidebarResizer.vue'

// 側欄寬度（可拖曳調整，記在本機）。實際顯示的寬度不超過目前視窗允許的上限，
// 視窗縮小時側欄跟著變窄，放大後回到使用者設定的寬度
const sidebarWidth = ref(loadSidebarWidth())
const viewportWidth = ref(window.innerWidth)
const onWindowResize = () => { viewportWidth.value = window.innerWidth }
onMounted(() => window.addEventListener('resize', onWindowResize))
onUnmounted(() => window.removeEventListener('resize', onWindowResize))
const sidebarMax = computed(() => sidebarMaxFor(viewportWidth.value))
const shownSidebarWidth = computed(() => Math.min(sidebarWidth.value, sidebarMax.value))

// ── Theme ─────────────────────────────────────────────────────────
const isDark = ref(localStorage.getItem('sipress-theme') !== 'light')

function toggleTheme() {
  isDark.value = !isDark.value
  const theme = isDark.value ? 'dark' : 'light'
  document.documentElement.setAttribute('data-theme', theme)
  localStorage.setItem('sipress-theme', theme)
}

// Set initial attribute
document.documentElement.setAttribute('data-theme', isDark.value ? 'dark' : 'light')

// Provide to all children
provide('isDark', isDark)
provide('toggleTheme', toggleTheme)
</script>

<template>
  <div class="app">
    <TitleBar />
    <div class="body">
      <Sidebar :style="{ width: shownSidebarWidth + 'px' }" />
      <SidebarResizer v-model="sidebarWidth" :max="sidebarMax" />
      <div class="content">
        <MetricStrip />
        <div class="middle">
          <ChartPanel />
          <RightPanel />
        </div>
        <LogPanel />
      </div>
    </div>
  </div>
</template>

<style>
/* ══════════════════════════════════════════════
   DARK THEME (default)
══════════════════════════════════════════════ */
:root,
[data-theme="dark"] {
  --bg0:     #0b0d12;
  --bg1:     #111318;
  --bg2:     #171a22;
  --bg3:     #1e2230;
  --bg4:     #252a38;

  --border:  rgba(255,255,255,0.06);
  --border2: rgba(255,255,255,0.11);
  --border3: rgba(255,255,255,0.20);

  --accent:  #00e5c0;
  --blue:    #3d9fff;
  --warn:    #f5a623;
  --danger:  #ff4d4d;
  --purple:  #b57af9;

  /* RGB triplets for rgba(var(--x-rgb), alpha) */
  --accent-rgb: 0,229,192;
  --blue-rgb:   61,159,255;
  --warn-rgb:   245,166,35;
  --danger-rgb: 255,77,77;
  --purple-rgb: 181,122,249;

  --text0: #e8eaf0;
  /* 對比（對 --bg1 / --bg3）：text1 7.7/6.5、text2 5.5/4.7 — 皆達 WCAG AA 4.5:1；
     text3 僅供裝飾（分隔線、停用狀態），不可用於需閱讀的文字 */
  --text1: #a0a6b8;
  --text2: #848ba0;
  --text3: #4f5568;

  --input-bg:     #0b0d12;
  --input-border: rgba(255,255,255,0.11);

  --scrollbar-thumb: rgba(255,255,255,0.1);

  /* 實心主要按鈕（accent / danger 底）上的文字色 */
  --on-accent: #04211c;
  --on-danger: #1a0505;

  color-scheme: dark;
}

/* ══════════════════════════════════════════════
   LIGHT THEME
══════════════════════════════════════════════ */
[data-theme="light"] {
  --bg0:     #f0f2f5;
  --bg1:     #ffffff;
  --bg2:     #f5f6f8;
  --bg3:     #eaecf0;
  --bg4:     #dde0e6;

  --border:  rgba(0,0,0,0.07);
  --border2: rgba(0,0,0,0.12);
  --border3: rgba(0,0,0,0.20);

  /* 亮色主題的狀態色會直接當文字色使用，需對 --bg0 ~ --bg3 都達 4.5:1 */
  --accent:  #00786a;
  --blue:    #1462a6;
  --warn:    #935c00;
  --danger:  #c02828;
  --purple:  #8b3fd9;

  --accent-rgb: 0,120,106;
  --blue-rgb:   20,98,166;
  --warn-rgb:   147,92,0;
  --danger-rgb: 192,40,40;
  --purple-rgb: 139,63,217;

  --text0: #1a1c22;
  --text1: #3f4560;
  --text2: #5b6377;
  --text3: #a3aab8;

  --input-bg:     #ffffff;
  --input-border: rgba(0,0,0,0.14);

  --scrollbar-thumb: rgba(0,0,0,0.12);

  --on-accent: #ffffff;
  --on-danger: #ffffff;

  color-scheme: light;
}

/* ══════════════════════════════════════════════
   RESET & BASE
══════════════════════════════════════════════ */
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

html, body, #app {
  height: 100%;
  overflow: hidden;
  background: var(--bg0);
  color: var(--text0);
  font-family: var(--sans);
  font-size: 13px;
  line-height: 1.5;
  -webkit-font-smoothing: antialiased;
  transition: background 0.25s, color 0.25s;
}

/* ── Scrollbar ─────────────────────────────── */
::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--scrollbar-thumb); border-radius: 2px; }

/* ── Shared Buttons ────────────────────────── */
.btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 4px 11px;
  border-radius: var(--radius);
  font-family: var(--mono);
  font-size: 11px;
  font-weight: 500;
  border: 1px solid var(--border2);
  background: transparent;
  color: var(--text1);
  cursor: pointer;
  transition: background 0.14s, border-color 0.14s, color 0.14s, transform 0.1s;
  white-space: nowrap;
  outline: none;
}
.btn:hover { background: var(--bg3); color: var(--text0); border-color: var(--border3); }
.btn:active { transform: scale(0.97); }
.btn:disabled { opacity: 0.35; cursor: not-allowed; }

.btn-sm { padding: 3px 8px; font-size: 11px; }

.btn-accent {
  color: var(--accent);
  border-color: rgba(var(--accent-rgb),0.3);
  background: rgba(var(--accent-rgb),0.06);
}
.btn-accent:hover { background: rgba(var(--accent-rgb),0.14); border-color: rgba(var(--accent-rgb),0.5); color: var(--accent); }

.btn-danger {
  color: var(--danger);
  border-color: rgba(var(--danger-rgb),0.3);
  background: rgba(var(--danger-rgb),0.07);
}
.btn-danger:hover { background: rgba(var(--danger-rgb),0.16); border-color: rgba(var(--danger-rgb),0.5); }

/* 主要動作（開始 / 停止測試）：實心底色，整個畫面只放一個 */
.btn-primary {
  color: var(--on-accent);
  background: var(--accent);
  border-color: var(--accent);
  font-weight: 600;
}
.btn-primary:hover { color: var(--on-accent); background: var(--accent); border-color: var(--accent); filter: brightness(1.1); }
.btn-primary.is-danger { color: var(--on-danger); background: var(--danger); border-color: var(--danger); }
.btn-primary.is-danger:hover { color: var(--on-danger); background: var(--danger); border-color: var(--danger); }
.btn-primary:focus-visible { outline: 2px solid rgba(var(--accent-rgb),0.6); outline-offset: 2px; }
.btn-primary.is-danger:focus-visible { outline-color: rgba(var(--danger-rgb),0.6); }

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { animation-duration: 0.01ms !important; animation-iteration-count: 1 !important; transition-duration: 0.01ms !important; }
}

/* ── Keyboard focus ────────────────────────── */
:focus-visible {
  outline: 2px solid rgba(var(--accent-rgb),0.6);
  outline-offset: 1px;
}
.btn:focus-visible { outline: 2px solid rgba(var(--accent-rgb),0.6); }

/* 只給螢幕閱讀器讀的文字 */
.sr-only {
  position: absolute;
  width: 1px; height: 1px;
  padding: 0; margin: -1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
  border: 0;
}

/* ── Fonts (referenced by components) ─────── */
:root {
  --mono: 'JetBrains Mono', 'Cascadia Code', 'Microsoft JhengHei', monospace;
  --sans: 'IBM Plex Sans', 'Microsoft JhengHei', system-ui, sans-serif;
  --radius-sm: 4px;
  --radius:    6px;
  --radius-lg: 10px;
}

/* ── App Shell ─────────────────────────────── */
.app {
  display: flex;
  flex-direction: column;
  height: 100vh;
}
.body {
  display: flex;
  flex: 1;
  overflow: hidden;
}
.content {
  display: flex;
  flex-direction: column;
  flex: 1;
  overflow: hidden;
}
.middle {
  display: flex;
  flex: 1;
  overflow: hidden;
}
</style>
