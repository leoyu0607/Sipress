<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from 'vue'
import {
  type AgentFlowConfig, type FlowPhase, type FlowCodeKey, type FlowStepConfig,
  FLOW_CODE_NAMES, FLOW_PHASE_NAMES,
  newFlowStep, sipxPresetFlow, expandFlowCode, flowErrors,
} from '../stores/testStore'

const props = defineProps<{
  visible: boolean
  flow: AgentFlowConfig
  /** 預覽代入用的分機（第一個帳號；沒有帳號時用範例） */
  sampleExt: string
}>()

const emit = defineEmits<{
  save: [flow: AgentFlowConfig]
  cancel: []
}>()

const PHASES: FlowPhase[] = ['onStart', 'afterCall', 'onStop']
const CODE_KEYS: FlowCodeKey[] = ['login', 'ready', 'notReady', 'logout']

// 編輯中的副本：按「儲存」才寫回設定
const draft = ref<AgentFlowConfig>(clone(props.flow))
function clone(f: AgentFlowConfig): AgentFlowConfig {
  return JSON.parse(JSON.stringify(f))
}

watch(() => props.visible, v => {
  if (!v) { window.removeEventListener('keydown', onKeydown); return }
  draft.value = clone(props.flow)
  window.addEventListener('keydown', onKeydown)
})
function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') emit('cancel')
}
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

const errors = computed(() => flowErrors(draft.value))

function addDial(phase: FlowPhase, key?: FlowCodeKey) {
  draft.value[phase].push(key
    ? newFlowStep('dial', draft.value.codes[key], FLOW_CODE_NAMES[key])
    : newFlowStep('dial'))
}
function addWait(phase: FlowPhase) {
  draft.value[phase].push(newFlowStep('wait', '', '', 2))
}
function removeStep(phase: FlowPhase, i: number) {
  draft.value[phase].splice(i, 1)
}
function applyPreset() {
  Object.assign(draft.value, sipxPresetFlow(draft.value.codes))
}
function clearAll() {
  for (const p of PHASES) draft.value[p] = []
}

// ── 拖曳排序：可在同一段內調整順序，也可拖到其他時間點 ──────────────
const dragging = ref<{ phase: FlowPhase; index: number } | null>(null)
const dropTarget = ref<{ phase: FlowPhase; index: number } | null>(null)

function onDragStart(e: DragEvent, phase: FlowPhase, index: number) {
  dragging.value = { phase, index }
  e.dataTransfer?.setData('text/plain', `${phase}:${index}`)
  if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
}
/** 指到某一步時插在它前面；指到該段空白處時放到最後 */
function onDragOver(e: DragEvent, phase: FlowPhase, index: number) {
  if (!dragging.value) return
  e.preventDefault()
  dropTarget.value = { phase, index }
}
function onDrop(e: DragEvent) {
  e.preventDefault()
  const from = dragging.value
  const to = dropTarget.value
  dragging.value = null
  dropTarget.value = null
  if (!from || !to) return
  const [step] = draft.value[from.phase].splice(from.index, 1)
  let index = to.index
  if (from.phase === to.phase && from.index < to.index) index -= 1
  draft.value[to.phase].splice(index, 0, step)
}
function onDragEnd() {
  dragging.value = null
  dropTarget.value = null
}
const isDropBefore = (phase: FlowPhase, i: number) =>
  dropTarget.value?.phase === phase && dropTarget.value.index === i

/** 第一個撥碼步驟代入後實際撥出的號碼 */
function preview(st: FlowStepConfig): string {
  return expandFlowCode(st.code.trim(), props.sampleExt, draft.value)
}

function save() {
  if (errors.value.length > 0) return
  emit('save', clone(draft.value))
}
</script>

