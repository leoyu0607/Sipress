<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import Icon from './Icon.vue'
import {
  useTestStore, newCallerStep, callerFlowErrors, callerFlowNeedsTts,
  CALLER_STEP_NAMES, TTS_DEFAULT_VOICE, TTS_KEY_ENV,
  type CallerFlowConfig, type CallerStepType,
} from '../stores/testStore'

const props = defineProps<{
  visible: boolean
  flow: CallerFlowConfig
}>()

const emit = defineEmits<{
  save: [flow: CallerFlowConfig]
  cancel: []
}>()

const store = useTestStore()
const STEP_TYPES: CallerStepType[] = ['wait', 'dtmf', 'play', 'say', 'waitSilence']

// 編輯中的副本：按「儲存」才寫回設定
const draft = ref<CallerFlowConfig>(clone(props.flow))
function clone(f: CallerFlowConfig): CallerFlowConfig {
  return JSON.parse(JSON.stringify(f))
}

watch(() => props.visible, v => {
  if (!v) { window.removeEventListener('keydown', onKeydown); stopPreview(); return }
  draft.value = clone(props.flow)
  previewErr.value = ''
  window.addEventListener('keydown', onKeydown)
})
function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') emit('cancel')
}
onUnmounted(() => { window.removeEventListener('keydown', onKeydown); stopPreview() })

const errors = computed(() => callerFlowErrors(draft.value))
const needsTts = computed(() => callerFlowNeedsTts(draft.value))

function addStep(type: CallerStepType) {
  draft.value.steps.push(newCallerStep(type))
}
function removeStep(i: number) {
  draft.value.steps.splice(i, 1)
}
function clearAll() {
  draft.value.steps = []
}

/** 範例流程：傳統按鍵 IVR 與智能語音 IVR */
function applyExample(kind: 'dtmf' | 'voice') {
  draft.value.steps = kind === 'dtmf'
    ? [
        newCallerStep('wait', { secs: 10 }),
        newCallerStep('dtmf', { digits: '1' }),
        newCallerStep('wait', { secs: 5 }),
        newCallerStep('dtmf', { digits: '2' }),
        newCallerStep('wait', { secs: 10 }),
      ]
    : [
        newCallerStep('waitSilence', { silenceMs: 800, timeoutSecs: 20 }),
        newCallerStep('say', { texts: ['我想查詢這個月的帳單', '請問這個月的帳單金額是多少'] }),
        newCallerStep('waitSilence', { silenceMs: 800, timeoutSecs: 20 }),
        newCallerStep('say', { texts: ['好的，謝謝'] }),
        newCallerStep('waitSilence', { silenceMs: 800, timeoutSecs: 10 }),
      ]
}

// ── 多個音檔 / 多段文字（每通隨機挑一個）──────────────────────────────
async function pickFile(stepIdx: number, fileIdx: number) {
  const path = await open({ multiple: false, filters: [{ name: '音檔（WAV / G.711）', extensions: ['wav', 'al', 'alaw', 'pcma', 'ul', 'ulaw', 'pcmu', 'raw'] }] })
  if (path && typeof path === 'string') draft.value.steps[stepIdx].files[fileIdx] = path
}
function addVariant(stepIdx: number, key: 'files' | 'texts') {
  draft.value.steps[stepIdx][key].push('')
}
function removeVariant(stepIdx: number, key: 'files' | 'texts', i: number) {
  const list = draft.value.steps[stepIdx][key]
  if (list.length > 1) list.splice(i, 1)
}
const fileName = (p: string) => p.replace(/\\/g, '/').split('/').pop() || p

// ── TTS 試聽 ────────────────────────────────────────────────────────
const previewing = ref('')      // 正在合成的文字
const previewErr = ref('')
let audio: HTMLAudioElement | null = null
function stopPreview() {
  audio?.pause()
  audio = null
}
async function preview(text: string) {
  if (!text.trim() || previewing.value) return
  stopPreview()
  previewErr.value = ''
  previewing.value = text
  try {
    audio = new Audio(await store.previewTts(draft.value.tts, text))
    await audio.play()
  } catch (e) {
    previewErr.value = String(e)
  } finally {
    previewing.value = ''
  }
}

// ── 拖曳排序 ────────────────────────────────────────────────────────
const dragging = ref<number | null>(null)
const dropTarget = ref<number | null>(null)

