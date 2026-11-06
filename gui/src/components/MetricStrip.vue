<script setup lang="ts">
import { computed } from 'vue'
import { useTestStore, asrGrade, errGrade, regGrade } from '../stores/testStore'

const store = useTestStore()

const isReady = computed(() => store.status !== 'idle')

// 分級同時給顏色與文字（顯示在單位那一行），不只靠顏色。
// 還沒有任何通話時 ASR / 錯誤率是 0，不代表「偏低」，不分級
const hasCalls = computed(() => isReady.value && store.metrics.initiated > 0)
const asr = computed(() => hasCalls.value ? asrGrade(store.metrics.asr) : null)
const err = computed(() => hasCalls.value ? errGrade(store.metrics.errorRate) : null)
const reg = computed(() => {
  const { registered, total } = store.agents
  return isReady.value && total > 0 ? regGrade(registered, total) : null
})
</script>

<template>
  <div class="metric-strip">

    <div class="cell">
      <div class="name">CPS</div>
      <div class="val c-accent">{{ isReady ? store.metrics.cps.toFixed(1) : '—' }}</div>
      <div class="unit">每秒發起</div>
    </div>

    <div class="cell">
      <div class="name">CONCUR</div>
      <div class="val c-blue">{{ isReady ? store.metrics.concurrency : '—' }}</div>
      <div class="unit">進行中通話</div>
    </div>

    <div class="cell">
      <div class="name">SUCCESS</div>
      <div class="val c-accent">{{ isReady ? store.metrics.succeeded.toLocaleString() : '—' }}</div>
      <div class="unit">已接通</div>
    </div>

    <div class="cell">
      <div class="name">FAILED</div>
      <div class="val c-danger">{{ isReady ? store.metrics.failed.toLocaleString() : '—' }}</div>
      <div class="unit">失敗 + 逾時</div>
    </div>

    <div class="cell">
      <div class="name">TOTAL</div>
      <div class="val">{{ isReady ? store.metrics.initiated.toLocaleString() : '—' }}</div>
      <div class="unit">累計發起</div>
    </div>

    <div class="cell">
      <div class="name">ASR</div>
      <div class="val" :style="asr && { color: asr.color }">
        {{ isReady ? store.metrics.asr.toFixed(1) + '%' : '—' }}
      </div>
      <div class="unit">接通率<span v-if="asr" class="grade" :style="{ color: asr.color }"> · {{ asr.label }}</span></div>
    </div>

    <div class="cell">
      <div class="name">ERR%</div>
      <div class="val" :style="err && { color: err.color }">
        {{ isReady ? store.metrics.errorRate.toFixed(1) + '%' : '—' }}
      </div>
      <div class="unit">錯誤率<span v-if="err" class="grade" :style="{ color: err.color }"> · {{ err.label }}</span></div>
    </div>

    <!-- 座席端量不到 PDD，改顯示註冊狀態 -->
    <div class="cell" v-if="store.config.mode === 'agent'">
      <div class="name">REG</div>
      <div class="val" :style="reg && { color: reg.color }">
        {{ isReady && store.agents.total > 0 ? `${store.agents.registered}/${store.agents.total}` : '—' }}
      </div>
      <div class="unit">已註冊座席<span v-if="reg" class="grade" :style="{ color: reg.color }"> · {{ reg.label }}</span></div>
    </div>

    <div class="cell" v-else>
      <div class="name">PDD</div>
      <div class="val c-warn">{{ isReady && store.metrics.pdd > 0 ? store.metrics.pdd.toFixed(0) + 'ms' : '—' }}</div>
      <div class="unit">撥號延遲 p50</div>
    </div>

  </div>
</template>

<style scoped>
.metric-strip {
  display: grid;
  grid-template-columns: repeat(8, 1fr);
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.cell {
  min-width: 0;
  padding: 10px 12px;
  border-right: 1px solid var(--border);
  cursor: default;
  transition: background 0.15s;
}
.cell:last-child { border-right: none; }
.cell:hover { background: var(--bg1); }

.name {
  font-family: var(--mono);
  font-size: 11px;
  font-weight: 500;
  letter-spacing: 0.1em;
  color: var(--text2);
  text-transform: uppercase;
  margin-bottom: 3px;
}

.val {
  font-family: var(--mono);
  font-size: 20px;
  font-weight: 700;
  line-height: 1;
  color: var(--text0);
  transition: color 0.3s;
}
.val.c-accent { color: var(--accent); }
.val.c-blue   { color: var(--blue); }
.val.c-warn   { color: var(--warn); }
.val.c-danger { color: var(--danger); }

.grade { font-weight: 600; }
.unit {
  font-size: 11px;
  color: var(--text2);
  margin-top: 2px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 窄視窗：8 欄 → 4×2 */
@media (max-width: 1080px) {
  .metric-strip { grid-template-columns: repeat(4, 1fr); }
  .cell:nth-child(4n) { border-right: none; }
  .cell:nth-child(-n+4) { border-bottom: 1px solid var(--border); }
  .cell { padding: 7px 12px; }
  .val { font-size: 17px; }
}
</style>
