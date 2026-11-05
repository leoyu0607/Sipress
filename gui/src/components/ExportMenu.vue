<script setup lang="ts">
/**
 * 標題列「匯出」選單：取代原本一排 5 個「↓ xxx」按鈕。
 * - 不能匯出的項目仍列出，並直接寫出原因（不只放在 hover 提示裡）
 * - 鍵盤：開啟後焦點在第一個可用項目；↑↓ / Home / End 移動，Esc 關閉並回到按鈕，Tab 關閉
 * - 點選單外面關閉
 */
import { computed, nextTick, onUnmounted, ref } from 'vue'
import { useTestStore } from '../stores/testStore'
import Icon from './Icon.vue'

const store = useTestStore()

const open     = ref(false)
const rootRef  = ref<HTMLElement | null>(null)
const trigger  = ref<HTMLButtonElement | null>(null)
const menuRef  = ref<HTMLElement | null>(null)

const hasSeries = computed(() => store.series.cps.length > 0)
const hasReport = computed(() => store.status === 'done')

interface Item { key: string; label: string; desc: string; reason: string; enabled: boolean; run: () => unknown }

const items = computed<Item[]>(() => [
  { key: 'json', label: 'JSON 報告', desc: '設定與每秒結果（不含密碼）', reason: '尚無測試資料',
    enabled: hasSeries.value, run: () => store.exportJson() },
  { key: 'csv', label: 'CSV 時序', desc: '每秒時序數據', reason: '尚無測試資料',
    enabled: hasSeries.value, run: () => store.exportCsv() },
  { key: 'html', label: 'HTML 報告', desc: '圖表報告，可離線檢視', reason: '測試完成後可匯出',
    enabled: hasReport.value, run: () => store.exportHtml() },
  { key: 'result', label: '壓測數據', desc: '含 10 秒時間軸，供整合報告使用', reason: '測試完成後可匯出',
    enabled: hasReport.value, run: () => store.exportResult() },
  { key: 'e2e', label: '端到端對照', desc: '民眾端 ↔ 座席端配對：漏接率、分配延遲、雙向音質',
    reason: '民眾端完成且座席端執行過後可匯出（例如雙端啟動）',
    enabled: store.canExportE2e, run: () => store.exportE2eHtml() },
])
const anyEnabled = computed(() => items.value.some(i => i.enabled))

function enabledButtons() {
  return [...(menuRef.value?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [])]
}

function onDocPointer(e: PointerEvent) {
  if (rootRef.value && !rootRef.value.contains(e.target as Node)) close(false)
}

async function show() {
  open.value = true
  document.addEventListener('pointerdown', onDocPointer, true)
  await nextTick()
  enabledButtons()[0]?.focus()
}
function close(returnFocus = true) {
  if (!open.value) return
  open.value = false
  document.removeEventListener('pointerdown', onDocPointer, true)
  if (returnFocus) trigger.value?.focus()
}
function toggle() { open.value ? close() : show() }

function choose(item: Item) {
  if (!item.enabled) return
  close()
  item.run()
}

function onMenuKey(e: KeyboardEvent) {
  const btns = enabledButtons()
  if (btns.length === 0) return
  const i = btns.indexOf(document.activeElement as HTMLButtonElement)
  const focus = (n: number) => { e.preventDefault(); btns[(n + btns.length) % btns.length].focus() }
  switch (e.key) {
    case 'ArrowDown': focus(i + 1); break
    case 'ArrowUp':   focus(i - 1); break
    case 'Home':      focus(0); break
    case 'End':       focus(btns.length - 1); break
    case 'Escape':    e.preventDefault(); close(); break
    case 'Tab':       close(false); break
  }
}
function onTriggerKey(e: KeyboardEvent) {
  if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && anyEnabled.value && !open.value) {
    e.preventDefault()
    show()
  }
}

onUnmounted(() => document.removeEventListener('pointerdown', onDocPointer, true))
</script>

<template>
  <div class="export-menu" ref="rootRef">
    <button ref="trigger" class="btn btn-sm" type="button"
            aria-haspopup="menu" :aria-expanded="open" aria-controls="export-menu-list"
            :disabled="!anyEnabled"
            :title="anyEnabled ? '匯出報告或數據' : '尚無測試資料'"
            @click="toggle" @keydown="onTriggerKey">
      <Icon name="download" :size="13" />匯出<Icon name="chevron-down" :size="12" class="caret" :class="{ open }" />
    </button>

    <div v-if="open" id="export-menu-list" ref="menuRef" class="menu" role="menu" aria-label="匯出" @keydown="onMenuKey">
      <button v-for="it in items" :key="it.key" type="button" role="menuitem" class="item"
              :disabled="!it.enabled" @click="choose(it)">
        <span class="item-label">{{ it.label }}</span>
        <span class="item-desc">{{ it.enabled ? it.desc : it.reason }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.export-menu { position: relative; display: inline-flex; }
.caret { transition: transform 0.15s; margin-left: -1px; }
.caret.open { transform: rotate(180deg); }

.menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: 50;
  min-width: 260px;
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
.item-label { font-size: 12px; font-weight: 500; }
.item-desc  { font-size: 11px; color: var(--text2); line-height: 1.4; }
</style>
