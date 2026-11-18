<script setup lang="ts">
/**
 * 標題列的開始按鈕：原本「開始測試」與「雙端啟動」兩顆並排，改成一顆主按鈕 + 旁邊的 ▾。
 * - 主按鈕：開始 / 停止目前分頁那一端；兩端同時執行時改為「全部停止」
 * - ▾ 選單：兩端一起跑（先座席註冊再撥號）、全部停止
 * 不能開始的原因寫在選單裡與按鈕提示，側欄上方也會列出。
 */
import { computed, nextTick, onUnmounted, ref } from 'vue'
import { useTestStore, MODE_LABEL } from '../stores/testStore'
import Icon from './Icon.vue'

const store = useTestStore()

const open    = ref(false)
const rootRef = ref<HTMLElement | null>(null)
const caret   = ref<HTMLButtonElement | null>(null)
const menuRef = ref<HTMLElement | null>(null)

const bothRunning = computed(() =>
  store.sessions.caller.status === 'running' && store.sessions.agent.status === 'running'
)
const side = computed(() => MODE_LABEL[store.config.mode])

/** 主按鈕要做的事 */
const action = computed<'stop-all' | 'stop' | 'start'>(() =>
  bothRunning.value ? 'stop-all' : store.status === 'running' ? 'stop' : 'start'
)
const label = computed(() =>
  store.stopping ? '停止中…'
  : action.value === 'stop-all' ? '全部停止'
  : action.value === 'stop' ? '停止測試'
  : '開始測試'
)
const blocked = computed(() => action.value === 'start' && store.validationErrors.length > 0)
const title = computed(() =>
  blocked.value ? [`無法開始${side.value}：`, ...store.validationErrors.map(e => `• ${e}`)].join('\n')
  : action.value === 'stop-all' ? '停止民眾端與座席端'
  : action.value === 'stop' ? `停止${side.value}`
  : `開始${side.value}測試；要兩端一起跑請按旁邊的 ▾`
)
const mainClass = computed(() =>
  action.value === 'start' ? 'btn btn-primary start-btn' : 'btn btn-primary is-danger start-btn'
)

function runMain() {
  if (action.value === 'stop-all') store.stopAll()
  else if (action.value === 'stop') store.stopTest()
  else store.startTest()
}

const bothBlocked = computed(() => store.bothValidationErrors.length > 0)

function items() {
  return [...(menuRef.value?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [])]
}
function onDocPointer(e: PointerEvent) {
  if (rootRef.value && !rootRef.value.contains(e.target as Node)) close(false)
}
async function show() {
  open.value = true
  document.addEventListener('pointerdown', onDocPointer, true)
  await nextTick()
  items()[0]?.focus()
}
function close(returnFocus = true) {
  if (!open.value) return
  open.value = false
  document.removeEventListener('pointerdown', onDocPointer, true)
  if (returnFocus) caret.value?.focus()
}
function toggle() { open.value ? close() : show() }

function startBoth() { close(); store.startBoth() }
function stopAll()   { close(); store.stopAll() }

function onMenuKey(e: KeyboardEvent) {
  const btns = items()
  if (e.key === 'Escape') { e.preventDefault(); close(); return }
  if (e.key === 'Tab') { close(false); return }
  if (btns.length === 0) return
  const i = btns.indexOf(document.activeElement as HTMLButtonElement)
  const focus = (n: number) => { e.preventDefault(); btns[(n + btns.length) % btns.length].focus() }
  if (e.key === 'ArrowDown') focus(i + 1)
  else if (e.key === 'ArrowUp') focus(i - 1)
}
function onCaretKey(e: KeyboardEvent) {
  if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !open.value) {
    e.preventDefault()
    show()
  }
}

onUnmounted(() => document.removeEventListener('pointerdown', onDocPointer, true))
</script>

<template>
  <div class="start-split" ref="rootRef">
    <span class="start-wrap" :title="title">
      <button :class="mainClass" :disabled="blocked || store.stopping" @click="runMain">
        <Icon :name="action === 'start' ? 'play' : 'stop'" :size="13" />{{ label }}
      </button>
    </span>
    <button ref="caret" type="button" class="btn btn-primary caret-btn" :class="{ 'is-danger': action !== 'start' }"
            aria-haspopup="menu" :aria-expanded="open" aria-controls="start-menu-list" aria-label="更多執行方式"
            title="兩端一起跑、全部停止" @click="toggle" @keydown="onCaretKey">
      <Icon name="chevron-down" :size="12" class="caret" :class="{ open }" />
    </button>

    <div v-if="open" id="start-menu-list" ref="menuRef" class="menu" role="menu" aria-label="執行方式" @keydown="onMenuKey">
      <button type="button" role="menuitem" class="item" :disabled="store.anyRunning || bothBlocked" @click="startBoth">
        <span class="item-label"><Icon name="arrow-left-right" :size="12" />兩端一起跑</span>
        <span class="item-desc" v-if="store.anyRunning">有一端正在執行，停止後才能兩端一起跑</span>
        <span class="item-desc" v-else-if="!bothBlocked">先啟動座席端註冊，再啟動民眾端撥號</span>
        <template v-else>
          <span class="item-desc">開始前需修正：</span>
          <span class="item-desc err" v-for="e in store.bothValidationErrors" :key="e">• {{ e }}</span>
        </template>
      </button>
      <button type="button" role="menuitem" class="item" :disabled="!store.anyRunning" @click="stopAll">
        <span class="item-label"><Icon name="stop" :size="12" />全部停止</span>
        <span class="item-desc">{{ store.anyRunning ? '停止民眾端與座席端' : '目前沒有執行中的測試' }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.start-split { position: relative; display: inline-flex; }
.start-wrap { display: inline-flex; }
.start-btn {
  min-width: 96px; justify-content: center;
  border-top-right-radius: 0; border-bottom-right-radius: 0;
}
.caret-btn {
  padding-left: 6px; padding-right: 6px;
  border-top-left-radius: 0; border-bottom-left-radius: 0;
  border-left: 1px solid rgba(0,0,0,0.2);
}
.caret { transition: transform 0.15s; }
.caret.open { transform: rotate(180deg); }

.menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: 50;
  width: 280px;
  padding: 4px;
  background: var(--bg2);
  border: 1px solid var(--border3);
  border-radius: var(--radius);
  box-shadow: 0 10px 28px rgba(0,0,0,0.35);
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.item {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text0);
  text-align: left;
  cursor: pointer;
  font-family: var(--sans);
}
.item:hover:not(:disabled), .item:focus-visible { background: var(--bg3); outline: none; }
.item:focus-visible { box-shadow: inset 0 0 0 2px rgba(var(--accent-rgb),0.6); }
.item:disabled { cursor: not-allowed; }
.item:disabled .item-label { color: var(--text2); }
.item-label { font-size: 12px; font-weight: 500; display: flex; align-items: center; gap: 5px; }
.item-desc  { font-size: 11px; color: var(--text2); line-height: 1.4; }
.item-desc.err { color: var(--danger); }
</style>
