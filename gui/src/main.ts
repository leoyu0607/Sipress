import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'

// 本地打包字型（內網/離線環境也能正確顯示）
import '@fontsource/jetbrains-mono/latin-300.css'
import '@fontsource/jetbrains-mono/latin-400.css'
import '@fontsource/jetbrains-mono/latin-500.css'
import '@fontsource/jetbrains-mono/latin-700.css'
import '@fontsource/ibm-plex-sans/latin-300.css'
import '@fontsource/ibm-plex-sans/latin-400.css'
import '@fontsource/ibm-plex-sans/latin-500.css'
import '@fontsource/ibm-plex-sans/latin-600.css'

const app = createApp(App)
app.use(createPinia())
app.mount('#app')
