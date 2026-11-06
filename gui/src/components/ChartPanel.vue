<script setup lang="ts">
/**
 * ChartPanel.vue
 *
 * - 監聽 store.seriesTick 重繪（系列長度達上限後仍會更新）
 * - X 軸秒數 = seriesOffset + index（捨棄舊資料後時間仍正確）
 * - 顏色全部取自 CSS 變數，跟隨亮/暗主題
 * - 滑鼠移入顯示該秒數值；最新點旁標示目前值
 * - MOS tab 固定 y 軸 1~5 並畫品質參考線（≥4.0 / ≥3.0）
 * - 可暫停：凍結畫面上的資料（背景仍持續收集），方便看清楚某一段；新測試開始或切換檢視端時自動恢復
 * - 圖表下方有文字摘要（目前 / 平均 / 最高 / 最低），canvas 以 role="img" 提供同樣內容給螢幕閱讀器
 * - 鍵盤：圖表取得焦點後 ← → 逐點查看數值、Home / End 到頭尾、Esc 離開
 */
import { ref, computed, watch, onMounted, onUnmounted, nextTick, inject } from 'vue'
import { useTestStore, mosGrade, type SeriesKey } from '../stores/testStore'
import Icon from './Icon.vue'

const store = useTestStore()
const isDark = inject<ReturnType<typeof ref<boolean>>>('isDark')!

const canvasRef   = ref<HTMLCanvasElement | null>(null)
const activeChart = ref<SeriesKey>('cps')
const hoverX      = ref<number | null>(null)
/** 鍵盤查看中的資料點索引（null = 未用鍵盤查看） */
const kbIdx       = ref<number | null>(null)

// ── 暫停：凍結各 tab 的資料快照 ─────────────────────────────────
interface SeriesView { data: number[]; offset: number }
const paused = ref(false)
const frozen = ref<Partial<Record<SeriesKey, SeriesView>> | null>(null)

function viewOf(key: SeriesKey): SeriesView {
  const f = paused.value ? frozen.value?.[key] : undefined
  return f ?? { data: store.series[key], offset: store.seriesOffset[key] }
}
function togglePause() {
  if (paused.value) {
    paused.value = false
    frozen.value = null
  } else {
    const snap: Partial<Record<SeriesKey, SeriesView>> = {}
    for (const k of TABS) snap[k] = { data: store.series[k].slice(), offset: store.seriesOffset[k] }
    frozen.value = snap
    paused.value = true
  }
}
// 新測試開始、或切換檢視的一端時，凍結的畫面已不相干 → 自動恢復
watch([() => store.status, () => store.config.mode], ([st, mode], [prevSt, prevMode]) => {
  if (!paused.value) return
  if (mode !== prevMode || (st === 'running' && prevSt !== 'running')) togglePause()
})

const current = computed(() => viewOf(activeChart.value))
const canPause = computed(() => paused.value || current.value.data.length >= 2)

// ── 文字摘要 ───────────────────────────────────────────────────
const summary = computed(() => {
  const key = activeChart.value
  const { data, offset } = current.value
  if (data.length < 2) return ''
  const last = data[data.length - 1]
  const avg = data.reduce((a, b) => a + b, 0) / data.length
  const parts = [
    `目前 ${fmtValue(key, last)}`,
    `平均 ${fmtValue(key, avg)}`,
    `最高 ${fmtValue(key, Math.max(...data))}`,
    `最低 ${fmtValue(key, Math.min(...data))}`,
  ]
  const span = `${offset}–${offset + data.length - 1}s`
  return `${parts.join(' · ')}（${span}${paused.value ? '，已暫停' : ''}）`
})
const canvasLabel = computed(() =>
  summary.value ? `${CHART_LABELS[activeChart.value]}：${summary.value}` : `${CHART_LABELS[activeChart.value]}：尚無資料`)

