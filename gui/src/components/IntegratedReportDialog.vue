<script setup lang="ts">
/**
 * 整合報告：選擇壓測結果（最近一次執行或 result.json）、主機效能資料（選填）、
 * 判定門檻與受測環境備註，存成 HTML、轉成 PDF 或用瀏覽器開啟。
 */
import { ref, computed, watch, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import {
  useTestStore, MODE_LABEL, DEFAULT_THRESHOLDS,
  loadReportSettings, saveReportSettings,
  type CallMode, type IntegratedRequest, type ReportThresholds,
} from '../stores/testStore'

const props = defineProps<{ visible: boolean }>()
const emit = defineEmits<{ close: [] }>()

const store = useTestStore()

/** 'caller' / 'agent' = 最近一次執行的結果；'file' = 壓測數據檔 */
const source = ref<CallMode | 'file'>('caller')
const resultPath = ref('')
const hostPath = ref('')
const thresholds = ref<ReportThresholds>({ ...DEFAULT_THRESHOLDS })
const notes = ref('')
const busy = ref<'' | 'html' | 'pdf' | 'open'>('')
const error = ref('')
const pdfReady = ref(true)

const done = (m: CallMode) => store.sessions[m].status === 'done'

const THRESHOLD_FIELDS: { key: keyof ReportThresholds; label: string; unit: string; hint: string; step: number }[] = [
  { key: 'mos',        label: 'MOS 下限',      unit: '',   hint: 'MOS（jitter buffer 200 ms 模擬）每通電話的 P5 不低於此值', step: 0.1 },
  { key: 'loss_pct',   label: '掉包率上限',     unit: '%',  hint: '整體掉包率（所有通話的掉包數 ÷ 應收封包數）', step: 0.1 },
  { key: 'jitter_ms',  label: 'Jitter 上限',   unit: 'ms', hint: '每通電話 Jitter 的 P95', step: 1 },
  { key: 'cpu_pct',    label: 'CPU 上限',      unit: '%',  hint: '主機 CPU 使用率（10 秒取樣）的 P95', step: 1 },
  { key: 'mem_pct',    label: '記憶體上限',     unit: '%',  hint: '主機記憶體使用率（10 秒取樣）的 P95', step: 1 },
  { key: 'disk_queue', label: 'Disk Queue 上限', unit: '',  hint: '每顆磁碟平均 I/O 佇列長度（10 秒取樣）的 P95', step: 0.5 },
]

watch(() => props.visible, async v => {
  if (!v) { window.removeEventListener('keydown', onKeydown); return }
  window.addEventListener('keydown', onKeydown)
  const saved = loadReportSettings()
  thresholds.value = saved.thresholds
  notes.value = saved.notes
  error.value = ''
  // 預設選目前模式最近一次的結果；沒有時改選檔案
  const m = store.config.mode as CallMode
  source.value = done(m) ? m : done(m === 'caller' ? 'agent' : 'caller') ? (m === 'caller' ? 'agent' : 'caller') : 'file'
  try { pdfReady.value = await invoke<boolean>('pdf_available') } catch { pdfReady.value = false }
})
function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && !busy.value) emit('close')
}
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

async function pickResult() {
  const p = await open({ multiple: false, filters: [{ name: '壓測數據（*_result.json）', extensions: ['json'] }] })
  if (p && typeof p === 'string') { resultPath.value = p; source.value = 'file' }
}
async function pickHost() {
  const p = await open({ multiple: false, filters: [{ name: '主機效能資料（ZIP / CSV）', extensions: ['zip', 'csv'] }] })
  if (p && typeof p === 'string') hostPath.value = p
}
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p

const invalid = computed(() => {
  if (source.value === 'file' && !resultPath.value) return '請選擇壓測數據檔'
  if (source.value !== 'file' && !done(source.value)) return `${MODE_LABEL[source.value]}還沒有完成的測試`
  const bad = THRESHOLD_FIELDS.find(f => !(Number.isFinite(thresholds.value[f.key]) && thresholds.value[f.key] >= 0))
  return bad ? `${bad.label}需為 0 以上的數字` : ''
})

function request(): IntegratedRequest {
  return {
    result_path: source.value === 'file' ? resultPath.value : null,
    mode: source.value === 'file' ? null : source.value,
    host_metrics_path: hostPath.value || null,
    thresholds: { ...thresholds.value },
    notes: notes.value,
  }
}

async function run(kind: 'html' | 'pdf' | 'open') {
  if (invalid.value || busy.value) return
  saveReportSettings(thresholds.value, notes.value)
  busy.value = kind
  error.value = ''
  const req = request()
  const err = kind === 'html' ? await store.exportIntegratedHtml(req)
            : kind === 'pdf'  ? await store.exportIntegratedPdf(req)
            : await store.openIntegratedReport(req)
  busy.value = ''
  if (err) error.value = err
}
</script>