<template>
  <Teleport to="body">
    <Transition name="overlay">
      <div v-if="visible" class="overlay" @click.self="emit('cancel')">
        <Transition name="dialog">
          <div v-if="visible" class="dialog" role="dialog" aria-modal="true" aria-labelledby="flow-dlg-title">

            <div class="dialog-hdr">
              <span class="dialog-title" id="flow-dlg-title">座席流程</span>
              <button class="close-btn" @click="emit('cancel')" aria-label="關閉">×</button>
            </div>

            <div class="dialog-body">
              <div class="intro">
                每個座席在三個時間點依序執行這些步驟。按住 <span class="grip-demo">⠿</span> 拖曳可調整順序，也能拖到其他時間點。
                撥碼可用 <code>{agent}</code>（座席號）與 <code>{pwd}</code>（座席密碼）。
              </div>

              <!-- 三個時間點 -->
              <section v-for="phase in PHASES" :key="phase" class="phase"
                       @dragover="onDragOver($event, phase, draft[phase].length)" @drop="onDrop">
                <div class="phase-hdr">{{ FLOW_PHASE_NAMES[phase] }}</div>
                <div v-if="draft[phase].length === 0" class="phase-empty"
                     :class="{ 'drop-here': isDropBefore(phase, 0) }">（不做事）</div>
                <div v-for="(st, i) in draft[phase]" :key="st.id" class="step"
                     :class="{ dragging: dragging?.phase === phase && dragging.index === i, 'drop-before': isDropBefore(phase, i) }"
                     draggable="true"
                     @dragstart="onDragStart($event, phase, i)"
                     @dragover.stop="onDragOver($event, phase, i)"
                     @drop.stop="onDrop"
                     @dragend="onDragEnd">
                  <span class="grip" title="拖曳排序">⠿</span>
                  <span class="num">{{ i + 1 }}</span>
                  <template v-if="st.type === 'dial'">
                    <span class="kind">撥碼</span>
                    <input v-model="st.code" class="in code" type="text" placeholder="*96" spellcheck="false">
                    <input v-model="st.label" class="in label" type="text" placeholder="名稱">
                  </template>
                  <template v-else>
                    <span class="kind">等待</span>
                    <input v-model.number="st.secs" class="in secs" type="number" min="0" step="0.5">
                    <span class="unit">秒</span>
                  </template>
                  <button class="del" @click="removeStep(phase, i)" aria-label="刪除步驟">×</button>
                </div>
                <div class="add-row">
                  <span class="add-lbl">＋</span>
                  <button v-for="k in CODE_KEYS" :key="k" class="chip" @click="addDial(phase, k)">{{ FLOW_CODE_NAMES[k] }}</button>
                  <button class="chip" @click="addDial(phase)">自訂撥碼</button>
                  <button class="chip" @click="addWait(phase)">等待</button>
                </div>
              </section>

              <!-- 變數與常用碼 -->
              <section class="settings">
                <div class="field-row3">
                  <label class="field">
                    <span><code>{agent}</code> = 分機末幾碼</span>
                    <input v-model.number="draft.agentDigits" class="in" type="number" min="0" title="0 = 整個分機號碼">
                  </label>
                  <label class="field">
                    <span><code>{pwd}</code> 座席密碼</span>
                    <input v-model="draft.agentPassword" class="in" type="text" placeholder="000000">
                  </label>
                  <label class="field">
                    <span>撥通 / 提示音後保持（秒）</span>
                    <input v-model.number="draft.holdSecs" class="in" type="number" min="0" max="60" step="0.5"
                           title="交換機回 200 OK 或開始播提示音（183）後保持幾秒再掛斷；提示音期間分機仍被佔用，SIPX 派來的電話會失敗，建議 1 秒">
                  </label>
                </div>
                <div class="codes">
                  <span class="codes-lbl">常用碼</span>
                  <label v-for="k in CODE_KEYS" :key="k" class="code-field">
                    <span>{{ FLOW_CODE_NAMES[k] }}</span>
                    <input v-model="draft.codes[k]" class="in" type="text" spellcheck="false">
                  </label>
                </div>
                <div class="preview">
                  預覽（分機 {{ sampleExt }}）：
                  <template v-for="phase in PHASES" :key="phase">
                    <code v-for="st in draft[phase].filter(s => s.type === 'dial')" :key="st.id">{{ preview(st) || '（空白）' }}</code>
                  </template>
                  <span v-if="draft.onStart.length + draft.afterCall.length + draft.onStop.length === 0" class="muted">尚無步驟</span>
                </div>
              </section>

              <div v-if="errors.length" class="errors">
                <div v-for="e in errors" :key="e">⚠ {{ e }}</div>
              </div>
            </div>

            <div class="dialog-footer">
              <button class="btn btn-sm" @click="applyPreset" title="非自動就緒的 SIPX：開始時登入 → 等待 1 秒 → 就緒；每通結束後就緒；結束時未就緒 → 登出。SIPX 設為自動就緒時，請刪掉就緒步驟">套用 SIPX 預設</button>
              <button class="btn btn-sm" @click="clearAll">清空</button>
              <span class="spacer"></span>
              <button class="btn btn-sm" @click="emit('cancel')">取消</button>
              <button class="btn btn-sm btn-accent" :disabled="errors.length > 0" @click="save">儲存</button>
            </div>

          </div>
        </Transition>
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
  width: 560px;
  max-width: calc(100vw - 40px);
  max-height: calc(100vh - 60px);
  display: flex; flex-direction: column;
  overflow: hidden;
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
}
.dialog-hdr {
  display: flex; align-items: center;
  padding: 14px 16px 12px;
  border-bottom: 1px solid var(--border);
}
.dialog-title { font-size: 13px; font-weight: 500; color: var(--text0); flex: 1; }
.close-btn {
  background: none; border: none; color: var(--text2);
  font-size: 18px; line-height: 1; cursor: pointer; padding: 0 2px;
}
.close-btn:hover { color: var(--text0); }
.dialog-body {
  padding: 12px 16px;
  overflow-y: auto; flex: 1;
  display: flex; flex-direction: column; gap: 10px;
}
.intro { font-size: 11px; color: var(--text2); line-height: 1.6; }
code { font-family: var(--mono); color: var(--accent); font-size: 11px; }
.grip-demo { font-family: var(--mono); color: var(--text1); }

