<script setup lang="ts">
/**
 * RightPanel.vue
 *
 * 1. 通話統計：發起 / 接通 / 完成 / 失敗 / 逾時（來自即時 snapshot）
 * 2. RTP 聲音品質（MOS / 掉包率 / Jitter / 封包統計），codec 依設定顯示
 * 3. MOS 分級使用 store 的 mosGrade()（全 App 一致）
 * 4. 掉包率顏色：<1% 優 / <3% 普通 / ≥3% 差；Jitter：<30ms / <60ms / ≥60ms
 */
import { computed } from 'vue'
import { useTestStore, mosGrade } from '../stores/testStore'

const store = useTestStore()

const stateList = computed(() => [
  { key: 'initiated', label: '發起',  color: 'var(--blue)',   val: store.callStates.initiated },
  { key: 'answered',  label: '接通',  color: 'var(--accent)', val: store.callStates.answered },
  { key: 'completed', label: '完成',  color: 'var(--text1)',  val: store.callStates.completed },
  { key: 'failed',    label: '失敗',  color: 'var(--danger)', val: store.callStates.failed },
  { key: 'timeout',   label: '逾時',  color: 'var(--warn)',   val: store.callStates.timeout },
])

const stateMax = computed(() => Math.max(store.callStates.initiated, 1))

const respList = computed(() =>
  Object.entries(store.respCodes)
    .filter(([, v]) => v > 0)
    .sort(([a], [b]) => Number(a) - Number(b))
    .map(([code, cnt]) => ({
      code, cnt,
      color: code.startsWith('1') ? 'var(--text1)'
           : code.startsWith('2') ? 'var(--accent)'
           : 'var(--danger)',
    }))
)

// ── RTP 指標顏色 ─────────────────────────────────────────────────

const rtp = computed(() => store.rtpMetrics)

// RTP 是否「已設定啟用」（enableAudio=true 且有音檔）
const rtpConfigured = computed(() =>
  store.config.caller.enableAudio && !!store.config.caller.audioFile
)

const mos = computed(() => mosGrade(rtp.value.mos))

const lossColor = computed(() => {
  const p = rtp.value.packetLoss
  if (p < 1)  return 'var(--accent)'
  if (p < 3)  return 'var(--warn)'
  return 'var(--danger)'
})

const jitterColor = computed(() => {
  const j = rtp.value.jitter
  if (j < 30) return 'var(--accent)'
  if (j < 60) return 'var(--warn)'
  return 'var(--danger)'
})
</script>

<template>
  <div class="right-panel">

    <!-- ── 通話統計 ─────────────────────────────────── -->
    <div class="rp-section">
      <div class="rp-title">通話統計</div>
      <div v-for="s in stateList" :key="s.key" class="state-row">
        <div class="state-name">{{ s.label }}</div>
        <div class="state-track">
          <div class="state-fill"
               :style="{ width: Math.min(100, s.val / stateMax * 100) + '%', background: s.color }">
          </div>
        </div>
        <div class="state-count">{{ s.val.toLocaleString() }}</div>
      </div>
    </div>

    <!-- ── RTP 聲音品質 ──────────────────────────────────── -->
    <div class="rp-section">
      <div class="rp-title">
        RTP 聲音品質
        <span v-if="rtp.enabled" class="rtp-on">{{ store.codecLabel.short }}</span>
        <span v-else-if="rtpConfigured && store.isRunning" class="rtp-active">啟用中</span>
        <span v-else-if="!rtpConfigured" class="rtp-off">未啟用</span>
      </div>

      <template v-if="rtp.enabled">
        <!-- MOS -->
        <div class="rtp-row">
          <div class="rtp-key">MOS</div>
          <div class="rtp-bar-wrap">
            <div class="rtp-bar"
                 :style="{ width: Math.max(0, (rtp.mos - 1) / 4 * 100) + '%', background: mos.color }">
            </div>
          </div>
          <div class="rtp-val" :style="{ color: mos.color }">
            {{ rtp.mos.toFixed(2) }}
            <span class="rtp-tag">{{ mos.label }}</span>
          </div>
        </div>

        <!-- 掉包率 -->
        <div class="rtp-row">
          <div class="rtp-key">掉包</div>
          <div class="rtp-bar-wrap">
            <div class="rtp-bar"
                 :style="{ width: Math.min(rtp.packetLoss * 10, 100) + '%', background: lossColor }">
            </div>
          </div>
          <div class="rtp-val" :style="{ color: lossColor }">
            {{ rtp.packetLoss.toFixed(2) }}%
          </div>
        </div>

        <!-- Jitter -->
        <div class="rtp-row">
          <div class="rtp-key">Jitter</div>
          <div class="rtp-bar-wrap">
            <div class="rtp-bar"
                 :style="{ width: Math.min(rtp.jitter / 150 * 100, 100) + '%', background: jitterColor }">
            </div>
          </div>
          <div class="rtp-val" :style="{ color: jitterColor }">
            {{ rtp.jitter.toFixed(1) }}ms
          </div>
        </div>

        <!-- 封包統計 -->
        <div class="rtp-stats">
          <div class="rtp-stat-item">
            <span class="rtp-stat-key">送出</span>
            <span class="rtp-stat-val">{{ rtp.packetsSent.toLocaleString() }}</span>
          </div>
          <div class="rtp-stat-item">
            <span class="rtp-stat-key">接收</span>
            <span class="rtp-stat-val">{{ rtp.packetsRecv.toLocaleString() }}</span>
          </div>
          <div class="rtp-stat-item" title="亂序封包數（測試完成後統計）">
            <span class="rtp-stat-key">亂序</span>
            <span class="rtp-stat-val" :style="{ color: rtp.outOfOrder > 0 ? 'var(--warn)' : 'var(--text1)' }">
              {{ rtp.outOfOrder }}
            </span>
          </div>
        </div>
      </template>

      <div v-else-if="rtpConfigured && store.isRunning" class="rtp-hint">
        {{ store.codecLabel.full }} ← 等待通話建立…
      </div>

      <div v-else-if="rtpConfigured" class="rtp-hint">
        {{ store.codecLabel.full }} 就緒
      </div>

      <div v-else class="rtp-hint">
        在左側「民眾端設定」開啟<br>
        <code>播放音檔</code> 並選擇檔案即可量測
      </div>
    </div>

    <!-- ── Response Codes ──────────────────────────────── -->
    <div class="rp-section">
      <div class="rp-title">SIP 回應碼</div>
      <div v-if="respList.length === 0" class="empty">尚無資料</div>
      <div v-for="r in respList" :key="r.code" class="rcode-row">
        <span class="rcode-label" :style="{ color: r.color }">{{ r.code }}</span>
        <span class="rcode-count">{{ r.cnt.toLocaleString() }}</span>
      </div>
      <div v-if="store.isRunning && respList.length > 0" class="rp-note">失敗碼明細於測試結束後補齊</div>
    </div>

  </div>
