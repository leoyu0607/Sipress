<script setup lang="ts">
/**
 * ChartPanel.vue
 *
 * - 監聽 store.seriesTick 重繪（系列長度達上限後仍會更新）
 * - X 軸秒數 = seriesOffset + index（捨棄舊資料後時間仍正確）
 * - 顏色全部取自 CSS 變數，跟隨亮/暗主題
 * - 滑鼠移入顯示該秒數值；最新點旁標示目前值
 * - MOS tab 固定 y 軸 1~5 並畫品質參考線（≥4.0 / ≥3.0）
 */
import { ref, computed, watch, onMounted, onUnmounted, nextTick, inject } from 'vue'
import { useTestStore, type SeriesKey } from '../stores/testStore'

const store = useTestStore()
const isDark = inject<ReturnType<typeof ref<boolean>>>('isDark')!

const canvasRef   = ref<HTMLCanvasElement | null>(null)
const activeChart = ref<SeriesKey>('cps')
const hoverX      = ref<number | null>(null)

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
  pdd:  'PDD — 平均撥號延遲 ms',
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
  ctx.font = '500 10px JetBrains Mono, monospace'
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
  const data   = store.series[key]
  const offset = store.seriesOffset[key]

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
  ctx.font      = '10px JetBrains Mono, monospace'
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
  ctx.font       = '10px JetBrains Mono, monospace'
  const every    = Math.max(1, Math.ceil(data.length / 6))
  for (let i = 0; i < data.length; i += every) {
    ctx.fillText(`${offset + i}s`, pad.l + i * xStep, H - 8)
  }

  // Hover crosshair 或最新點
  const hx = hoverX.value
  const hi = hx !== null && hx >= pad.l - xStep / 2 && hx <= pad.l + cW + xStep / 2
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
}

function onMouseMove(e: MouseEvent) {
  const rect = canvasRef.value?.getBoundingClientRect()
  if (!rect) return
  hoverX.value = e.clientX - rect.left
}

// 資料、主題、tab 或滑鼠位置變化時重繪
watch([() => store.seriesTick, () => store.status, () => store.config.mode, isDark, activeChart, hoverX], () => nextTick(draw))

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
        MOS {{ store.rtpMetrics.mos.toFixed(2) }}
      </div>

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

    <div class="canvas-wrap">
      <canvas ref="canvasRef" @mousemove="onMouseMove" @mouseleave="hoverX = null"></canvas>
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
  flex: 1 1 140px;
}

/* RTP MOS badge */
.rtp-badge {
  display: flex;
  align-items: center;
  gap: 5px;
  font-family: var(--mono);
  font-size: 10px;
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
  font-size: 10px;
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

canvas {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  display: block;
  cursor: crosshair;
}
</style>