/** 每個圖表使用的 CSS 色彩變數 */
const CHART_VARS: Record<SeriesKey, string> = {
  cps:  '--accent',
  conc: '--blue',
  asr:  '--blue',
  ccr:  '--accent',
  pdd:  '--warn',
  fail: '--danger',
  mos:  '--purple',
}

const CHART_LABELS: Record<SeriesKey, string> = {
  cps:  'CPS — 每秒發起通數',
  conc: '併發 — 進行中通話',
  asr:  'ASR — 接通率 %',
  ccr:  'CCR — 完成率 %',
  pdd:  'PDD — 撥號延遲中位數（p50）ms',
  fail: '失敗 — 累計失敗 + 逾時',
  mos:  'MOS — 通話品質 (1~5)',
}

const TABS: SeriesKey[] = ['cps', 'conc', 'asr', 'ccr', 'pdd', 'fail', 'mos']

const mosDisabled = computed(() => !store.rtpMetrics.enabled && store.series.mos.length === 0)

let dpr = 1

function cssVar(name: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

/** 從目前 theme 取得 canvas 繪圖顏色 */
function themeColors() {
  const dark = isDark.value
  return {
    grid:  dark ? 'rgba(255,255,255,0.05)' : 'rgba(0,0,0,0.07)',
    label: cssVar('--text2'),
    empty: cssVar('--text2'),
    text0: cssVar('--text0'),
    bg:    cssVar('--bg1'),
  }
}

function fmtValue(key: SeriesKey, v: number) {
  return key === 'pdd'  ? `${v.toFixed(0)}ms`
       : key === 'asr' || key === 'ccr' ? `${v.toFixed(1)}%`
       : key === 'mos'  ? v.toFixed(2)
       : key === 'conc' || key === 'fail' ? `${Math.round(v)}`
       : v.toFixed(1)
}

function fmtAxis(key: SeriesKey, v: number) {
  return key === 'pdd'  ? `${v.toFixed(0)}ms`
       : key === 'asr' || key === 'ccr' ? `${v.toFixed(0)}%`
       : key === 'mos'  ? v.toFixed(1)
       : key === 'conc' || key === 'fail' ? `${Math.round(v)}`
       : v.toFixed(1)
}

function resize() {
  const canvas = canvasRef.value
  if (!canvas) return
  dpr = window.devicePixelRatio || 1
  const { width, height } = canvas.getBoundingClientRect()
  canvas.width  = width  * dpr
  canvas.height = height * dpr
  draw()
}

function drawPill(ctx: CanvasRenderingContext2D, text: string, x: number, y: number, color: string, bg: string, W: number) {
  ctx.font = '500 11px JetBrains Mono, monospace'
  const w = ctx.measureText(text).width + 10
  const h = 16
  let px = x + 8
  if (px + w > W - 2) px = x - 8 - w
  const py = Math.max(2, y - h / 2)
  ctx.fillStyle = bg
  ctx.strokeStyle = color
  ctx.lineWidth = 1
  ctx.beginPath()
  ctx.roundRect(px, py, w, h, 3)
  ctx.fill(); ctx.stroke()
  ctx.fillStyle = color
  ctx.textAlign = 'left'
  ctx.fillText(text, px + 5, py + 11.5)
}

function draw() {
  const canvas = canvasRef.value
  if (!canvas) return
  const ctx = canvas.getContext('2d')!
  const W   = canvas.width  / dpr
  const H   = canvas.height / dpr
  ctx.setTransform(1, 0, 0, 1, 0, 0)
  ctx.clearRect(0, 0, canvas.width, canvas.height)
  ctx.scale(dpr, dpr)

  const { grid, label, empty, text0, bg } = themeColors()

  const key    = activeChart.value
  const { data, offset } = viewOf(key)

  if (data.length < 2) {
    ctx.fillStyle = empty
    ctx.font = '11px JetBrains Mono, Microsoft JhengHei, monospace'
    ctx.textAlign = 'center'
    const msg =
        key === 'mos' && mosDisabled.value ? '未啟用 RTP（勾選「播放音檔」並選擇檔案）'
      : store.status === 'idle'            ? '開始測試後顯示即時圖表'
      : key === 'pdd'                      ? '等待第一通接通…'
      : '等待資料…'
    ctx.fillText(msg, W / 2, H / 2)
    return
  }

  const pad = { l: 52, r: 14, t: 14, b: 28 }
  const cW  = W - pad.l - pad.r
  const cH  = H - pad.t - pad.b
  const color = cssVar(CHART_VARS[key])

  // MOS 用固定 y 軸 1~5；其他用 max * 1.15
  const isMos = key === 'mos'
  const maxV  = isMos ? 5 : (Math.max(...data) * 1.15 || 1)
  const minV  = isMos ? 1 : 0
  const range = maxV - minV
  const xStep = cW / (data.length - 1)

  // Grid lines + y-axis labels
  ctx.font      = '11px JetBrains Mono, monospace'
  ctx.textAlign = 'right'
  const steps   = 4
  for (let i = 0; i <= steps; i++) {
    const frac = i / steps
    const y    = pad.t + cH - frac * cH
    ctx.strokeStyle = grid
    ctx.lineWidth   = 1
    ctx.beginPath(); ctx.moveTo(pad.l, y); ctx.lineTo(pad.l + cW, y); ctx.stroke()
    ctx.fillStyle = label
    ctx.fillText(fmtAxis(key, minV + frac * range), pad.l - 5, y + 3)
  }

  // MOS 參考線 ≥4.0（優） / ≥3.0（普通）
  if (isMos) {
    const drawRefLine = (val: number, c: string, lbl: string) => {
      const y = pad.t + cH - ((val - minV) / range) * cH
      ctx.setLineDash([4, 3])
      ctx.strokeStyle = c
      ctx.globalAlpha = 0.6
      ctx.lineWidth   = 1
      ctx.beginPath(); ctx.moveTo(pad.l, y); ctx.lineTo(pad.l + cW, y); ctx.stroke()
      ctx.setLineDash([])
      ctx.fillStyle   = c
      ctx.textAlign   = 'left'
      ctx.fillText(lbl, pad.l + 4, y - 3)
      ctx.globalAlpha = 1
    }
    drawRefLine(4.0, cssVar('--accent'), '4.0 優')
    drawRefLine(3.0, cssVar('--warn'),   '3.0 普通')
  }

  // 座標點
  const pts: [number, number][] = data.map((v, i) => [
    pad.l + i * xStep,
    pad.t + cH - Math.max(0, Math.min(1, (v - minV) / range)) * cH,
  ])

  // Fill area
  ctx.beginPath()
  ctx.moveTo(pts[0][0], pad.t + cH)
  for (const [x, y] of pts) ctx.lineTo(x, y)
  ctx.lineTo(pts[pts.length - 1][0], pad.t + cH)
  ctx.closePath()
  const grad = ctx.createLinearGradient(0, pad.t, 0, pad.t + cH)
  ctx.globalAlpha = 1
  grad.addColorStop(0, color)
  grad.addColorStop(1, 'transparent')
  ctx.globalAlpha = 0.16
  ctx.fillStyle = grad
  ctx.fill()
  ctx.globalAlpha = 1

  // Line
  ctx.beginPath()
  ctx.moveTo(pts[0][0], pts[0][1])
  for (let i = 1; i < pts.length; i++) {
    const [px, py] = pts[i - 1]
    const [nx, ny] = pts[i]
    const cpx = (px + nx) / 2
    ctx.bezierCurveTo(cpx, py, cpx, ny, nx, ny)
  }
  ctx.strokeStyle = color
  ctx.lineWidth   = 1.5
  ctx.lineJoin    = 'round'
  ctx.stroke()

  // X-axis ticks（實際經過秒數）
  ctx.textAlign  = 'center'
  ctx.fillStyle  = label
  ctx.font       = '11px JetBrains Mono, monospace'
  const every    = Math.max(1, Math.ceil(data.length / 6))
  for (let i = 0; i < data.length; i += every) {
    ctx.fillText(`${offset + i}s`, pad.l + i * xStep, H - 8)
  }

  // Hover crosshair 或最新點
  const hx = hoverX.value
  const hi = kbIdx.value !== null
    ? Math.max(0, Math.min(data.length - 1, kbIdx.value))
    : hx !== null && hx >= pad.l - xStep / 2 && hx <= pad.l + cW + xStep / 2
    ? Math.max(0, Math.min(data.length - 1, Math.round((hx - pad.l) / xStep)))
    : null
  const idx = hi ?? data.length - 1
  const [lx, ly] = pts[idx]

  if (hi !== null) {
    ctx.strokeStyle = label
    ctx.globalAlpha = 0.5
    ctx.lineWidth   = 1
    ctx.beginPath(); ctx.moveTo(lx, pad.t); ctx.lineTo(lx, pad.t + cH); ctx.stroke()
    ctx.globalAlpha = 1
  }

  ctx.globalAlpha = 0.25
  ctx.beginPath(); ctx.arc(lx, ly, 6, 0, Math.PI * 2)
  ctx.fillStyle = color; ctx.fill()
  ctx.globalAlpha = 1
  ctx.beginPath(); ctx.arc(lx, ly, 3, 0, Math.PI * 2)
  ctx.fillStyle = color; ctx.fill()

  const text = hi !== null
    ? `${offset + idx}s  ${fmtValue(key, data[idx])}`
    : fmtValue(key, data[idx])
  drawPill(ctx, text, lx, ly, hi !== null ? text0 : color, bg, W)
}

function switchChart(key: SeriesKey) {
  if (key === 'mos' && mosDisabled.value) return
  activeChart.value = key
  kbIdx.value = null
}

/** 鍵盤查看：← → 逐點、Home / End、Esc 離開 */
function onCanvasKey(e: KeyboardEvent) {
  const n = current.value.data.length
  if (n < 2) return
  const at = kbIdx.value ?? n - 1
  const next =
      e.key === 'ArrowLeft'  ? at - 1
    : e.key === 'ArrowRight' ? at + 1
    : e.key === 'Home'       ? 0
    : e.key === 'End'        ? n - 1
    : undefined
  if (e.key === 'Escape') { kbIdx.value = null; return }
  if (next === undefined) return
  e.preventDefault()
  kbIdx.value = Math.max(0, Math.min(n - 1, next))
}
/** 鍵盤查看中那一點的文字（給螢幕閱讀器的 live region） */
const kbText = computed(() => {
  if (kbIdx.value === null) return ''
  const { data, offset } = current.value
  const i = Math.min(kbIdx.value, data.length - 1)
  return i >= 0 ? `${offset + i}s：${fmtValue(activeChart.value, data[i])}` : ''
})

function onMouseMove(e: MouseEvent) {
  const rect = canvasRef.value?.getBoundingClientRect()
  if (!rect) return
  hoverX.value = e.clientX - rect.left
}

// 資料、主題、tab 或滑鼠位置變化時重繪
watch([() => store.seriesTick, () => store.status, () => store.config.mode, isDark, activeChart, hoverX, kbIdx, paused], () => nextTick(draw))

const ro = new ResizeObserver(() => { resize() })

onMounted(() => {
  if (canvasRef.value) {
    ro.observe(canvasRef.value.parentElement!)
    nextTick(resize)
  }
})
onUnmounted(() => ro.disconnect())
</script>

<template>
  <div class="chart-panel">
    <div class="panel-hdr">
      <span class="panel-title" :title="CHART_LABELS[activeChart]">{{ CHART_LABELS[activeChart] }}</span>

      <!-- RTP badge：若 RTP 啟用顯示 MOS 即時值 -->
      <div v-if="store.rtpMetrics.enabled" class="rtp-badge">
        <span class="rtp-dot" :class="store.mosRating"></span>
        MOS {{ store.rtpMetrics.mos.toFixed(2) }} · {{ mosGrade(store.rtpMetrics.mos).label }}
      </div>

      <button class="btn btn-sm pause-btn" type="button" :class="{ active: paused }"
              :aria-pressed="paused" :disabled="!canPause"
              :title="paused ? '繼續更新圖表（資料在暫停期間仍持續收集）' : '暫停更新圖表，方便查看目前畫面'"
              @click="togglePause">
        <Icon :name="paused ? 'play' : 'pause'" :size="12" />{{ paused ? '繼續' : '暫停' }}
      </button>

      <div class="tabs" role="tablist">
        <button
          v-for="key in TABS"
          :key="key"
          class="tab"
          role="tab"
          :aria-selected="activeChart === key"
          :class="{ active: activeChart === key, 'tab-rtp': key === 'mos' }"
          :disabled="key === 'mos' && mosDisabled"
          :title="key === 'mos' && mosDisabled ? '啟用「播放音檔」後顯示' : CHART_LABELS[key]"
          @click="switchChart(key)"
        >{{ key === 'conc' ? 'CONC' : key.toUpperCase() }}</button>
      </div>
    </div>

    <p class="chart-summary" :class="{ paused }" aria-hidden="true">{{ summary || ' ' }}</p>

    <div class="canvas-wrap">
      <canvas ref="canvasRef" role="img" tabindex="0" :aria-label="canvasLabel"
              title="← → 逐點查看數值，Esc 離開"
              @mousemove="onMouseMove" @mouseleave="hoverX = null"
              @keydown="onCanvasKey" @blur="kbIdx = null"></canvas>
      <span class="sr-only" aria-live="polite">{{ kbText }}</span>
    </div>
  </div>
</template>

<style scoped>
.chart-panel {
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 10px 14px 10px;
  flex: 1;
  min-width: 0;
}

.panel-hdr {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px 10px;
  margin-bottom: 8px;
  flex-shrink: 0;
}

.panel-title {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  flex: 1 1 0;               /* 空間不夠時標題先截斷（有 hover 提示），MOS 徽章與分頁維持同一行 */
}

/* RTP MOS badge */
.rtp-badge {
  display: flex;
  align-items: center;
  gap: 5px;
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text1);
  background: var(--bg3);
  border: 1px solid var(--border2);
  padding: 2px 8px;
  border-radius: 99px;
}
.rtp-dot {
  width: 6px; height: 6px;
  border-radius: 50%;
  background: var(--text2);
}
.rtp-dot.excellent { background: var(--accent); }
.rtp-dot.fair      { background: var(--warn); }
.rtp-dot.poor      { background: var(--danger); }