</template>

<style scoped>
.right-panel {
  border-left: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  width: 210px;
  flex-shrink: 0;
}

.rp-section {
  padding: 10px 12px;
  border-bottom: 1px solid var(--border);
}

.rp-title {
  font-family: var(--mono);
  font-size: 10px;
  font-weight: 500;
  letter-spacing: 0.1em;
  color: var(--text2);
  text-transform: uppercase;
  margin-bottom: 8px;
  display: flex;
  align-items: center;
  gap: 6px;
}

.rtp-off {
  font-size: 10px;
  color: var(--text2);
  background: var(--bg3);
  border: 1px solid var(--border);
  padding: 1px 5px;
  border-radius: 3px;
  text-transform: lowercase;
}
.rtp-active {
  font-size: 10px;
  color: var(--warn);
  background: rgba(var(--warn-rgb),0.15);
  border: 1px solid var(--warn);
  padding: 1px 5px;
  border-radius: 3px;
}
.rtp-on {
  font-size: 10px;
  color: var(--accent);
  background: rgba(var(--accent-rgb),0.15);
  border: 1px solid var(--accent);
  padding: 1px 5px;
  border-radius: 3px;
}

/* ── State bars ─────────────────────────── */
.state-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 5px;
}
.state-name {
  font-size: 11px;
  color: var(--text1);
  min-width: 30px;
}
.state-track {
  flex: 1;
  height: 3px;
  background: var(--bg3);
  border-radius: 2px;
  overflow: hidden;
}
.state-fill {
  height: 100%;
  border-radius: 2px;
  transition: width 0.6s ease;
}
.state-count {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--text0);
  min-width: 44px;
  text-align: right;
}

/* ── RTP 品質 ───────────────────────────── */
.rtp-row {
  display: flex;
  align-items: center;
  gap: 5px;
  margin-bottom: 6px;
}
.rtp-key {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--text1);
  min-width: 36px;
}
.rtp-bar-wrap {
  flex: 1;
  height: 3px;
  background: var(--bg3);
  border-radius: 2px;
  overflow: hidden;
}
.rtp-bar {
  height: 100%;
  border-radius: 2px;
  transition: width 0.6s ease;
}
.rtp-val {
  font-family: var(--mono);
  font-size: 10px;
  min-width: 52px;
  text-align: right;
  display: flex;
  align-items: center;
  gap: 3px;
  justify-content: flex-end;
}
.rtp-tag {
  font-size: 10px;
  opacity: 0.75;
}

.rtp-stats {
  display: flex;
  gap: 8px;
  margin-top: 4px;
}
.rtp-stat-item {
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.rtp-stat-key {
  font-size: 10px;
  color: var(--text2);
}
.rtp-stat-val {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--text1);
}

.rtp-hint {
  font-size: 11px;
  color: var(--text2);
  line-height: 1.7;
}
.rtp-hint code {
  color: var(--accent);
  background: rgba(var(--accent-rgb),0.08);
  padding: 0 3px;
  border-radius: 2px;
}

/* ── Resp codes ─────────────────────────── */
.rcode-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 3px;
}
.rcode-label {
  font-family: var(--mono);
  font-size: 11px;
  font-weight: 500;
}
.rcode-count {
  font-family: var(--mono);
  font-size: 10px;
  color: var(--text1);
}
.empty {
  font-size: 11px;
  color: var(--text2);
}

.rp-note {
  font-size: 10px;
  color: var(--text2);
  margin-top: 4px;
}
</style>
