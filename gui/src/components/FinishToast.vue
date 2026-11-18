<script setup lang="ts">
/**
 * 測試結束的提示：不會自動切畫面，按「查看結果」才切到結果階段。
 * 兩端各自提示；切到另一端的分頁也看得到。
 */
import { computed } from 'vue'
import { useTestStore, MODE_LABEL, type CallMode } from '../stores/testStore'
import Icon from './Icon.vue'

const store = useTestStore()

const items = computed(() =>
  (['caller', 'agent'] as CallMode[]).filter(m =>
    store.finishedUnseen[m] && !(store.config.mode === m && store.stage === 'result'))
)

function view(m: CallMode) {
  store.config.mode = m
  store.goStage('result', m)
}
function dismiss(m: CallMode) { store.finishedUnseen[m] = false }
</script>

<template>
  <div class="toasts" aria-live="polite">
    <div v-for="m in items" :key="m" class="toast" role="status">
      <Icon :name="store.sessions[m].status === 'error' ? 'alert' : 'check'" :size="14"
            :class="store.sessions[m].status === 'error' ? 'err' : 'ok'" />
      <span class="msg">{{ MODE_LABEL[m] }}測試{{ store.sessions[m].status === 'error' ? '執行失敗' : '完成' }}</span>
      <button type="button" class="btn btn-primary btn-sm" @click="view(m)">查看結果</button>
      <button type="button" class="close" aria-label="關閉提示" title="關閉提示" @click="dismiss(m)"><Icon name="x" :size="12" /></button>
    </div>
  </div>
</template>

<style scoped>
.toasts { position: fixed; right: 16px; bottom: 16px; z-index: 60; display: flex; flex-direction: column; gap: 8px; }
.toast {
  display: flex; align-items: center; gap: 10px;
  padding: 10px 12px; border-radius: 8px;
  background: var(--bg2); border: 1px solid var(--border3);
  box-shadow: 0 10px 28px rgba(0,0,0,0.35);
  font-size: 13px; color: var(--text0);
}
.ok  { color: var(--accent); }
.err { color: var(--danger); }
.close { border: none; background: none; color: var(--text1); cursor: pointer; display: inline-flex; padding: 4px; border-radius: var(--radius-sm); }
.close:hover { background: var(--bg3); color: var(--text0); }
</style>
