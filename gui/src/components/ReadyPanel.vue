<script setup lang="ts">
/**
 * 設定階段的右側：開始前檢查、這次會跑什麼、判定標準、開始按鈕、上次結果。
 * 設定本身在左邊側欄；這裡只整理與提示，不重複放輸入欄位（判定標準除外）。
 */
import { computed, ref, watch } from 'vue'
import { useTestStore, MODE_LABEL, DEFAULT_THRESHOLDS, type CallMode, type ReportThresholds } from '../stores/testStore'
import StartButton from './StartButton.vue'
import Icon from './Icon.vue'

const store = useTestStore()
const mode = computed(() => store.config.mode)
const side = computed(() => MODE_LABEL[mode.value])
const session = computed(() => store.sessions[mode.value])
/** 另一端正在跑（例如「兩端一起跑」先啟動座席端、等註冊時） */
const otherMode = computed<CallMode>(() => mode.value === 'caller' ? 'agent' : 'caller')
const otherRunning = computed(() => store.sessions[otherMode.value].status === 'running')

function fmtSecs(s: number) {
  if (s <= 0) return '不限（手動停止）'
  if (s < 60) return `${s} 秒`
  const m = Math.floor(s / 60), r = s % 60
  return r ? `${m} 分 ${r} 秒` : `${m} 分鐘`
}

const PATTERN_LABEL: Record<string, string> = {
  flat: '固定速率', ramp: '逐步加速', step: '分段', spike: '突增', batch: '批次',
}
const AUDIO_LABEL: Record<string, string> = { silence: '不送語音', loop: '循環播放音檔', flow: '通話流程' }

/** 這次會跑：每一行是一項（標籤、值） */
const plan = computed<[string, string][]>(() => {
  const c = store.config
  if (mode.value === 'caller') {
    const p = c.caller
    const rows: [string, string][] = [
      ['持續時間', fmtSecs(p.duration)],
      ['同時通話上限', `${p.concurrency} 通`],
      ['發起速率', p.cpsPattern.type === 'flat' ? `每秒 ${p.cps} 通` : PATTERN_LABEL[p.cpsPattern.type] ?? p.cpsPattern.type],
      ['總通數', p.totalCalls > 0 ? `${p.totalCalls.toLocaleString()} 通` : '不限'],
      ['通話聲音', AUDIO_LABEL[p.audioMode] + (p.audioMode === 'silence' ? '' : ` · ${store.codecLabel.short}`)],
    ]
    return rows
  }
  const a = c.agent
  return [
    ['持續時間', fmtSecs(a.duration)],
    ['座席帳號', `${a.accounts.length} 個`],
    ['接聽', `響鈴 ${a.behavior.ringMin}–${a.behavior.ringMax} 秒後接，通話 ${a.behavior.talkMin}–${a.behavior.talkMax} 秒`],
  ]
})

// ── 判定標準（存在本機；整合報告用同一份）──────────────────────────────────────
type Field = { key: keyof ReportThresholds; label: string; unit: string; step: number; callerOnly?: boolean; hint: string }
const FIELDS: Field[] = [
  { key: 'asr_pct',   label: '接通率 ≥', unit: '%',  step: 1,   callerOnly: true, hint: '民眾端接通率（接通 ÷ 發起）' },
  { key: 'mos',       label: 'MOS ≥',    unit: '',   step: 0.1, hint: 'MOS（jitter buffer 200 ms 模擬）每通電話的 P5' },
  { key: 'loss_pct',  label: '掉包率 ≤', unit: '%',  step: 0.1, hint: '整體掉包率' },
  { key: 'jitter_ms', label: 'Jitter ≤', unit: 'ms', step: 1,   hint: '每通電話 Jitter 的 P95' },
]
const fields = computed(() => FIELDS.filter(f => !f.callerOnly || mode.value === 'caller'))
const draft = ref<ReportThresholds>({ ...store.thresholds })
watch(() => store.thresholds, t => { draft.value = { ...t } }, { deep: true })
function commit(f: Field) {
  const v = draft.value[f.key]
  if (!(Number.isFinite(v) && v >= 0)) { draft.value[f.key] = store.thresholds[f.key]; return }
  store.setThresholds({ ...store.thresholds, [f.key]: v })
}
const isDefault = computed(() => FIELDS.every(f => store.thresholds[f.key] === DEFAULT_THRESHOLDS[f.key]))
function resetThresholds() {
  const t = { ...store.thresholds }
  for (const f of FIELDS) (t[f.key] as number) = DEFAULT_THRESHOLDS[f.key]
  store.setThresholds(t)
}

