<script setup lang="ts">
/**
 * 標題列中間的階段步驟：設定 → 執行中 → 結果。
 * 目前階段標成實心；還不能去的階段（例如還沒跑過就看結果）停用。
 */
import { computed } from 'vue'
import { useTestStore, type Stage } from '../stores/testStore'

const store = useTestStore()

const status = computed(() => store.status)
const STEPS: { key: Stage; label: string }[] = [
  { key: 'setup',   label: '設定' },
  { key: 'running', label: '執行中' },
  { key: 'result',  label: '結果' },
]
const ORDER: Record<Stage, number> = { setup: 0, running: 1, result: 2 }

function enabled(s: Stage) {
  if (s === 'setup') return true
  if (s === 'running') return status.value !== 'idle'
  return status.value === 'done' || status.value === 'error'
}
/** 已經過的階段打勾：執行過才算過了設定，測試結束才算過了執行中 */
function passed(s: Stage) {
  if (s === store.stage) return false
  if (s === 'setup') return status.value !== 'idle' && ORDER[store.stage] > 0
  if (s === 'running') return (status.value === 'done' || status.value === 'error') && store.stage === 'result'
  return false
}
function title(s: Stage) {
  if (enabled(s)) return ''
  return s === 'running' ? '還沒開始測試' : '測試結束後才有結果'
}
</script>

<template>
  <nav class="stage-nav" aria-label="測試階段">
    <template v-for="(s, i) in STEPS" :key="s.key">
      <span v-if="i > 0" class="line" :class="{ done: passed(STEPS[i - 1].key) || store.stage === s.key }" aria-hidden="true"></span>
      <button type="button" class="step"
              :class="{ current: store.stage === s.key, passed: passed(s.key), attention: s.key === 'result' && store.finishedUnseen[store.config.mode] }"
              :aria-current="store.stage === s.key ? 'step' : undefined"
              :disabled="!enabled(s.key)" :title="title(s.key)"
              @click="store.goStage(s.key)">
        <span class="num" aria-hidden="true">{{ passed(s.key) ? '✓' : i + 1 }}</span>{{ s.label }}
      </button>
    </template>
  </nav>
</template>

<style scoped>
.stage-nav { display: flex; align-items: center; gap: 6px; -webkit-app-region: no-drag; }
.step {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 3px 8px; border: none; border-radius: var(--radius);
  background: transparent; color: var(--text1);
  font-family: var(--sans); font-size: 12px; cursor: pointer;
}
.step:hover:not(:disabled) { background: var(--bg3); color: var(--text0); }
.step:focus-visible { outline: 2px solid rgba(var(--accent-rgb),0.6); outline-offset: 1px; }
.step:disabled { cursor: not-allowed; color: var(--text2); }
.num {
  width: 18px; height: 18px; border-radius: 50%;
  display: inline-flex; align-items: center; justify-content: center;
  border: 1px solid var(--border3); font-size: 10px; font-family: var(--mono);
}
.step.passed .num { border-color: var(--accent); color: var(--accent); }
.step.current { color: var(--accent); font-weight: 600; }
.step.current .num { background: var(--accent); border-color: var(--accent); color: var(--on-accent, #0b0d12); }
.step.attention:not(.current) { color: var(--accent); }
.step.attention:not(.current) .num { border-color: var(--accent); animation: attn 1.4s ease-in-out infinite; }
@keyframes attn { 0%,100% { box-shadow: 0 0 0 0 rgba(var(--accent-rgb),0.5); } 50% { box-shadow: 0 0 0 4px rgba(var(--accent-rgb),0); } }
.line { width: 18px; height: 1px; background: var(--border3); }
.line.done { background: var(--accent); }
</style>
