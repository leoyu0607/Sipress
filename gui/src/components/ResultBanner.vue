<script setup lang="ts">
/**
 * 結果階段的上方結論：判定（通過 / 未通過 / 無法判定）、一句話摘要、各項判定與關鍵數字，
 * 以及下一步（整合報告、用同樣設定再跑一次、調整設定）。
 * 判定由 Rust 依「判定標準」計算，規則與整合報告相同。
 */
import { computed, ref } from 'vue'
import { useTestStore, MODE_LABEL } from '../stores/testStore'
import IntegratedReportDialog from './IntegratedReportDialog.vue'
import Icon from './Icon.vue'

const store = useTestStore()
const reportOpen = ref(false)

const mode = computed(() => store.config.mode)
const verdict = computed(() => store.verdicts[mode.value])
const failedRun = computed(() => store.status === 'error')

const VERDICT_LABEL = { pass: '通過', fail: '未通過', nodata: '無法判定' } as const
const tag = computed(() =>
  failedRun.value ? { cls: 'fail', text: '執行失敗' }
  : verdict.value ? { cls: verdict.value.verdict, text: VERDICT_LABEL[verdict.value.verdict] }
  : { cls: 'nodata', text: '判定中…' }
)

/** 失敗回應碼，多的在前（不含 200） */
const failCodes = computed(() =>
  Object.entries(store.respCodes)
    .filter(([c, n]) => c !== '200' && n > 0)
    .sort((a, b) => b[1] - a[1])
)
const CODE_NAME: Record<string, string> = {
  '408': '逾時', '480': '暫時無法接通', '486': '忙線', '487': '已取消', '503': '服務無法使用', '603': '拒接',
}
const codeText = (c: string) => CODE_NAME[c] ? `${c} ${CODE_NAME[c]}` : c

const fmt = (v: number, d = 0) => v.toLocaleString(undefined, { minimumFractionDigits: d, maximumFractionDigits: d })

const sentence = computed(() => {
  const m = store.metrics
  const parts: string[] = []
  if (mode.value === 'caller') parts.push(`發起 ${fmt(m.initiated)} 通，接通率 ${fmt(m.asr, 1)}%`)
  else parts.push(`接聽 ${fmt(m.succeeded)} 通`)
  if (store.rtpMetrics.enabled) parts.push(`MOS ${fmt(store.rtpMetrics.mosJb200 ?? store.rtpMetrics.mos, 2)}`)
  let tail = ''
  const v = verdict.value
  if (failedRun.value) tail = '測試沒有正常結束，請看事件記錄。'
  else if (v?.verdict === 'pass') tail = '各項都在判定標準內。'
  else if (v?.verdict === 'fail') tail = `未達標：${v.checks.filter(c => c.pass === false).map(c => c.name).join('、')}。`
  else if (v?.verdict === 'nodata') tail = '沒有足夠的數據可以判定。'
  const top = failCodes.value[0]
  const why = top && m.failed > 0 ? `失敗主要是 ${codeText(top[0])}（${fmt(top[1])} 通）。` : ''
  return `${parts.join('，')}。${tail}${why}`
})

const fmtValue = (v: number | null, unit: string) =>
  v === null ? '—' : `${Number.isInteger(v) ? v : v.toFixed(2)}${unit === 'ms' ? ' ms' : unit}`

/** 關鍵數字 */
const keys = computed(() => {
  const m = store.metrics, r = store.rtpMetrics
  const list: { label: string; value: string; sub: string }[] = []
  if (mode.value === 'caller') {
    list.push({ label: '接通率 ASR', value: `${fmt(m.asr, 1)}%`, sub: `接通 ${fmt(m.succeeded)} / 發起 ${fmt(m.initiated)}` })
  } else {
    list.push({ label: '已接聽', value: fmt(m.succeeded), sub: `${store.agents.total} 個座席帳號` })
  }
  list.push(r.enabled
    ? { label: 'MOS·JB', value: fmt(r.mosJb200 ?? r.mos, 2), sub: `掉包 ${fmt(r.packetLoss, 2)}% · Jitter ${fmt(r.jitter, 1)} ms` }
    : { label: 'MOS', value: '—', sub: '這次沒有收送語音' })
  if (mode.value === 'caller') list.push({ label: '撥號延遲 p50', value: `${fmt(m.pdd)} ms`, sub: '送出 INVITE 到響鈴' })
  list.push({
    label: '失敗', value: `${fmt(m.failed)} 通`,
    sub: failCodes.value.slice(0, 3).map(([c, n]) => `${c} ×${fmt(n)}`).join(' · ') || '沒有失敗',
  })
  return list
})