// ── 上次結果 ──────────────────────────────────────────────────────────────────
const hasResult = computed(() => session.value.status === 'done' || session.value.status === 'error')
const verdict = computed(() => store.verdicts[mode.value])
const VERDICT_LABEL = { pass: '通過', fail: '未通過', nodata: '無法判定' } as const
</script>

<template>
  <main class="ready">
    <div v-if="session.status === 'running'" class="running-note" role="status">
      <Icon name="lock" :size="12" />{{ side }}測試執行中，設定已鎖定
      <button type="button" class="btn btn-sm" @click="store.goStage('running')">回到執行畫面</button>
    </div>

    <div v-else-if="otherRunning" class="running-note" role="status">
      {{ MODE_LABEL[otherMode] }}測試執行中
      <button type="button" class="btn btn-sm" @click="store.config.mode = otherMode">切換到{{ MODE_LABEL[otherMode] }}</button>
    </div>

    <header class="head">
      <h1>準備開始{{ side }}測試</h1>
      <p>左邊改設定，這裡會即時更新。按「開始測試」後畫面會切到執行中。</p>
    </header>

    <div class="cards">
      <section class="card" aria-labelledby="ready-check">
        <h2 id="ready-check">開始前檢查</h2>
        <ul v-if="store.validationErrors.length" class="checks">
          <li v-for="e in store.validationErrors" :key="e" class="bad"><Icon name="alert" :size="12" /><span>{{ e }}</span></li>
        </ul>
        <p v-else class="ok"><Icon name="check" :size="12" />設定完整，可以開始</p>
      </section>

      <section class="card" aria-labelledby="ready-plan">
        <h2 id="ready-plan">這次會跑</h2>
        <dl class="plan">
          <template v-for="[k, v] in plan" :key="k"><dt>{{ k }}</dt><dd>{{ v }}</dd></template>
        </dl>
      </section>
    </div>

    <div class="start-row">
      <StartButton class="big-start" />
      <span v-if="store.validationErrors.length" class="start-hint">先修正上面 {{ store.validationErrors.length }} 項才能開始</span>
    </div>

    <section class="card" aria-labelledby="ready-th">
      <div class="card-head">
        <h2 id="ready-th">判定標準</h2>
        <span class="sub">結果畫面與整合報告用這組門檻判定通過與否</span>
        <button v-if="!isDefault" type="button" class="link" @click="resetThresholds">恢復預設</button>
      </div>
      <div class="th-grid">
        <label v-for="f in fields" :key="f.key" class="th" :title="f.hint">
          <span>{{ f.label }}</span>
          <span class="th-in">
            <input v-model.number="draft[f.key]" type="number" min="0" :step="f.step" @change="commit(f)">
            <span class="unit">{{ f.unit }}</span>
          </span>
        </label>
      </div>
    </section>

    <section v-if="hasResult" class="card last" aria-labelledby="ready-last">
      <h2 id="ready-last">上次結果</h2>
      <span v-if="verdict" class="verdict" :class="verdict.verdict">{{ VERDICT_LABEL[verdict.verdict] }}</span>
      <span v-else-if="session.status === 'error'" class="verdict fail">執行失敗</span>
      <div class="kv"><span>發起</span><b>{{ session.metrics.initiated.toLocaleString() }}</b></div>
      <div class="kv"><span>接通率</span><b>{{ session.metrics.asr.toFixed(1) }}%</b></div>
      <div class="kv" v-if="session.rtpMetrics.enabled"><span>MOS</span><b>{{ session.rtpMetrics.mos.toFixed(2) }}</b></div>
      <div class="kv"><span>失敗</span><b>{{ session.metrics.failed.toLocaleString() }}</b></div>
      <button type="button" class="link see" @click="store.goStage('result')">看完整結果 →</button>
    </section>
  </main>