function onDragStart(e: DragEvent, index: number) {
  dragging.value = index
  e.dataTransfer?.setData('text/plain', String(index))
  if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
}
function onDragOver(e: DragEvent, index: number) {
  if (dragging.value === null) return
  e.preventDefault()
  dropTarget.value = index
}
function onDrop() {
  const from = dragging.value
  let to = dropTarget.value
  dragging.value = null
  dropTarget.value = null
  if (from === null || to === null || from === to) return
  const [step] = draft.value.steps.splice(from, 1)
  if (from < to) to -= 1
  draft.value.steps.splice(to, 0, step)
}
function onDragEnd() {
  dragging.value = null
  dropTarget.value = null
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
          <div v-if="visible" class="dialog" role="dialog" aria-modal="true" aria-labelledby="cflow-dlg-title">

            <div class="dialog-hdr">
              <span class="dialog-title" id="cflow-dlg-title">通話流程</span>
              <button class="close-btn" @click="emit('cancel')" aria-label="關閉"><Icon name="x" :size="16" /></button>
            </div>

            <div class="dialog-body">
              <div class="intro">
                每通接通後依序執行，用來測試傳統按鍵 IVR 與智能語音 IVR。設定流程後會取代側欄的「播放音檔」與「DTMF 序列」。
                按住 <span class="grip-demo">⠿</span> 拖曳可調整順序。
              </div>

              <section class="steps" @dragover="onDragOver($event, draft.steps.length)" @drop="onDrop">
                <div v-if="draft.steps.length === 0" class="steps-empty"
                     :class="{ 'drop-here': dropTarget === 0 }">尚無步驟：從下方加入，或套用範例</div>
                <div v-for="(st, i) in draft.steps" :key="st.id" class="step"
                     :class="{ dragging: dragging === i, 'drop-before': dropTarget === i }"
                     draggable="true"
                     @dragstart="onDragStart($event, i)"
                     @dragover.stop="onDragOver($event, i)"
                     @drop.stop="onDrop"
                     @dragend="onDragEnd">
                  <span class="grip" title="拖曳排序">⠿</span>
                  <span class="num">{{ i + 1 }}</span>
                  <span class="kind">{{ CALLER_STEP_NAMES[st.type] }}</span>

                  <div class="body">
                    <template v-if="st.type === 'wait'">
                      <input v-model.number="st.secs" class="in secs" type="number" min="0" step="0.5" aria-label="等待秒數">
                      <span class="unit">秒（送靜音，例如先聽完招呼語）</span>
                    </template>

                    <template v-else-if="st.type === 'dtmf'">
                      <input v-model="st.digits" class="in digits" type="text" placeholder="1 或 0912345678#" spellcheck="false" aria-label="按鍵">
                      <span class="unit">依序按下（0-9 * # A-D）</span>
                    </template>

                    <template v-else-if="st.type === 'play'">
                      <div v-for="(f, j) in st.files" :key="j" class="variant">
                        <input v-model="st.files[j]" class="in grow" type="text" placeholder="prompt.wav" :title="f" aria-label="音檔路徑">
                        <button class="btn btn-xs" @click="pickFile(i, j)">瀏覽</button>
                        <button v-if="st.files.length > 1" class="mini-del" @click="removeVariant(i, 'files', j)" :aria-label="`移除 ${fileName(f)}`"><Icon name="x" :size="11" /></button>
                      </div>
                      <button class="link" @click="addVariant(i, 'files')">＋ 再加一個檔案</button>
                      <span v-if="st.files.length > 1" class="unit">每通隨機挑一個</span>
                    </template>

                    <template v-else-if="st.type === 'say'">
                      <div v-for="(t, j) in st.texts" :key="j" class="variant">
                        <input v-model="st.texts[j]" class="in grow sans" type="text" placeholder="我想查詢這個月的帳單" aria-label="說話內容">
                        <button class="btn btn-xs" :disabled="!t.trim() || !!previewing" @click="preview(t)"
                                :title="'用目前的 TTS 設定合成並播放（合成結果會快取，測試時直接使用）'">
                          {{ previewing === t ? '合成中…' : '試聽' }}
                        </button>
                        <button v-if="st.texts.length > 1" class="mini-del" @click="removeVariant(i, 'texts', j)" aria-label="移除這段文字"><Icon name="x" :size="11" /></button>
                      </div>
                      <button class="link" @click="addVariant(i, 'texts')">＋ 再加一種說法</button>
                      <span v-if="st.texts.length > 1" class="unit">每通隨機挑一段</span>
                    </template>

                    <template v-else>
                      <span class="unit">對方安靜</span>
                      <input v-model.number="st.silenceMs" class="in secs" type="number" min="100" max="10000" step="100" aria-label="安靜毫秒數">
                      <span class="unit">毫秒算講完，最多等</span>
                      <input v-model.number="st.timeoutSecs" class="in secs" type="number" min="1" step="1" aria-label="最長等待秒數">
                      <span class="unit">秒</span>
                    </template>
                  </div>

                  <button class="del" @click="removeStep(i)" aria-label="刪除步驟"><Icon name="x" :size="13" /></button>
                </div>
                <div class="add-row">
                  <span class="add-lbl">＋</span>
                  <button v-for="t in STEP_TYPES" :key="t" class="chip" @click="addStep(t)">{{ CALLER_STEP_NAMES[t] }}</button>
                </div>
              </section>

              <label class="check-row">
                <input type="checkbox" v-model="draft.hangupWhenDone">
                流程跑完就掛斷（不勾選則送靜音，直到單通時長到期才掛斷）
              </label>

              <!-- TTS -->
              <section v-if="needsTts" class="tts">
                <div class="tts-hdr">TTS 設定 <span class="muted">（測試開始前合成一次並快取，壓測期間不連線）</span></div>
                <div class="tts-grid">
                  <label class="field">
                    <span>服務</span>
                    <select v-model="draft.tts.provider" class="in sans">
                      <option value="azure">Azure AI Speech</option>
                      <option value="google">Google Cloud TTS</option>
                    </select>
                  </label>
                  <label v-if="draft.tts.provider === 'azure'" class="field">
                    <span>區域</span>
                    <input v-model="draft.tts.region" class="in" type="text" placeholder="eastasia" spellcheck="false">
                  </label>
                  <label class="field">
                    <span>聲音</span>
                    <input v-model="draft.tts.voice" class="in" type="text" :placeholder="TTS_DEFAULT_VOICE[draft.tts.provider]" spellcheck="false">
                  </label>
                  <label class="field key">
                    <span>API 金鑰</span>
                    <input v-model="draft.tts.apiKey" class="in" type="password" :placeholder="`留空 = 使用環境變數 ${TTS_KEY_ENV}`" autocomplete="off" spellcheck="false">
                  </label>
                </div>
                <div class="tts-note">
                  文字會送到所選的雲端服務合成。金鑰只存在這台電腦，不會寫進結果檔與匯出的流程檔。
                </div>
                <div v-if="previewErr" class="err-line"><Icon name="alert" :size="12" />{{ previewErr }}</div>
              </section>

              <div v-if="errors.length" class="errors">
                <div v-for="e in errors" :key="e" class="err-line"><Icon name="alert" :size="12" />{{ e }}</div>
              </div>
            </div>

            <div class="dialog-footer">
              <button class="btn btn-sm" @click="applyExample('dtmf')" title="等 10 秒（招呼語）→ 按 1 → 等 5 秒 → 按 2 → 等 10 秒">按鍵 IVR 範例</button>
              <button class="btn btn-sm" @click="applyExample('voice')" title="等對方講完 → 說話 → 等對方講完 → 說話 → 等對方講完">語音 IVR 範例</button>
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
  width: 640px;
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
  display: inline-flex; align-items: center; cursor: pointer; padding: 2px; border-radius: var(--radius-sm);
}
.close-btn:hover { color: var(--text0); }
.dialog-body {
  padding: 12px 16px;
  overflow-y: auto; flex: 1;
  display: flex; flex-direction: column; gap: 10px;
}
.intro { font-size: 11px; color: var(--text2); line-height: 1.6; }
.grip-demo { font-family: var(--mono); color: var(--text1); }

