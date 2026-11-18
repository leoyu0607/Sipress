<script setup lang="ts">
/**
 * 範本視窗：
 * - save：把目前設定存成範本（同名會覆蓋）
 * - manage：改名 / 說明、套用、刪除、匯出（可選是否含密碼與金鑰）、匯入
 */
import { computed, ref, watch, onUnmounted } from 'vue'
import { useTestStore } from '../stores/testStore'
import Icon from './Icon.vue'

const props = defineProps<{ mode: 'save' | 'manage' | null }>()
const emit = defineEmits<{ close: [] }>()

const store = useTestStore()
const visible = computed(() => props.mode !== null)

// ── 存成範本 ────────────────────────────────────────────────────────
const name = ref('')
const description = ref('')
const withThresholds = ref(true)
const nameTaken = computed(() => store.templates.some(t => t.name === name.value.trim()))

// ── 管理 ────────────────────────────────────────────────────────────
const includeSecrets = ref(false)
const confirmDelete = ref<string | null>(null)
const importMsg = ref<{ ok: boolean; text: string } | null>(null)
const fileInput = ref<HTMLInputElement | null>(null)
const nameInput = ref<HTMLInputElement | null>(null)

watch(() => props.mode, m => {
  if (!m) { window.removeEventListener('keydown', onKeydown); return }
  window.addEventListener('keydown', onKeydown)
  importMsg.value = null
  confirmDelete.value = null
  includeSecrets.value = false
  if (m === 'save') {
    const t = store.activeTemplate
    name.value = t?.name ?? ''
    description.value = t?.description ?? ''
    withThresholds.value = t ? !!t.thresholds : true
    setTimeout(() => nameInput.value?.select(), 50)
  }
})
function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') emit('close')
}
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

function save() {
  if (!name.value.trim()) return
  store.saveAsTemplate(name.value, description.value, withThresholds.value)
  emit('close')
}

function apply(id: string) {
  const t = store.templates.find(x => x.id === id)
  if (!t) return
  const err = store.applyTemplate(t)
  if (err) importMsg.value = { ok: false, text: err }
  else emit('close')
}

function remove(id: string) {
  if (confirmDelete.value !== id) {
    confirmDelete.value = id
    return
  }
  confirmDelete.value = null
  store.deleteTemplate(id)
}

function exportOne(id: string) {
  store.exportTemplates([id], includeSecrets.value)
}
function exportAll() {
  store.exportTemplates(store.templates.map(t => t.id), includeSecrets.value)
}

function onFile(e: Event) {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  const reader = new FileReader()
  reader.onload = () => {
    try {
      const n = store.importTemplates(String(reader.result ?? ''))
      importMsg.value = { ok: true, text: `已匯入 ${n} 個範本` }
    } catch (err) {
      importMsg.value = { ok: false, text: `無法匯入：${err instanceof Error ? err.message : err}` }
    }
  }
  reader.onerror = () => { importMsg.value = { ok: false, text: '無法讀取檔案' } }
  reader.readAsText(file)
}

const fmtTime = (iso: string) => {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}/${p(d.getMonth() + 1)}/${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}
</script>