</template>

<style scoped>
.ready {
  flex: 1; min-width: 0; overflow-y: auto;
  padding: 24px 32px; display: flex; flex-direction: column; gap: 16px;
}
/* 寬螢幕不要把卡片和欄位拉得太開 */
.ready > * { width: 100%; max-width: 1040px; flex-shrink: 0; }
.running-note {
  display: flex; align-items: center; gap: 8px;
  padding: 8px 12px; border-radius: var(--radius);
  background: rgba(var(--accent-rgb),0.08); border: 1px solid rgba(var(--accent-rgb),0.3);
  color: var(--text0); font-size: 12px;
}
.running-note .btn { margin-left: auto; }
.head h1 { font-size: 18px; font-weight: 600; }
.head p { color: var(--text1); font-size: 12px; margin-top: 2px; }
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 12px; }
.card {
  background: var(--bg1); border: 1px solid var(--border2); border-radius: 8px;
  padding: 12px 16px; display: flex; flex-direction: column; gap: 8px;
}
.card h2 { font-size: 13px; font-weight: 600; }
.card-head { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; }
.sub { font-size: 11px; color: var(--text2); }
.checks { list-style: none; display: flex; flex-direction: column; gap: 4px; }
.checks li { display: flex; gap: 6px; align-items: flex-start; font-size: 12px; color: var(--text0); }
.checks li :deep(svg) { color: var(--warn); flex-shrink: 0; margin-top: 3px; }
.ok { display: flex; align-items: center; gap: 6px; color: var(--accent); font-size: 12px; }
.plan { display: grid; grid-template-columns: auto 1fr; gap: 4px 14px; font-size: 12px; }
.plan dt { color: var(--text1); }
.plan dd { color: var(--text0); font-family: var(--mono); }
.start-row { display: flex; align-items: center; gap: 12px; }
.big-start :deep(.btn) { font-size: 13px; padding-top: 8px; padding-bottom: 8px; }
.big-start :deep(.start-btn) { min-width: 140px; }
/* 按鈕在面板左側：選單往右展開，否則會被面板左緣切掉 */
.big-start :deep(.menu) { left: 0; right: auto; }
.big-start { width: auto; }
.start-hint { font-size: 12px; color: var(--warn); }
.th-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 8px 16px; }
.th { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--text1); }
.th-in { display: inline-flex; align-items: center; gap: 4px; }
.th input {
  width: 72px; padding: 4px 6px; text-align: right;
  background: var(--input-bg); border: 1px solid var(--border2); border-radius: var(--radius-sm);
  color: var(--text0); font-family: var(--mono); font-size: 12px;
}
.unit { width: 18px; color: var(--text2); font-size: 11px; }
.link { border: none; background: none; color: var(--blue); cursor: pointer; font-size: 12px; padding: 0; }
.link:hover { text-decoration: underline; }
.card.last { flex-direction: row; align-items: center; gap: 20px; flex-wrap: wrap; }
.kv { display: flex; flex-direction: column; font-size: 11px; color: var(--text1); }
.kv b { font-family: var(--mono); font-size: 15px; font-weight: 500; color: var(--text0); }
.see { margin-left: auto; }
.verdict { padding: 2px 10px; border-radius: 999px; font-size: 12px; font-weight: 600; }
.verdict.pass   { color: var(--accent); background: rgba(var(--accent-rgb),0.12); }
.verdict.fail   { color: var(--danger); background: rgba(var(--danger-rgb),0.12); }
.verdict.nodata { color: var(--text1);  background: var(--bg3); }
</style>
