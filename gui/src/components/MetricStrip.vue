<script setup lang="ts">
import { computed } from 'vue'
import { useTestStore } from '../stores/testStore'

const store = useTestStore()

const isReady = computed(() => store.status !== 'idle')

const asrClass = computed(() => {
  if (!isReady.value) return ''
  const v = store.metrics.asr
  return v >= 85 ? 'c-accent' : v >= 70 ? 'c-warn' : 'c-danger'
})

const regClass = computed(() => {
  const { registered, total } = store.agents
  if (!isReady.value || total === 0) return ''
  return registered >= total ? 'c-accent' : registered > 0 ? 'c-warn' : 'c-danger'
})

const errClass = computed(() => {
  if (!isReady.value) return ''
  const v = store.metrics.errorRate
  return v <= 5 ? 'c-accent' : v <= 20 ? 'c-warn' : 'c-danger'
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
      <div class="val" :class="asrClass">
        {{ isReady ? store.metrics.asr.toFixed(1) + '%' : '—' }}
      </div>
      <div class="unit">接通率</div>
    </div>

    <div class="cell">
      <div class="name">ERR%</div>
      <div class="val" :class="errClass">
        {{ isReady ? store.metrics.errorRate.toFixed(1) + '%' : '—' }}
      </div>
      <div class="unit">錯誤率</div>
    </div>

    <!-- 座席端量不到 PDD，改顯示註冊狀態 -->
    <div class="cell" v-if="store.config.mode === 'agent'">
      <div class="name">REG</div>
      <div class="val" :class="regClass">
        {{ isReady && store.agents.total > 0 ? `${store.agents.registered}/${store.agents.total}` : '—' }}
      </div>
      <div class="unit">已註冊座席</div>
    </div>

    <div class="cell" v-else>
      <div class="name">PDD</div>
      <div class="val c-warn">{{ isReady && store.metrics.pdd > 0 ? store.metrics.pdd.toFixed(0) + 'ms' : '—' }}</div>
      <div class="unit">{{ store.status === 'done' ? '撥號延遲 p50' : '撥號延遲 平均' }}</div>
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
  font-size: 10px;
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

.unit {
  font-size: 10px;
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
