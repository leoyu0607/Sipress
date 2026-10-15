import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

// Node 25 起內建 Web Storage 預設開啟，全域的 localStorage 會蓋掉 happy-dom 的實作
// （沒給 --localstorage-file 時連 clear() 都沒有）。在測試 worker 關掉它；
// 舊版 Node 不認得這個旗標就不加，避免 "bad option"。
// @ts-expect-error process is a nodejs global
const webStorageFlag = process.allowedNodeEnvironmentFlags.has('--no-experimental-webstorage')
  ? ['--no-experimental-webstorage']
  : []

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: 'happy-dom',
    include: ['src/**/*.spec.ts'],
    execArgv: webStorageFlag,
  },
})