/* ── 時間點 ─────────────────────────────────────────── */
.phase {
  background: var(--bg0);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 6px 8px 8px;
  display: flex; flex-direction: column; gap: 4px;
}
.phase-hdr { font-size: 11px; color: var(--text1); font-weight: 500; padding: 2px 0 4px; }
.phase-empty {
  font-size: 11px; color: var(--text2); padding: 6px; text-align: center;
  border: 1px dashed var(--border2); border-radius: var(--radius-sm);
}
.phase-empty.drop-here { border-color: rgba(var(--accent-rgb), 0.6); color: var(--accent); }

.step {
  display: flex; align-items: center; gap: 6px;
  padding: 4px 6px;
  background: var(--bg2);
  border: 1px solid var(--border2);
  border-radius: var(--radius-sm);
  position: relative;
}
.step.dragging { opacity: 0.4; }
.step.drop-before::before {
  content: ''; position: absolute; left: 0; right: 0; top: -3px;
  height: 2px; background: var(--accent); border-radius: 1px;
}
.grip { cursor: grab; color: var(--text2); font-family: var(--mono); user-select: none; }
.num { font-family: var(--mono); font-size: 10px; color: var(--text2); width: 14px; text-align: right; }
.kind { font-size: 11px; color: var(--text1); width: 28px; flex-shrink: 0; }
.unit { font-size: 11px; color: var(--text2); }
.in {
  background: var(--bg0);
  border: 1px solid var(--border2);
  border-radius: var(--radius-sm);
  padding: 4px 7px;
  color: var(--text0);
  font-family: var(--mono);
  font-size: 12px;
  outline: none;
  min-width: 0;
}
.in:focus { border-color: rgba(var(--accent-rgb), 0.6); }
.in.code { flex: 2; }
.in.label { flex: 1; font-family: inherit; }
.in.secs { width: 70px; }
.del {
  margin-left: auto;
  background: none; border: none; color: var(--text2);
  font-size: 15px; cursor: pointer; padding: 0 2px;
}
.del:hover { color: var(--danger); }

.add-row { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; padding-top: 2px; }
.add-lbl { font-size: 11px; color: var(--text2); }
.chip {
  background: none;
  border: 1px solid var(--border2);
  border-radius: 10px;
  padding: 1px 8px;
  font-size: 11px;
  color: var(--text1);
  cursor: pointer;
}
.chip:hover { border-color: rgba(var(--accent-rgb), 0.5); color: var(--accent); }

/* ── 設定 ─────────────────────────────────────────── */
.settings { display: flex; flex-direction: column; gap: 8px; }
.field-row3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
.field { display: flex; flex-direction: column; gap: 3px; font-size: 11px; color: var(--text1); }
.codes { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.codes-lbl { font-size: 11px; color: var(--text1); }
.code-field { display: flex; align-items: center; gap: 4px; font-size: 11px; color: var(--text2); }
.code-field .in { width: 96px; }
.preview { font-size: 11px; color: var(--text2); display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.preview code { background: var(--bg0); border: 1px solid var(--border); border-radius: 3px; padding: 0 5px; }
.muted { color: var(--text2); }
.errors { font-size: 11px; color: var(--danger); display: flex; flex-direction: column; gap: 2px; }

.dialog-footer {
  display: flex; align-items: center; gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}
.spacer { flex: 1; }

.overlay-enter-active, .overlay-leave-active { transition: opacity 0.2s ease; }
.overlay-enter-from, .overlay-leave-to { opacity: 0; }
.dialog-enter-active, .dialog-leave-active { transition: opacity 0.2s ease, transform 0.2s ease; }
.dialog-enter-from, .dialog-leave-to { opacity: 0; transform: scale(0.96) translateY(-8px); }
</style>
