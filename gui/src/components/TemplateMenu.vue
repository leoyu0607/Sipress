<script setup lang="ts">
/**
 * 標題列「範本」選單：套用範本、存成範本、管理（匯入 / 匯出 / 刪除）。
 * 按鈕顯示目前使用的範本；設定改過、尚未更新範本時加上「已修改」。
 * 鍵盤操作與「匯出」選單相同：↑↓ / Home / End 移動，Esc 關閉，Tab 關閉。
 */
import { computed, nextTick, onUnmounted, ref } from 'vue'
import { useTestStore, builtinTemplates, type TestTemplate } from '../stores/testStore'
import TemplateDialog from './TemplateDialog.vue'
import Icon from './Icon.vue'

const store = useTestStore()

const open    = ref(false)
const rootRef = ref<HTMLElement | null>(null)
const trigger = ref<HTMLButtonElement | null>(null)
const menuRef = ref<HTMLElement | null>(null)
const dialog  = ref<'save' | 'manage' | null>(null)
const error   = ref('')

const builtins = builtinTemplates()
const running  = computed(() => store.anyRunning)

const label = computed(() => store.activeTemplate ? store.activeTemplate.name : '範本')
const triggerTitle = computed(() => {
  const t = store.activeTemplate
  if (!t) return '範本：把常用設定存起來，下次一鍵套用'
  return `目前使用範本「${t.name}」${store.templateModified ? '（設定已修改，尚未更新範本）' : ''}${t.description ? `\n${t.description}` : ''}`
})

const fmtDate = (iso: string) => {
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? '' : `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`
}

function items() {
  return [...(menuRef.value?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [])]
}
function onDocPointer(e: PointerEvent) {
  if (rootRef.value && !rootRef.value.contains(e.target as Node)) close(false)
}
async function show() {
  error.value = ''
  open.value = true
  document.addEventListener('pointerdown', onDocPointer, true)
  await nextTick()
  items()[0]?.focus()
}
function close(returnFocus = true) {
  if (!open.value) return
  open.value = false
  document.removeEventListener('pointerdown', onDocPointer, true)
  if (returnFocus) trigger.value?.focus()
}
function toggle() { open.value ? close() : show() }

function apply(t: TestTemplate) {
  const err = store.applyTemplate(t)
  if (err) { error.value = err; return }
  close()
}
function undo() {
  store.undoApplyTemplate()
  close()
}
function update() {
  store.updateActiveTemplate()
  close()
}
function openDialog(kind: 'save' | 'manage') {
  close(false)
  dialog.value = kind
}

function onMenuKey(e: KeyboardEvent) {
  const btns = items()
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
  if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !open.value) {
    e.preventDefault()
    show()
  }
}

onUnmounted(() => document.removeEventListener('pointerdown', onDocPointer, true))
</script>

