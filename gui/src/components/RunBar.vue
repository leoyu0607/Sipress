<script setup lang="ts">
/**
 * 執行中階段的上方列：鎖定的設定摘要、進度與時間。
 * 測試結束後改成「測試已結束」並提供「查看結果」。
 */
import { computed } from 'vue'
import { useTestStore, MODE_LABEL } from '../stores/testStore'
import Icon from './Icon.vue'

const store = useTestStore()

const AUDIO_LABEL: Record<string, string> = { silence: '不送語音', loop: '循環播放音檔', flow: '通話流程' }

const summary = computed(() => {
  const c = store.config
  const target = c.server || '（未設定伺服器）'
  if (c.mode === 'caller') {
    const p = c.caller
    const callee = p.calleeFixed || (p.calleePrefix ? `${p.calleePrefix}…` : '')
    return [
      callee ? `${target} → ${callee}` : target,
      `${p.concurrency} 併發`,
      p.cpsPattern.type === 'flat' ? `每秒 ${p.cps} 通` : `速率：${p.cpsPattern.type}`,
      AUDIO_LABEL[p.audioMode],
    ].join(' · ')
  }
  return [target, `${c.agent.accounts.length} 個座席帳號`].join(' · ')
})

const finished = computed(() => store.status === 'done' || store.status === 'error')
const duration = computed(() => store.config[store.config.mode].duration)

function clock(s: number) {
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}
const timeText = computed(() =>
  duration.value > 0 ? `${clock(store.elapsedSec)} / ${clock(duration.value)}` : clock(store.elapsedSec)
)
const pct = computed(() => finished.value ? 100 : store.progressPct)
</script>

<template>
  <div class="run-bar" :class="{ finished }">
    <template v-if="!finished">
      <Icon name="lock" :size="12" class="lock" />
      <span class="summary" :title="summary">{{ MODE_LABEL[store.config.mode] }} · {{ summary }}</span>
      <button type="button" class="link" @click="store.goStage('setup')">檢視設定</button>
    </template>
    <template v-else>
      <span class="done-text" role="status">
        <Icon :name="store.status === 'error' ? 'alert' : 'check'" :size="13" />
        {{ MODE_LABEL[store.config.mode] }}測試{{ store.status === 'error' ? '執行失敗' : '已結束' }}
      </span>
      <span class="summary">{{ summary }}</span>
      <button type="button" class="btn btn-primary btn-sm see" @click="store.goStage('result')">查看結果 →</button>
    </template>

    <div class="progress" :title="duration > 0 ? `已跑 ${pct.toFixed(0)}%` : '持續時間不限'">
      <div v-if="duration > 0 || finished" class="track"><div class="fill" :style="{ width: pct + '%' }"></div></div>
      <span class="time">{{ timeText }}</span>
    </div>
  </div>
</template>

<style scoped>
.run-bar {
  flex-shrink: 0; display: flex; align-items: center; gap: 12px;
  padding: 7px 14px; background: var(--bg1); border-bottom: 1px solid var(--border);
  font-size: 12px; min-width: 0;
}
.run-bar.finished { background: linear-gradient(rgba(var(--accent-rgb),0.06), rgba(var(--accent-rgb),0.06)), var(--bg1); }
.lock { color: var(--text2); flex-shrink: 0; }
.summary { color: var(--text1); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.done-text { display: inline-flex; align-items: center; gap: 6px; color: var(--accent); font-weight: 600; white-space: nowrap; }
.link { border: none; background: none; color: var(--blue); cursor: pointer; font-size: 12px; padding: 0; white-space: nowrap; }
.link:hover { text-decoration: underline; }
.see { white-space: nowrap; }
.progress { margin-left: auto; display: flex; align-items: center; gap: 10px; flex-shrink: 0; }
.track { width: 220px; height: 6px; border-radius: 3px; background: var(--bg3); overflow: hidden; }
.fill { height: 100%; background: var(--accent); transition: width 0.4s; }
.time { font-family: var(--mono); color: var(--text0); }
</style>