const canRerun = computed(() => store.validationErrors.length === 0 && !store.isRunning)
function rerun() { void store.startTest() }
</script>

<template>
  <section class="result-banner" aria-labelledby="result-title">
    <div class="left">
      <div class="meta">
        <span class="tag" :class="tag.cls">{{ tag.text }}</span>
        <span class="side">{{ MODE_LABEL[mode] }}</span>
      </div>
      <h1 id="result-title">{{ sentence }}</h1>
      <ul v-if="verdict && verdict.checks.length" class="checks" aria-label="各項判定">
        <li v-for="c in verdict.checks" :key="c.name + c.scope"
            :class="c.pass === null ? 'na' : c.pass ? 'ok' : 'bad'" :title="c.note">
          <Icon v-if="c.pass !== null" :name="c.pass ? 'check' : 'x'" :size="11" /><span v-else aria-hidden="true">–</span>
          {{ c.name }} {{ fmtValue(c.value, c.unit) }}<span class="lim">（{{ c.limit }}{{ c.unit === 'ms' ? ' ms' : c.unit }}）</span>
        </li>
      </ul>
      <div class="actions">
        <button type="button" class="btn btn-primary" @click="reportOpen = true"><Icon name="file-text" :size="12" />整合報告…</button>
        <button type="button" class="btn" :disabled="!canRerun" :title="canRerun ? '設定不變，再跑一次' : store.validationErrors.join('\n')" @click="rerun">
          <Icon name="play" :size="12" />用同樣設定再跑一次
        </button>
        <button type="button" class="btn" @click="store.goStage('setup')"><Icon name="settings" :size="12" />調整設定</button>
      </div>
    </div>
    <div class="keys">
      <div v-for="k in keys" :key="k.label" class="key">
        <div class="k-label">{{ k.label }}</div>
        <div class="k-value">{{ k.value }}</div>
        <div class="k-sub">{{ k.sub }}</div>
      </div>
    </div>
    <IntegratedReportDialog :visible="reportOpen" @close="reportOpen = false" />
  </section>
</template>

<style scoped>
.result-banner {
  flex-shrink: 0; display: flex; gap: 20px; align-items: flex-start; flex-wrap: wrap;
  padding: 14px 18px; background: var(--bg1); border-bottom: 1px solid var(--border);
}
.left { flex: 1 1 380px; min-width: 0; display: flex; flex-direction: column; gap: 8px; }
.meta { display: flex; align-items: center; gap: 8px; }
.tag { padding: 2px 10px; border-radius: 999px; font-size: 12px; font-weight: 600; }
.tag.pass   { color: var(--accent); background: rgba(var(--accent-rgb),0.12); }
.tag.fail   { color: var(--danger); background: rgba(var(--danger-rgb),0.12); }
.tag.nodata { color: var(--text1);  background: var(--bg3); }
.side { font-size: 12px; color: var(--text1); }
h1 { font-size: 15px; font-weight: 600; line-height: 1.5; color: var(--text0); }
.checks { list-style: none; display: flex; flex-wrap: wrap; gap: 6px; }
.checks li {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 2px 8px; border-radius: var(--radius-sm); font-size: 11px; font-family: var(--mono);
  border: 1px solid var(--border2); color: var(--text0);
}
.checks li.ok :deep(svg)  { color: var(--accent); }
.checks li.bad { border-color: rgba(var(--danger-rgb),0.45); }
.checks li.bad :deep(svg) { color: var(--danger); }
.checks li.na  { color: var(--text1); }
.lim { color: var(--text2); }
.actions { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 2px; }
.keys { flex: 1 1 460px; display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 8px; }
.key { background: var(--bg2); border-radius: 8px; padding: 8px 12px; min-width: 0; }
.k-label { font-size: 11px; color: var(--text1); }
.k-value { font-family: var(--mono); font-size: 20px; color: var(--text0); }
.k-sub { font-size: 11px; color: var(--text2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