<template>
  <Teleport to="body">
    <Transition name="overlay">
      <div v-if="visible" class="overlay" @click.self="emit('close')">
        <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="tpl-dlg-title"
             :class="{ wide: mode === 'manage' }">
          <div class="dialog-hdr">
            <span class="dialog-title" id="tpl-dlg-title">{{ mode === 'save' ? '把目前設定存成範本' : '管理範本' }}</span>
            <button class="close-btn" @click="emit('close')" aria-label="關閉"><Icon name="x" :size="16" /></button>
          </div>

          <!-- 存成範本 -->
          <template v-if="mode === 'save'">
            <div class="dialog-body">
              <div class="intro">範本會記住目前所有設定：民眾端與座席端、通話流程、座席帳號，下次一鍵套用。</div>
              <label class="field">
                <span>名稱</span>
                <input ref="nameInput" v-model="name" class="in" type="text" placeholder="例如：嘉義 1999 每月例行測試" maxlength="60"
                       @keydown.enter.prevent="save">
              </label>
              <div v-if="nameTaken" class="hint warn">已有同名範本，儲存會覆蓋它</div>
              <label class="field">
                <span>說明（給執行的人看）</span>
                <textarea v-model="description" class="in area" rows="3"
                          placeholder="例如：每月第一週執行；先雙端啟動，接通率應 ≥ 99%"></textarea>
              </label>
              <label class="check-row">
                <input type="checkbox" v-model="withThresholds">
                一併記住整合報告的判定門檻（MOS、掉包率、Jitter、主機資源）
              </label>
              <div class="hint">密碼與 TTS 金鑰會存在這台電腦的範本裡；匯出給別人時可選擇不包含。</div>
            </div>
            <div class="dialog-footer">
              <span class="spacer"></span>
              <button class="btn btn-sm" @click="emit('close')">取消</button>
              <button class="btn btn-sm btn-accent" :disabled="!name.trim()" @click="save">
                {{ nameTaken ? '覆蓋範本' : '儲存' }}
              </button>
            </div>
          </template>

          <!-- 管理 -->
          <template v-else>
            <div class="dialog-body">
              <div v-if="store.templates.length === 0" class="empty">
                還沒有範本。先在主畫面設定好，再從「範本 → 把目前設定存成範本」建立；或匯入同事給的範本檔。
              </div>
              <div v-for="t in store.templates" :key="t.id" class="row"
                   :class="{ active: store.activeTemplate?.id === t.id }">
                <div class="row-main">
                  <input :value="t.name" class="in name" type="text" maxlength="60" aria-label="範本名稱"
                         @change="store.editTemplate(t.id, ($event.target as HTMLInputElement).value, t.description)">
                  <input :value="t.description" class="in desc" type="text" placeholder="說明（給執行的人看）" aria-label="範本說明"
                         @change="store.editTemplate(t.id, t.name, ($event.target as HTMLInputElement).value)">
                  <div class="meta">
                    <span v-if="store.activeTemplate?.id === t.id" class="badge">使用中</span>
                    更新於 {{ fmtTime(t.updatedAt) }}
                    <span v-if="t.thresholds">· 含判定門檻</span>
                  </div>
                </div>
                <div class="row-actions">
                  <button class="btn btn-xs" :disabled="store.anyRunning" @click="apply(t.id)">套用</button>
                  <button class="btn btn-xs" @click="exportOne(t.id)">匯出</button>
                  <button class="btn btn-xs" :class="{ danger: confirmDelete === t.id }" @click="remove(t.id)">
                    {{ confirmDelete === t.id ? '確定刪除？' : '刪除' }}
                  </button>
                </div>
              </div>

              <label class="check-row secret">
                <input type="checkbox" v-model="includeSecrets">
                匯出時包含密碼與 TTS 金鑰
              </label>
              <div class="hint" :class="{ warn: includeSecrets }">
                {{ includeSecrets
                  ? '匯出的檔案含座席密碼、撥號認證密碼與 TTS 金鑰，拿到檔案的人都能使用，請小心保管。'
                  : '預設不含密碼與金鑰：對方匯入後需自行填入。' }}
              </div>
              <div v-if="importMsg" class="hint" :class="importMsg.ok ? 'ok' : 'err'">{{ importMsg.text }}</div>
            </div>
            <div class="dialog-footer">
              <input ref="fileInput" type="file" accept=".json,application/json" class="hidden" @change="onFile">
              <button class="btn btn-sm" @click="fileInput?.click()"><Icon name="upload" :size="13" />匯入…</button>
              <button class="btn btn-sm" :disabled="store.templates.length === 0" @click="exportAll">
                <Icon name="download" :size="13" />全部匯出
              </button>
              <span class="spacer"></span>
              <button class="btn btn-sm" @click="emit('close')">關閉</button>
            </div>
          </template>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.overlay {
  position: fixed; inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: flex; align-items: center; justify-content: center;
  z-index: 1000;
  backdrop-filter: blur(2px);
}
.dialog {
  background: var(--bg1);
  border: 1px solid var(--border3);
  border-radius: var(--radius-lg);
  width: 440px;
  max-width: calc(100vw - 40px);
  max-height: calc(100vh - 60px);
  display: flex; flex-direction: column;
  overflow: hidden;
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
}
.dialog.wide { width: 620px; }
.dialog-hdr { display: flex; align-items: center; padding: 14px 16px 12px; border-bottom: 1px solid var(--border); }
.dialog-title { font-size: 13px; font-weight: 500; color: var(--text0); flex: 1; }
.close-btn {
  background: none; border: none; color: var(--text2);
  display: inline-flex; align-items: center; cursor: pointer; padding: 2px; border-radius: var(--radius-sm);
}
.close-btn:hover { color: var(--text0); }
.dialog-body { padding: 12px 16px; overflow-y: auto; flex: 1; display: flex; flex-direction: column; gap: 10px; }
.intro, .empty { font-size: 11px; color: var(--text2); line-height: 1.6; }
.field { display: flex; flex-direction: column; gap: 4px; font-size: 11px; color: var(--text1); }
.in {
  background: var(--bg0);
  border: 1px solid var(--border2);
  border-radius: var(--radius-sm);
  padding: 5px 8px;
  color: var(--text0);
  font-family: inherit;
  font-size: 12px;
  outline: none;
  min-width: 0;
}
.in:focus { border-color: rgba(var(--accent-rgb), 0.6); }
.area { resize: vertical; line-height: 1.5; }
.check-row { display: flex; align-items: center; gap: 6px; font-size: 11px; color: var(--text1); cursor: pointer; }
.check-row.secret { margin-top: 4px; }
.hint { font-size: 11px; color: var(--text2); line-height: 1.5; }
.hint.warn { color: var(--warn, #d29922); }
.hint.ok { color: var(--accent); }
.hint.err { color: var(--danger); }

.row {
  display: flex; gap: 8px; align-items: flex-start;
  padding: 8px;
  background: var(--bg2);
  border: 1px solid var(--border2);
  border-radius: var(--radius-sm);
}
.row.active { border-color: rgba(var(--accent-rgb), 0.45); }
.row-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
.in.name { font-weight: 500; }
.meta { font-size: 10px; color: var(--text2); display: flex; align-items: center; gap: 4px; }
.badge { color: var(--accent); border: 1px solid currentColor; border-radius: 8px; padding: 0 5px; }
.row-actions { display: flex; flex-direction: column; gap: 4px; flex-shrink: 0; }
.btn-xs { font-size: 11px; padding: 2px 10px; justify-content: center; }
.btn-xs.danger { color: var(--danger); border-color: rgba(var(--danger-rgb), 0.5); }
.hidden { display: none; }

.dialog-footer { display: flex; align-items: center; gap: 8px; padding: 12px 16px; border-top: 1px solid var(--border); }
.spacer { flex: 1; }

.overlay-enter-active, .overlay-leave-active { transition: opacity 0.2s ease; }
.overlay-enter-from, .overlay-leave-to { opacity: 0; }
</style>
