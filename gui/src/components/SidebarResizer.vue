<script lang="ts">
// 給 App.vue 用的常數與讀取函式（<script setup> 不能 export）
export const SIDEBAR_MIN = 240
export const SIDEBAR_MAX = 440
export const SIDEBAR_DEFAULT = 260
/** 主內容區至少保留的寬度：側欄再寬，圖表也不能被擠到不能用 */
export const CONTENT_MIN = 620
const WIDTH_KEY = 'sipress-sidebar-width'

/** 依目前視窗寬度可用的最大側欄寬度（960px 視窗 → 340px；1280px → 440px） */
export function sidebarMaxFor(viewportWidth: number): number {
  return Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, viewportWidth - CONTENT_MIN))
}
const clamp = (w: number, max = SIDEBAR_MAX) => Math.round(Math.min(max, Math.max(SIDEBAR_MIN, w)))

/** 讀回上次的寬度；沒有或讀不到時用預設值 */
export function loadSidebarWidth(): number {
  try {
    const v = Number(localStorage.getItem(WIDTH_KEY))
    return v ? clamp(v) : SIDEBAR_DEFAULT
  } catch {
    return SIDEBAR_DEFAULT
  }
}
</script>

<script setup lang="ts">
/**
 * 側欄寬度調整把手（放在側欄與主內容之間）。
 * - 滑鼠拖曳調整，放開時記住寬度（localStorage，與事件記錄高度相同做法）
 * - 鍵盤：Tab 移到把手後 ← → 每次 16px、Home / End 到最小 / 最大
 * - 雙擊恢復預設寬度
 */
import { computed, onUnmounted, ref } from 'vue'

const STEP = 16
const width = defineModel<number>({ required: true })
/** 目前可用的最大寬度（由 App 依視窗寬度傳入） */
const props = withDefaults(defineProps<{ max?: number }>(), { max: SIDEBAR_MAX })
const dragging = ref(false)
/** 畫面上實際的寬度（存的寬度可能大於目前視窗允許的上限） */
const shown = computed(() => Math.min(width.value, props.max))

// 存「剛算出來的值」而不是讀回 width.value：v-model 的值要等父元件重繪後才更新，
// 立刻讀回會拿到上一次的寬度
function save(w: number) {
  try { localStorage.setItem(WIDTH_KEY, String(w)) } catch { /* 私密模式等 */ }
}
function set(w: number) {
  const v = clamp(w, props.max)
  width.value = v
  save(v)
}

let startX = 0
let startW = 0
let lastW = 0
function onMove(e: MouseEvent) {
  lastW = clamp(startW + (e.clientX - startX), props.max)
  width.value = lastW
}
function onUp() {
  if (!dragging.value) return
  dragging.value = false
  window.removeEventListener('mousemove', onMove)
  window.removeEventListener('mouseup', onUp)
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
  save(lastW)
}
function onDown(e: MouseEvent) {
  startX = e.clientX
  startW = shown.value
  lastW = startW
  dragging.value = true
  document.body.style.cursor = 'col-resize'
  document.body.style.userSelect = 'none'   // 拖曳時不要選到文字
  window.addEventListener('mousemove', onMove)
  window.addEventListener('mouseup', onUp)
}
function onKey(e: KeyboardEvent) {
  const next =
      e.key === 'ArrowLeft'  ? shown.value - STEP
    : e.key === 'ArrowRight' ? shown.value + STEP
    : e.key === 'Home'       ? SIDEBAR_MIN
    : e.key === 'End'        ? props.max
    : null
  if (next === null) return
  e.preventDefault()
  set(next)
}

onUnmounted(onUp)
</script>

<template>
  <div class="sidebar-resizer" :class="{ dragging }"
       role="separator" aria-orientation="vertical" aria-label="側欄寬度"
       :aria-valuenow="shown" :aria-valuemin="SIDEBAR_MIN" :aria-valuemax="max"
       tabindex="0" title="拖曳調整側欄寬度（雙擊恢復預設）"
       @mousedown.prevent="onDown" @dblclick="set(SIDEBAR_DEFAULT)" @keydown="onKey"></div>
</template>

<style scoped>
/* 疊在側欄右邊框上，不佔版面寬度 */
.sidebar-resizer {
  position: relative;
  z-index: 3;
  flex-shrink: 0;
  width: 6px;
  margin: 0 -3px;
  cursor: col-resize;
  outline: none;
}
.sidebar-resizer::after {
  content: '';
  position: absolute;
  inset: 0 2px;
  transition: background 0.15s;
}
.sidebar-resizer:hover::after,
.sidebar-resizer.dragging::after { background: rgba(var(--accent-rgb), 0.35); }
.sidebar-resizer:focus-visible::after { background: rgba(var(--accent-rgb), 0.6); }
</style>
