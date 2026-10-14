<script setup lang="ts">
import { ref, computed, watch, nextTick, onUnmounted } from 'vue'

interface GenerateOptions {
  count: number
  startExt: number
  usernamePrefix: string
  passwordMode: 'same' | 'ext' | 'custom'
  samePassword: string
  customPassword: string
  domain: string
}

const props = defineProps<{
  visible: boolean
  defaultDomain: string
}>()

const emit = defineEmits<{
  confirm: [opts: GenerateOptions]
  cancel: []
}>()

// ── Form state ────────────────────────────────────────────────────
const count          = ref(5)
const startExt       = ref(1001)
const usernamePrefix = ref('')
const passwordMode   = ref<'same' | 'ext' | 'custom'>('same')
const samePassword   = ref('1234')
const customPassword = ref('')
const domain         = ref(props.defaultDomain)

// Sync domain when prop changes
watch(() => props.defaultDomain, v => { domain.value = v })

const firstInput = ref<HTMLInputElement | null>(null)
const MAX_COUNT  = 500

// Reset when opened; focus first field; Esc/Enter work regardless of focus
watch(() => props.visible, v => {
  if (!v) { window.removeEventListener('keydown', handleKeydown); return }
  window.addEventListener('keydown', handleKeydown)
  nextTick(() => { firstInput.value?.focus(); firstInput.value?.select() })
  count.value          = 5
  startExt.value       = 1001
  usernamePrefix.value = ''
  passwordMode.value   = 'same'
  samePassword.value   = '1234'
  customPassword.value = ''
  domain.value         = props.defaultDomain
})

// ── Preview ────────────────────────────────────────────────────────
const preview = computed(() => {
  const rows = []
  const n = Math.min(count.value, 4)
  for (let i = 0; i < n; i++) {
    const ext  = startExt.value + i
    const user = usernamePrefix.value ? `${usernamePrefix.value}${ext}` : String(ext)
    const pass = passwordMode.value === 'ext'    ? String(ext)
               : passwordMode.value === 'custom' ? customPassword.value.replaceAll('{ext}', String(ext))
               : samePassword.value
    rows.push({ ext, user, pass, domain: domain.value })
  }
  if (count.value > 4) rows.push(null) // ellipsis row
  return rows
})

const isValid = computed(() =>
  count.value >= 1 && count.value <= MAX_COUNT &&
  startExt.value >= 1 &&
  domain.value.trim() !== '' &&
  (passwordMode.value !== 'same'   || samePassword.value !== '') &&
  (passwordMode.value !== 'custom' || customPassword.value !== '')
)

function handleConfirm() {
  if (!isValid.value) return
  emit('confirm', {
    count:          count.value,
    startExt:       startExt.value,
    usernamePrefix: usernamePrefix.value,
    passwordMode:   passwordMode.value,
    samePassword:   samePassword.value,
    customPassword: customPassword.value,
    domain:         domain.value,
  })
}

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') emit('cancel')
  if (e.key === 'Enter' && isValid.value) handleConfirm()
}
onUnmounted(() => window.removeEventListener('keydown', handleKeydown))
</script>

