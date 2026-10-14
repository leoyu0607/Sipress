<script setup lang="ts">
import { ref, computed, watch, nextTick, onUnmounted } from 'vue'
import { useTestStore, type LogEntry } from '../stores/testStore'

const store = useTestStore()
const bodyRef = ref<HTMLDivElement | null>(null)
const autoScroll = ref(true)

// ── Level filter ──────────────────────────────────────────────────
const LEVELS: { key: LogEntry['level']; label: string; color: string }[] = [
  { key: 'ok',   label: 'OK',   color: 'var(--accent)' },
  { key: 'info', label: 'INFO', color: 'var(--blue)' },
  { key: 'warn', label: 'WARN', color: 'var(--warn)' },
  { key: 'err',  label: 'ERR',  color: 'var(--danger)' },
]
const levelColor: Record<string, string> = Object.fromEntries(
  [...LEVELS.map(l => [l.key, l.color]), ['dbg', 'var(--text2)']]
)
const hidden = ref<Set<string>>(new Set())
function toggleLevel(k: string) {
  const s = new Set(hidden.value)
  s.has(k) ? s.delete(k) : s.add(k)
  hidden.value = s
}
const counts = computed(() => {
  const c: Record<string, number> = {}
  for (const l of store.logs) c[l.level] = (c[l.level] ?? 0) + 1
  return c
})
const visibleLogs = computed(() =>
  hidden.value.size === 0 ? store.logs : store.logs.filter(l => !hidden.value.has(l.level))
)

watch(() => store.logs[store.logs.length - 1]?.id, async () => {
  if (!autoScroll.value) return
  await nextTick()
  if (bodyRef.value) bodyRef.value.scrollTop = bodyRef.value.scrollHeight
})

function onScroll() {
  if (!bodyRef.value) return
  const el = bodyRef.value
  autoScroll.value = el.scrollTop + el.clientHeight >= el.scrollHeight - 20
}

// ── Copy (with feedback) ──────────────────────────────────────────
const copied = ref(false)
async function copyLog() {
  const text = visibleLogs.value
    .map(l => `${l.ts} [${l.level.toUpperCase()}] ${l.msg}`)
    .join('\n')
  await navigator.clipboard.writeText(text)
  copied.value = true
  setTimeout(() => (copied.value = false), 1500)
}

// ── Clear (two-step confirm) ──────────────────────────────────────
const confirmClear = ref(false)
let confirmTimer: ReturnType<typeof setTimeout> | null = null
function onClear() {
  if (!confirmClear.value) {
    confirmClear.value = true
    confirmTimer = setTimeout(() => (confirmClear.value = false), 3000)
    return
  }
  if (confirmTimer) clearTimeout(confirmTimer)
  confirmClear.value = false
  store.clearLog()
}

// ── Resizable height ──────────────────────────────────────────────
const HEIGHT_KEY = 'sipress-log-height'
const height = ref(Number(localStorage.getItem(HEIGHT_KEY)) || 220)
const collapsed = ref(false)
let dragStartY = 0
let dragStartH = 0

function onDragMove(e: MouseEvent) {
  const maxH = window.innerHeight * 0.6
  height.value = Math.round(Math.min(maxH, Math.max(90, dragStartH + (dragStartY - e.clientY))))
}
function onDragEnd() {
  window.removeEventListener('mousemove', onDragMove)
  window.removeEventListener('mouseup', onDragEnd)
  document.body.style.cursor = ''
  localStorage.setItem(HEIGHT_KEY, String(height.value))
}
function onDragStart(e: MouseEvent) {
  if (collapsed.value) return
  dragStartY = e.clientY
  dragStartH = height.value
  document.body.style.cursor = 'ns-resize'
  window.addEventListener('mousemove', onDragMove)
  window.addEventListener('mouseup', onDragEnd)
}
onUnmounted(onDragEnd)
</script>