<template>
  <Teleport to="body">
    <Transition name="overlay">
      <div v-if="visible" class="overlay" @click.self="!busy && emit('close')">
        <Transition name="dialog">
          <div v-if="visible" class="dialog" role="dialog" aria-modal="true" aria-labelledby="ir-dlg-title">

            <div class="dialog-hdr">
              <span class="dialog-title" id="ir-dlg-title">整合報告</span>
              <button class="close-btn" :disabled="!!busy" @click="emit('close')" aria-label="關閉">×</button>
            </div>

            <div class="dialog-body">
              <div class="intro">
                交給客戶的正式報告：結論、測試條件、通話量與聲音品質圖表，可加上受測主機的 CPU、記憶體、磁碟資料。版面以 A4 列印為準。
              </div>

              <!-- 壓測結果 -->
              <section class="group">
                <div class="group-hdr">壓測結果</div>
                <label v-for="m in (['caller', 'agent'] as CallMode[])" :key="m" class="radio" :class="{ disabled: !done(m) }">
                  <input v-model="source" type="radio" :value="m" :disabled="!done(m)">
                  最近一次{{ MODE_LABEL[m] }}測試
                  <span v-if="!done(m)" class="muted">（尚無結果）</span>
                </label>
                <label class="radio">
                  <input v-model="source" type="radio" value="file">
                  壓測數據檔
                  <button class="btn btn-sm" @click.prevent="pickResult">選擇…</button>
                  <span class="path" :title="resultPath">{{ resultPath ? baseName(resultPath) : 'CLI 加 --html 或 GUI「↓ 數據」產生的 *_result.json' }}</span>
                </label>
              </section>

              <!-- 主機資料 -->
              <section class="group">
                <div class="group-hdr">受測主機效能資料<span class="muted">（選填）</span></div>
                <div class="row">
                  <button class="btn btn-sm" @click="pickHost">選擇…</button>
                  <span class="path" :title="hostPath">{{ hostPath ? baseName(hostPath) : '主機 agent「效能」頁的「匯出分析資料 (ZIP)」，時間範圍需涵蓋測試時間' }}</span>
                  <button v-if="hostPath" class="link" @click="hostPath = ''">移除</button>
                </div>
              </section>

              <!-- 門檻 -->
              <section class="group">
                <div class="group-hdr">
                  判定門檻
                  <button class="link" @click="thresholds = { ...DEFAULT_THRESHOLDS }">恢復預設</button>
                </div>
                <div class="grid">
                  <label v-for="f in THRESHOLD_FIELDS" :key="f.key" class="field" :title="f.hint">
                    <span>{{ f.label }}</span>
                    <span class="in-wrap">
                      <input v-model.number="thresholds[f.key]" class="in" type="number" min="0" :step="f.step">
                      <span class="unit">{{ f.unit }}</span>
                    </span>
                  </label>
                </div>
              </section>

              <!-- 備註 -->
              <section class="group">
                <div class="group-hdr">受測環境備註<span class="muted">（選填，列在「測試條件」）</span></div>
                <textarea v-model="notes" class="in notes" rows="3"
                          placeholder="例如：受測系統 SIPX 4.2；交換機與壓測機在同一內網；測試期間無其他話務"></textarea>
              </section>

              <div v-if="invalid" class="errors">⚠ {{ invalid }}</div>
              <div v-else-if="error" class="errors">⚠ {{ error }}</div>
              <div v-if="!pdfReady" class="hint">找不到 Chrome 或 Edge，無法直接轉成 PDF；請用「瀏覽器開啟」後按報告右上角的「列印 / 另存 PDF」。</div>
            </div>

            <div class="dialog-footer">
              <span v-if="busy" class="muted">{{ busy === 'pdf' ? '轉成 PDF 中…' : '產生中…' }}</span>
              <span class="spacer"></span>
              <button class="btn btn-sm" :disabled="!!invalid || !!busy" @click="run('html')">儲存 HTML</button>
              <button class="btn btn-sm" :disabled="!!invalid || !!busy" @click="run('open')"
                      title="用預設瀏覽器開啟報告，可在瀏覽器列印成 PDF">瀏覽器開啟</button>
              <button class="btn btn-sm btn-accent" :disabled="!!invalid || !!busy || !pdfReady" @click="run('pdf')"
                      :title="pdfReady ? '用本機的 Chrome / Edge 轉成 PDF' : '找不到 Chrome 或 Edge'">匯出 PDF</button>
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
  width: 600px;
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

.group {
  background: var(--bg0);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 8px 10px 10px;
  display: flex; flex-direction: column; gap: 6px;
}
.group-hdr { font-size: 11px; color: var(--text1); font-weight: 500; display: flex; align-items: center; gap: 6px; }
.muted { color: var(--text2); font-weight: 400; font-size: 11px; }
.radio { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--text0); min-width: 0; }
.radio.disabled { color: var(--text2); }
.row { display: flex; align-items: center; gap: 6px; min-width: 0; }
.path {
  font-family: var(--mono); font-size: 11px; color: var(--text2);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; flex: 1;
}
.link {
  background: none; border: none; padding: 0;
  font-size: 11px; color: var(--accent); cursor: pointer; margin-left: auto;
}
.link:hover { text-decoration: underline; }

.grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
.field { display: flex; flex-direction: column; gap: 3px; font-size: 11px; color: var(--text1); cursor: help; }
.in-wrap { display: flex; align-items: center; gap: 4px; }
.unit { font-size: 11px; color: var(--text2); min-width: 18px; }
.in {
  background: var(--bg1);
  border: 1px solid var(--border2);
  border-radius: var(--radius-sm);
  padding: 4px 7px;
  color: var(--text0);
  font-family: var(--mono);
  font-size: 12px;
  outline: none;
  min-width: 0;
  width: 100%;
}
.in:focus { border-color: rgba(var(--accent-rgb), 0.6); }
.notes { font-family: inherit; resize: vertical; line-height: 1.5; }

.errors { font-size: 11px; color: var(--danger); }
.hint { font-size: 11px; color: var(--warn); line-height: 1.5; }

.dialog-footer {
  display: flex; align-items: center; gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}
.spacer { flex: 1; }

@media (max-width: 560px) { .grid { grid-template-columns: repeat(2, 1fr); } }

.overlay-enter-active, .overlay-leave-active { transition: opacity 0.2s ease; }
.overlay-enter-from, .overlay-leave-to { opacity: 0; }
.dialog-enter-active, .dialog-leave-active { transition: opacity 0.2s ease, transform 0.2s ease; }
.dialog-enter-from, .dialog-leave-to { opacity: 0; transform: scale(0.96) translateY(-8px); }
</style>