<template>
  <Teleport to="body">
    <Transition name="overlay">
      <div v-if="visible" class="overlay" @click.self="emit('cancel')">
        <Transition name="dialog">
          <div v-if="visible" class="dialog" role="dialog" aria-modal="true" aria-labelledby="gen-dlg-title">

            <!-- Header -->
            <div class="dialog-hdr">
              <span class="dialog-title" id="gen-dlg-title">快速生成座席帳號</span>
              <button class="close-btn" @click="emit('cancel')" aria-label="關閉">×</button>
            </div>

            <!-- Body -->
            <div class="dialog-body">

              <!-- Row: count + start ext -->
              <div class="field-row">
                <div class="field">
                  <label>生成數量</label>
                  <input ref="firstInput" v-model.number="count" type="number" min="1" :max="MAX_COUNT" class="field-input" :class="{ invalid: count > MAX_COUNT || count < 1 }">
                  <div class="field-err" v-if="count > MAX_COUNT">單次最多 {{ MAX_COUNT }} 個</div>
                </div>
                <div class="field">
                  <label>起始分機號</label>
                  <input v-model.number="startExt" type="number" min="1" class="field-input">
                </div>
              </div>

              <!-- Username prefix -->
              <div class="field">
                <label>
                  帳號前綴
                  <span class="label-hint">留空則帳號 = 分機號</span>
                </label>
                <div class="prefix-preview-row">
                  <input v-model="usernamePrefix" type="text" class="field-input" placeholder="例：agent_">
                  <div class="prefix-example">
                    → <code>{{ usernamePrefix || startExt }}{{ usernamePrefix ? startExt : '' }}</code>
                  </div>
                </div>
              </div>

              <!-- Password mode -->
              <div class="field">
                <label>密碼設定</label>
                <div class="radio-group">
                  <label class="radio-opt" :class="{ active: passwordMode === 'same' }">
                    <input type="radio" v-model="passwordMode" value="same">
                    <span class="radio-label">統一密碼</span>
                  </label>
                  <label class="radio-opt" :class="{ active: passwordMode === 'ext' }">
                    <input type="radio" v-model="passwordMode" value="ext">
                    <span class="radio-label">密碼 = 分機號</span>
                  </label>
                  <label class="radio-opt" :class="{ active: passwordMode === 'custom' }">
                    <input type="radio" v-model="passwordMode" value="custom">
                    <span class="radio-label">自訂規則</span>
                  </label>
                </div>
              </div>

              <!-- Password input (conditional) -->
              <div class="field" v-if="passwordMode === 'same'">
                <label>統一密碼</label>
                <input v-model="samePassword" type="text" class="field-input"
                       placeholder="輸入所有帳號共用密碼">
              </div>

              <div class="field" v-if="passwordMode === 'custom'">
                <label>
                  密碼規則
                  <span class="label-hint"><code>{ext}</code> 代入分機號</span>
                </label>
                <input v-model="customPassword" type="text" class="field-input"
                       placeholder="例：pass{ext} → pass1001">
                <div class="field-hint" v-if="customPassword">
                  預覽：<code>{{ customPassword.replaceAll('{ext}', String(startExt)) }}</code>
                </div>
              </div>

              <!-- Domain -->
              <div class="field">
                <label>SIP Domain</label>
                <input v-model="domain" type="text" class="field-input" placeholder="192.168.1.100">
              </div>

              <!-- Preview table -->
              <div class="preview-section">
                <div class="preview-label">預覽（前 {{ Math.min(count, 4) }} 筆）</div>
                <div class="preview-table">
                  <div class="preview-hdr">
                    <span class="pc ext">分機</span>
                    <span class="pc user">帳號</span>
                    <span class="pc pass">密碼</span>
                    <span class="pc dom">Domain</span>
                  </div>
                  <div v-for="(row, i) in preview" :key="i">
                    <div v-if="row" class="preview-row">
                      <span class="pc ext">{{ row.ext }}</span>
                      <span class="pc user">{{ row.user }}</span>
                      <span class="pc pass">{{ row.pass }}</span>
                      <span class="pc dom">{{ row.domain }}</span>
                    </div>
                    <div v-else class="preview-ellipsis">
                      ··· 共 {{ count }} 筆
                    </div>
                  </div>
                </div>
              </div>

            </div>

            <!-- Footer -->
            <div class="dialog-footer">
              <button class="btn btn-sm" @click="emit('cancel')">取消</button>
              <button class="btn btn-sm btn-accent" :disabled="!isValid" @click="handleConfirm">
                生成 {{ count }} 個帳號
              </button>
            </div>

          </div>
        </Transition>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* ── Overlay ───────────────────────────────────────────────────── */
.overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
  backdrop-filter: blur(2px);
}

/* ── Dialog box ────────────────────────────────────────────────── */
.dialog {
  background: var(--bg1);
  border: 1px solid var(--border3);
  border-radius: var(--radius-lg);
  width: 420px;
  max-width: calc(100vw - 40px);
  max-height: calc(100vh - 80px);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
}

/* ── Header ────────────────────────────────────────────────────── */
.dialog-hdr {
  display: flex;
  align-items: center;
  padding: 14px 16px 12px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}