<template>
  <div class="log-panel" :style="{ height: collapsed ? 'auto' : height + 'px' }">
    <div class="resize-handle" v-if="!collapsed" @mousedown.prevent="onDragStart" title="拖曳調整高度"></div>

    <div class="log-hdr">
      <button class="collapse-btn" @click="collapsed = !collapsed" :title="collapsed ? '展開' : '收合'">
        <span class="arrow" :class="{ open: !collapsed }">▶</span>
      </button>
      <span class="log-title">事件記錄</span>
      <span class="log-count">{{ store.logs.length }}</span>

      <div class="level-filters">
        <button
          v-for="l in LEVELS"
          :key="l.key"
          class="lvl-chip"
          :class="{ off: hidden.has(l.key) }"
          :style="{ '--chip': l.color }"
          :title="hidden.has(l.key) ? `顯示 ${l.label}` : `隱藏 ${l.label}`"
          @click="toggleLevel(l.key)"
        >{{ l.label }} <span class="lvl-n">{{ counts[l.key] ?? 0 }}</span></button>
      </div>

      <div class="log-actions">
        <button class="btn btn-sm"
                :class="{ active: autoScroll }"
                @click="autoScroll = !autoScroll"
                :title="autoScroll ? '自動捲動：開' : '自動捲動：關'">
          {{ autoScroll ? '↓ 自動捲動' : '⏸ 已暫停' }}
        </button>
        <button class="btn btn-sm" :class="{ active: copied }" @click="copyLog">{{ copied ? '✓ 已複製' : '複製' }}</button>
        <button class="btn btn-sm" :class="{ 'btn-danger': confirmClear }" @click="onClear">
          {{ confirmClear ? '確定清除？' : '清除' }}
        </button>
      </div>
    </div>

    <div class="log-body" ref="bodyRef" @scroll="onScroll" v-show="!collapsed">
      <div v-if="visibleLogs.length === 0" class="log-empty">
        {{ store.logs.length === 0 ? '尚無事件' : '目前篩選條件下沒有事件' }}
      </div>
      <div
        v-for="entry in visibleLogs"
        :key="entry.id"
        class="log-line"
      >
        <span class="log-ts">{{ entry.ts }}</span>
        <span class="log-level" :style="{ color: levelColor[entry.level] }">
          {{ entry.level.toUpperCase() }}
        </span>
        <span class="log-msg" :class="{ 'is-err': entry.level === 'err' }">{{ entry.msg }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.log-panel {
  position: relative;
  border-top: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}

.resize-handle {
  position: absolute;
  top: -3px; left: 0; right: 0;
  height: 6px;
  cursor: ns-resize;
  z-index: 2;
}
.resize-handle:hover { background: rgba(var(--accent-rgb),0.25); }

.log-hdr {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 14px 5px 8px;
  background: var(--bg1);
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.collapse-btn {
  background: none;
  border: none;
  cursor: pointer;
  padding: 2px 4px;
  color: var(--text2);
}
.arrow { font-size: 8px; display: inline-block; transition: transform 0.2s; }
.arrow.open { transform: rotate(90deg); }

.log-title {
  font-size: 11px;
  font-weight: 500;
  color: var(--text1);
}

.log-count {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--text2);
  background: var(--bg3);
  padding: 1px 6px;
  border-radius: 3px;
}

.level-filters { display: flex; gap: 4px; margin-left: 6px; }
.lvl-chip {
  font-family: var(--mono);
  font-size: 10px;
  font-weight: 500;
  padding: 1px 7px;
  border-radius: 99px;
  border: 1px solid var(--chip);
  color: var(--chip);
  background: transparent;
  cursor: pointer;
  transition: opacity 0.15s;
}
.lvl-chip.off { opacity: 0.35; border-style: dashed; }
.lvl-n { opacity: 0.7; }

.log-actions {
  display: flex;
  gap: 4px;
  margin-left: auto;
}

.log-body {
  flex: 1;
  overflow-y: auto;
  padding: 3px 0;
  font-family: var(--mono);
  font-size: 11px;
  line-height: 1.8;
  min-height: 0;
}

.log-empty {
  padding: 10px 14px;
  color: var(--text2);
  font-size: 11px;
}

.log-line {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 0 14px;
  transition: background 0.1s;
}
.log-line:hover { background: var(--bg1); }

.log-ts {
  color: var(--text3);
  min-width: 82px;
  flex-shrink: 0;
  font-size: 10px;
}

.log-level {
  min-width: 34px;
  flex-shrink: 0;
  font-size: 10px;
  font-weight: 500;
}

.log-msg { color: var(--text1); user-select: text; }
.log-msg.is-err { color: var(--text0); }

/* active button style */
.btn.active {
  color: var(--accent);
  border-color: rgba(var(--accent-rgb),0.3);
  background: rgba(var(--accent-rgb),0.06);
}
</style>