/* ── 步驟 ─────────────────────────────────────────── */
.steps {
  background: var(--bg0);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 8px;
  display: flex; flex-direction: column; gap: 4px;
}
.steps-empty {
  font-size: 11px; color: var(--text2); padding: 10px; text-align: center;
  border: 1px dashed var(--border2); border-radius: var(--radius-sm);
}
.steps-empty.drop-here { border-color: rgba(var(--accent-rgb), 0.6); color: var(--accent); }

.step {
  display: flex; align-items: flex-start; gap: 6px;
  padding: 5px 6px;
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
.grip { cursor: grab; color: var(--text2); font-family: var(--mono); user-select: none; line-height: 24px; }
.num { font-family: var(--mono); font-size: 11px; color: var(--text2); width: 14px; text-align: right; line-height: 24px; }
.kind { font-size: 11px; color: var(--text1); width: 72px; flex-shrink: 0; line-height: 24px; }
.body { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; align-items: center; gap: 4px 6px; }
.variant { display: flex; align-items: center; gap: 4px; width: 100%; }
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
.in.sans { font-family: inherit; }
.in.grow { flex: 1; }
.in.secs { width: 70px; }
.in.digits { width: 160px; }
.btn-xs { font-size: 11px; padding: 2px 8px; flex-shrink: 0; }
.link {
  background: none; border: none; padding: 0;
  font-size: 11px; color: var(--accent); cursor: pointer;
}
.link:hover { text-decoration: underline; }
.mini-del, .del {
  background: none; border: none; color: var(--text2);
  display: inline-flex; align-items: center; cursor: pointer; padding: 2px;
}
.del { line-height: 24px; margin-top: 3px; }
.mini-del:hover, .del:hover { color: var(--danger); }

.add-row { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; padding-top: 4px; }
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

.check-row { display: flex; align-items: center; gap: 6px; font-size: 11px; color: var(--text1); cursor: pointer; }

/* ── TTS ─────────────────────────────────────────── */
.tts {
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 8px;
  display: flex; flex-direction: column; gap: 8px;
}
.tts-hdr { font-size: 11px; color: var(--text1); font-weight: 500; }
.tts-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
.field { display: flex; flex-direction: column; gap: 3px; font-size: 11px; color: var(--text1); }
.field.key { grid-column: 1 / -1; }
.tts-note { font-size: 11px; color: var(--text2); line-height: 1.5; }
.muted { color: var(--text2); font-weight: normal; }

.errors { font-size: 11px; color: var(--danger); display: flex; flex-direction: column; gap: 2px; }
.err-line { display: flex; align-items: flex-start; gap: 5px; font-size: 11px; color: var(--danger); overflow-wrap: anywhere; }
.err-line .icon { margin-top: 2px; flex-shrink: 0; }

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