.dialog-title {
  font-size: 13px;
  font-weight: 500;
  color: var(--text0);
  flex: 1;
}
.close-btn {
  background: none;
  border: none;
  color: var(--text2);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
  padding: 0 2px;
  transition: color 0.15s;
}
.close-btn:hover { color: var(--text0); }

/* ── Body ──────────────────────────────────────────────────────── */
.dialog-body {
  padding: 14px 16px;
  overflow-y: auto;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

/* ── Fields ────────────────────────────────────────────────────── */
.field { display: flex; flex-direction: column; gap: 4px; }
.field-row { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }

.field label {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: var(--text1);
}
.label-hint {
  font-size: 10px;
  color: var(--text2);
  font-family: var(--mono);
}

.field-input {
  background: var(--bg0);
  border: 1px solid var(--border2);
  border-radius: var(--radius-sm);
  padding: 6px 9px;
  color: var(--text0);
  font-family: var(--mono);
  font-size: 12px;
  outline: none;
  transition: border-color 0.15s;
  width: 100%;
}
.field-input:hover { border-color: var(--border3); }
.field-input:focus { border-color: rgba(var(--accent-rgb),0.6); }
.field-input.invalid { border-color: rgba(var(--danger-rgb),0.6); }
.field-err { font-size: 10px; color: var(--danger); }

.field-hint {
  font-size: 10px;
  color: var(--text2);
  font-family: var(--mono);
}
.field-hint code { color: var(--accent); }

.prefix-preview-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.prefix-preview-row .field-input { flex: 1; }
.prefix-example {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text2);
  white-space: nowrap;
  flex-shrink: 0;
}
.prefix-example code { color: var(--accent); }

/* ── Radio group ───────────────────────────────────────────────── */
.radio-group {
  display: flex;
  gap: 6px;
}
.radio-opt {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 5px 10px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border2);
  cursor: pointer;
  transition: all 0.15s;
  flex: 1;
  justify-content: center;
}
.radio-opt:hover { border-color: var(--border3); background: var(--bg3); }
.radio-opt.active {
  border-color: rgba(var(--accent-rgb),0.4);
  background: rgba(var(--accent-rgb),0.07);
  color: var(--accent);
}
.radio-opt input[type="radio"] { display: none; }
.radio-label {
  font-size: 11px;
  font-family: var(--mono);
  color: inherit;
  white-space: nowrap;
}

/* ── Preview table ─────────────────────────────────────────────── */
.preview-section {
  background: var(--bg0);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  overflow: hidden;
}
.preview-label {
  padding: 5px 10px;
  font-size: 10px;
  font-family: var(--mono);
  font-weight: 500;
  letter-spacing: 0.08em;
  color: var(--text2);
  text-transform: uppercase;
  border-bottom: 1px solid var(--border);
  background: var(--bg2);
}
.preview-table { font-family: var(--mono); font-size: 11px; }
.preview-hdr {
  display: flex;
  padding: 4px 10px;
  border-bottom: 1px solid var(--border);
  background: var(--bg2);
}
.preview-hdr .pc { color: var(--text2); font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; }
.preview-row {
  display: flex;
  padding: 4px 10px;
  border-bottom: 1px solid var(--border);
  transition: background 0.1s;
}
.preview-row:last-child { border-bottom: none; }
.preview-row:hover { background: var(--bg2); }
.preview-ellipsis {
  padding: 5px 10px;
  color: var(--text2);
  font-size: 11px;
  text-align: center;
}

/* Column widths */
.pc.ext  { width: 52px; flex-shrink: 0; color: var(--text0); }
.pc.user { flex: 1; color: var(--blue); }
.pc.pass { width: 80px; flex-shrink: 0; color: var(--text1); }
.pc.dom  { width: 100px; flex-shrink: 0; color: var(--text2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

/* ── Footer ────────────────────────────────────────────────────── */
.dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
  flex-shrink: 0;
}

/* ── Transitions ───────────────────────────────────────────────── */
.overlay-enter-active,
.overlay-leave-active { transition: opacity 0.2s ease; }
.overlay-enter-from,
.overlay-leave-to { opacity: 0; }

.dialog-enter-active,
.dialog-leave-active { transition: opacity 0.2s ease, transform 0.2s ease; }
.dialog-enter-from,
.dialog-leave-to { opacity: 0; transform: scale(0.96) translateY(-8px); }
</style>