<template>
  <div class="tpl-menu" ref="rootRef">
    <button ref="trigger" class="btn btn-sm trigger" type="button"
            aria-haspopup="menu" :aria-expanded="open" aria-controls="tpl-menu-list"
            :title="triggerTitle" @click="toggle" @keydown="onTriggerKey">
      <Icon name="layers" :size="13" />
      <span class="trigger-label">{{ label }}</span>
      <span v-if="store.templateModified" class="modified" aria-label="已修改">已修改</span>
      <Icon name="chevron-down" :size="12" class="caret" :class="{ open }" />
    </button>

    <div v-if="open" id="tpl-menu-list" ref="menuRef" class="menu" role="menu" aria-label="範本" @keydown="onMenuKey">
      <div v-if="running" class="note">測試執行中，停止後才能套用範本</div>
      <div v-if="error" class="note err">{{ error }}</div>

      <button v-if="store.templateUndo" type="button" role="menuitem" class="item undo"
              :disabled="running" @click="undo">
        <span class="item-label"><Icon name="rotate-ccw" :size="12" />復原</span>
        <span class="item-desc">回到套用「{{ store.templateUndo.name }}」之前的設定</span>
      </button>

      <div class="group">我的範本</div>
      <div v-if="store.templates.length === 0" class="empty">
        還沒有範本。設定好之後按「把目前設定存成範本」，下次一鍵套用。
      </div>
      <button v-for="t in store.templates" :key="t.id" type="button" role="menuitem" class="item"
              :class="{ active: store.activeTemplate?.id === t.id }" :disabled="running" @click="apply(t)">
        <span class="item-label">
          <Icon v-if="store.activeTemplate?.id === t.id" name="check" :size="12" class="check" />{{ t.name }}
          <span class="date">{{ fmtDate(t.updatedAt) }}</span>
        </span>
        <span class="item-desc">{{ t.description || '（沒有說明）' }}</span>
      </button>

      <div class="group">內建情境 <span class="group-hint">保留目前的伺服器、號碼與帳號</span></div>
      <button v-for="t in builtins" :key="t.id" type="button" role="menuitem" class="item"
              :disabled="running" @click="apply(t)">
        <span class="item-label">{{ t.name }}</span>
        <span class="item-desc">{{ t.description }}</span>
      </button>

      <div class="divider" role="separator"></div>
      <button type="button" role="menuitem" class="item action" @click="openDialog('save')">
        <span class="item-label"><Icon name="save" :size="12" />把目前設定存成範本…</span>
      </button>
      <button v-if="store.activeTemplate" type="button" role="menuitem" class="item action"
              :disabled="!store.templateModified" @click="update">
        <span class="item-label"><Icon name="refresh" :size="12" />更新範本「{{ store.activeTemplate.name }}」</span>
        <span class="item-desc">{{ store.templateModified ? '用目前的設定覆蓋這個範本' : '設定與範本相同，不需要更新' }}</span>
      </button>
      <button type="button" role="menuitem" class="item action" @click="openDialog('manage')">
        <span class="item-label"><Icon name="settings" :size="12" />管理範本…</span>
        <span class="item-desc">改名、刪除、匯出給同事、匯入</span>
      </button>
    </div>

    <TemplateDialog :mode="dialog" @close="dialog = null" />
  </div>
</template>

<style scoped>
.tpl-menu { position: relative; display: inline-flex; }
.trigger { max-width: 220px; }
.trigger-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.modified {
  font-size: 10px; color: var(--warn, #d29922);
  border: 1px solid currentColor; border-radius: 8px; padding: 0 5px; flex-shrink: 0;
}
.caret { transition: transform 0.15s; margin-left: -1px; flex-shrink: 0; }
.caret.open { transform: rotate(180deg); }

.menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: 50;
  width: 320px;
  max-height: min(70vh, 560px);
  overflow-y: auto;
  padding: 4px;
  background: var(--bg2);
  border: 1px solid var(--border3);
  border-radius: var(--radius);
  box-shadow: 0 10px 28px rgba(0,0,0,0.35);
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.group {
  font-size: 11px; color: var(--text2); font-weight: 500;
  padding: 8px 10px 3px;
}
.group-hint { font-weight: normal; margin-left: 4px; }
.empty { font-size: 11px; color: var(--text2); padding: 4px 10px 6px; line-height: 1.5; }
.note { font-size: 11px; color: var(--text1); padding: 6px 10px; }
.note.err { color: var(--danger); }
.divider { height: 1px; background: var(--border2); margin: 4px 2px; }

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
.item.active .item-label { color: var(--accent); }
.item-label { font-size: 12px; font-weight: 500; display: flex; align-items: center; gap: 5px; width: 100%; }
.item-desc  { font-size: 11px; color: var(--text2); line-height: 1.4; }
.date { margin-left: auto; font-size: 10px; color: var(--text2); font-weight: normal; font-family: var(--mono); }
.check { flex-shrink: 0; }
.undo .item-label { color: var(--accent); }

/* 窄視窗（最小 960px）：範本名稱先縮短，其他按鈕不被擠掉 */
@media (max-width: 1100px) {
  .trigger { max-width: 150px; }
}
</style>