.tabs {
  display: flex;
  gap: 1px;
  margin-left: auto;
  background: var(--bg2);
  padding: 2px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border);
}

.tab {
  padding: 3px 8px;
  font-family: var(--mono);
  font-size: 11px;
  font-weight: 500;
  color: var(--text2);
  border-radius: 3px;
  cursor: pointer;
  border: none;
  background: transparent;
  transition: all 0.15s;
}
.tab:hover:not(:disabled) { color: var(--text1); }
.tab.active { background: var(--bg4); color: var(--text0); }

/* MOS tab — 紫色強調 */
.tab.tab-rtp { color: var(--purple); }
.tab.tab-rtp.active { background: rgba(var(--purple-rgb),0.15); color: var(--purple); }
.tab:disabled { opacity: 0.35; cursor: not-allowed; }

.canvas-wrap {
  flex: 1;
  position: relative;
  min-height: 0;
}
canvas:focus-visible { outline: 2px solid rgba(var(--accent-rgb),0.6); outline-offset: 2px; border-radius: var(--radius-sm); }

.pause-btn { flex-shrink: 0; }
.pause-btn.active { color: var(--warn); border-color: rgba(var(--warn-rgb),0.45); background: rgba(var(--warn-rgb),0.1); }

.chart-summary {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text2);
  margin: -2px 0 6px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex-shrink: 0;
}
.chart-summary.paused { color: var(--warn); }

canvas {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  display: block;
  cursor: crosshair;
}
</style>
