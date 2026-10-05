<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { enable, isEnabled, disable } from '@tauri-apps/plugin-autostart'
import { 
  Play, 
  Square, 
  Copy, 
  Check, 
  Settings, 
  ShieldCheck, 
  Activity, 
  ExternalLink,
  Minus,
  Maximize2,
  X,
  RefreshCw,
  ArrowUp,
  ArrowDown,
  ChevronUp,
  ChevronDown,
  Globe,
  Wifi,
  Download,
  AlertCircle
} from 'lucide-vue-next'

interface DcTestResult {
  dc: string
  host: string
  ok: boolean
  latency_ms: number
  message: string
}

interface UpdateCheckResult {
  has_update: boolean
  current_version: string
  latest_version: string
  release_url: string
}

interface ProxyConfig {
  host: string
  port: number
  secret: string
  dc_redirects: Record<number, string>
  buffer_size: number
  pool_size: number
  fallback_cfproxy: boolean
  cfproxy_user_domain_enabled: boolean
  cfproxy_user_domains: string[]
  cfproxy_worker_enabled: boolean
  cfproxy_worker_domains: string[]
  cfproxy_h2_media: boolean
  disable_secure: boolean
  fake_tls_domain: string
  proxy_protocol: boolean
  force_test_dc: boolean
  verbose: boolean
  log_max_mb: number
}

interface TelemetrySnapshot {
  connections_total: number
  connections_active: number
  connections_ws: number
  connections_cfproxy: number
  connections_tcp: number
  bytes_up: number
  bytes_down: number
  speed_up_kbps: number
  speed_down_kbps: number
}

const isRunning = ref(false)
const tgLink = ref('')
const copied = ref(false)
const toast = ref<{ show: boolean; message: string }>({ show: false, message: '' })
const activeTab = ref<'main' | 'settings'>('main')
const autostartActive = ref(false)

function showToast(message: string) {
  toast.value = { show: true, message }
  setTimeout(() => {
    toast.value.show = false
  }, 2600)
}

const userDomainsInput = ref('')
const workerDomainsInput = ref('')
const dcRedirectsInput = ref('2: 149.154.167.220, 4: 149.154.167.220')

const isTestingConnectivity = ref(false)
const testResults = ref<DcTestResult[]>([])
const showTestModal = ref(false)

const updateInfo = ref<UpdateCheckResult | null>(null)
const isCheckingUpdate = ref(false)

async function checkForUpdates(manual = false) {
  isCheckingUpdate.value = true
  try {
    const res = await invoke<UpdateCheckResult>('check_updates')
    updateInfo.value = res
    if (manual) {
      if (res.has_update) {
        showToast(`Доступна новая версия: ${res.latest_version}!`)
      } else {
        showToast('У вас установлена самая актуальная версия!')
      }
    }
  } catch (e) {
    console.error('Update check error:', e)
    if (manual) showToast('Ошибка при проверке обновлений')
  } finally {
    isCheckingUpdate.value = false
  }
}

async function runConnectivityTest() {
  isTestingConnectivity.value = true
  showTestModal.value = true
  try {
    const custom = userDomainsInput.value
      .split(/[\s,;]+/)
      .map(s => s.trim())
      .filter(Boolean)
    testResults.value = await invoke<DcTestResult[]>('test_connectivity', { customDomains: custom })
  } catch (e) {
    console.error('Connectivity test error:', e)
  } finally {
    isTestingConnectivity.value = false
  }
}

function parseDcRedirects(text: string): Record<number, string> {
  const result: Record<number, string> = {}
  const pairs = text.split(/[\s,;]+/).map(s => s.trim()).filter(Boolean)
  for (const pair of pairs) {
    const [dcStr, ip] = pair.split(':')
    if (dcStr && ip) {
      const dcNum = parseInt(dcStr.trim(), 10)
      if (!isNaN(dcNum)) {
        result[dcNum] = ip.trim()
      }
    }
  }
  return result
}

const telemetry = ref<TelemetrySnapshot>({
  connections_total: 0,
  connections_active: 0,
  connections_ws: 0,
  connections_cfproxy: 0,
  connections_tcp: 0,
  bytes_up: 0,
  bytes_down: 0,
  speed_up_kbps: 0,
  speed_down_kbps: 0,
})

const config = ref<ProxyConfig>({
  host: '127.0.0.1',
  port: 1443,
  secret: '',
  dc_redirects: { 2: '149.154.167.220', 4: '149.154.167.220' },
  buffer_size: 262144,
  pool_size: 4,
  fallback_cfproxy: true,
  cfproxy_user_domain_enabled: false,
  cfproxy_user_domains: [],
  cfproxy_worker_enabled: false,
  cfproxy_worker_domains: [],
  cfproxy_h2_media: true,
  disable_secure: false,
  fake_tls_domain: '',
  proxy_protocol: false,
  force_test_dc: false,
  verbose: false,
  log_max_mb: 20,
})

function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i]
}

function generateSecret() {
  const arr = new Uint8Array(16)
  window.crypto.getRandomValues(arr)
  config.value.secret = Array.from(arr).map(b => b.toString(16).padStart(2, '0')).join('')
  updateTgLink(config.value)
}

function updateTgLink(cfg: ProxyConfig) {
  const host = cfg.host === '0.0.0.0' ? '127.0.0.1' : cfg.host
  let cleanSecret = cfg.secret.trim()
  // Only slice 'dd' or 'ee' prefix if length is greater than 32 (meaning prefix was already prepended)
  if ((cleanSecret.startsWith('dd') || cleanSecret.startsWith('ee')) && cleanSecret.length > 32) {
    cleanSecret = cleanSecret.slice(2)
  }

  let formattedSecret = ''
  if (cfg.fake_tls_domain && cfg.fake_tls_domain.trim()) {
    const sniHex = Array.from(new TextEncoder().encode(cfg.fake_tls_domain.trim()))
      .map(b => b.toString(16).padStart(2, '0'))
      .join('')
    formattedSecret = `ee${cleanSecret}${sniHex}`
  } else {
    formattedSecret = `dd${cleanSecret}`
  }

  tgLink.value = `tg://proxy?server=${host}&port=${cfg.port}&secret=${formattedSecret}`
}

async function fetchStatus() {
  try {
    isRunning.value = await invoke<boolean>('is_running')
    const cfg = await invoke<ProxyConfig>('get_config')
    config.value = cfg
    userDomainsInput.value = (cfg.cfproxy_user_domains || []).join(', ')
    workerDomainsInput.value = (cfg.cfproxy_worker_domains || []).join(', ')
    dcRedirectsInput.value = Object.entries(cfg.dc_redirects || {})
      .map(([dc, ip]) => `${dc}: ${ip}`)
      .join(', ')

    updateTgLink(cfg)
    autostartActive.value = await isEnabled()
  } catch (err) {
    console.error('Failed to load status:', err)
  }
}

async function toggleAutostart() {
  try {
    if (autostartActive.value) {
      await disable()
      autostartActive.value = false
    } else {
      await enable()
      autostartActive.value = true
    }
  } catch (err) {
    console.error('Error toggling autostart:', err)
  }
}

async function toggleProxy() {
  if (isRunning.value) {
    try {
      await invoke('stop_proxy')
      isRunning.value = false
      telemetry.value.connections_active = 0
      telemetry.value.speed_up_kbps = 0
      telemetry.value.speed_down_kbps = 0
    } catch (err) {
      console.error('Error stopping proxy:', err)
    }
    return
  }

  try {
    await invoke<string>('start_proxy')
    updateTgLink(config.value)
    isRunning.value = true
  } catch (err) {
    console.error('Error starting proxy:', err)
  }
}

async function saveSettings() {
  try {
    config.value.cfproxy_user_domains = userDomainsInput.value
      .split(/[\s,;]+/)
      .map(s => s.trim())
      .filter(Boolean)

    config.value.cfproxy_worker_domains = workerDomainsInput.value
      .split(/[\s,;]+/)
      .map(s => s.trim())
      .filter(Boolean)

    config.value.dc_redirects = parseDcRedirects(dcRedirectsInput.value)

    await invoke('save_config', { newConfig: config.value })
    updateTgLink(config.value)
    showToast('Настройки успешно применены и сохранены!')
  } catch (err) {
    console.error('Error saving settings:', err)
  }
}

async function copyLink() {
  if (!tgLink.value) return
  await navigator.clipboard.writeText(tgLink.value)
  copied.value = true
  setTimeout(() => {
    copied.value = false
  }, 2000)
}

async function openTelegram() {
  if (!tgLink.value) return
  try {
    await invoke('open_url', { url: tgLink.value })
  } catch (err) {
    console.error('Failed to open tg link via backend:', err)
    window.open(tgLink.value.replace('tg://proxy?', 'https://t.me/proxy?'), '_blank')
  }
}

async function minimizeApp() {
  await invoke('minimize_window')
}

async function toggleMaximizeApp() {
  await invoke('toggle_maximize_window')
}

async function closeApp() {
  await invoke('close_window')
}

onMounted(() => {
  fetchStatus()
  checkForUpdates(false)
  listen<TelemetrySnapshot>('telemetry-update', (event) => {
    if (!isRunning.value) {
      event.payload.connections_active = 0
      event.payload.speed_up_kbps = 0
      event.payload.speed_down_kbps = 0
    }
    telemetry.value = event.payload
  })
})
</script>

<template>
  <div class="flex flex-col h-screen w-screen bg-[#111317] text-[#e1e4ea] font-sans overflow-hidden select-none border border-white/5 rounded-xl shadow-2xl">
    <!-- Custom Draggable Header -->
    <header data-tauri-drag-region class="h-9 bg-[#111317] flex items-center justify-between px-3 shrink-0 cursor-move">
      <div data-tauri-drag-region class="flex items-center gap-2 pointer-events-none">
        <span class="text-[11px] font-semibold text-white/40 tracking-wider font-mono">TG-WS-PROXY</span>
      </div>
      <div class="flex items-center gap-1">
        <button 
          @click="minimizeApp"
          class="w-7 h-6 flex items-center justify-center rounded hover:bg-white/10 text-white/50 hover:text-white transition"
          title="Свернуть"
        >
          <Minus class="w-3.5 h-3.5" />
        </button>
        <button 
          @click="toggleMaximizeApp"
          class="w-7 h-6 flex items-center justify-center rounded hover:bg-white/10 text-white/50 hover:text-white transition"
          title="Развернуть"
        >
          <Maximize2 class="w-3 h-3" />
        </button>
        <button 
          @click="closeApp"
          class="w-7 h-6 flex items-center justify-center rounded hover:bg-rose-500/20 hover:text-rose-300 text-white/50 transition"
          title="В трей"
        >
          <X class="w-3.5 h-3.5" />
        </button>
      </div>
    </header>

    <!-- Main Layout (Sidebar + Content) -->
    <div class="flex-1 flex overflow-hidden p-3 gap-3 pt-0">
      <!-- Left Sidebar -->
      <aside class="w-60 bg-[#16191f] rounded-2xl p-4 flex flex-col justify-between border border-white/5 shadow-inner shrink-0">
        <div class="space-y-6">
          <!-- Logo / App Name -->
          <div class="flex items-center gap-3">
            <div class="w-9 h-9 rounded-xl bg-[#202530] border border-white/5 flex items-center justify-center shadow-md">
              <ShieldCheck class="w-5 h-5 text-white/90" />
            </div>
            <h1 class="font-bold text-sm text-white tracking-wide">TG WS Proxy</h1>
          </div>

          <!-- Navigation Pills -->
          <nav class="space-y-1.5">
            <button 
              @click="activeTab = 'main'"
              :class="[
                'w-full flex items-center gap-3 px-3.5 py-2.5 rounded-xl text-xs font-medium transition duration-200',
                activeTab === 'main' 
                  ? 'bg-gradient-to-r from-[#2a303c] to-[#202530] text-white shadow-lg border border-white/10' 
                  : 'text-white/50 hover:text-white/80 hover:bg-white/5'
              ]"
            >
              <Activity class="w-4 h-4" />
              <span>Управление</span>
            </button>

            <button 
              @click="activeTab = 'settings'"
              :class="[
                'w-full flex items-center gap-3 px-3.5 py-2.5 rounded-xl text-xs font-medium transition duration-200',
                activeTab === 'settings' 
                  ? 'bg-gradient-to-r from-[#2a303c] to-[#202530] text-white shadow-lg border border-white/10' 
                  : 'text-white/50 hover:text-white/80 hover:bg-white/5'
              ]"
            >
              <Settings class="w-4 h-4" />
              <span>Настройки</span>
            </button>

            <button 
              @click="runConnectivityTest"
              :disabled="isTestingConnectivity"
              class="w-full flex items-center gap-3 px-3.5 py-2.5 rounded-xl text-xs font-medium text-white/60 hover:text-white hover:bg-white/5 transition border border-white/[0.04] cursor-pointer disabled:opacity-50 mt-2"
              title="Проверить пинг и доступность DC1–DC5"
            >
              <Wifi class="w-4 h-4 text-emerald-400" :class="isTestingConnectivity ? 'animate-pulse' : ''" />
              <span>{{ isTestingConnectivity ? 'Проверка...' : 'Тест связности DC' }}</span>
            </button>
          </nav>
        </div>

        <!-- Sidebar Bottom Status Indicator -->
        <div class="flex items-center gap-3 p-1">
          <div class="w-2.5 h-2.5 rounded-full bg-emerald-500 shadow-[0_0_10px_rgba(16,185,129,0.8)] animate-pulse" />
          <div>
            <div class="text-xs font-semibold text-white/90">
              Мост активен
            </div>
            <div class="text-[11px] font-mono text-white/40">
              {{ config.host }}:{{ config.port }}
            </div>
          </div>
        </div>
      </aside>

      <!-- Right Main Content Area -->
      <main class="flex-1 bg-[#16191f] rounded-2xl p-5 border border-white/5 shadow-inner overflow-y-auto">
        <!-- New Version Update Banner -->
        <div 
          v-if="updateInfo && updateInfo.has_update" 
          class="mb-4 bg-gradient-to-r from-blue-900/40 via-indigo-900/40 to-purple-900/40 border border-blue-500/30 rounded-2xl p-3.5 flex items-center justify-between shadow-lg shadow-blue-950/30 animate-pulse"
        >
          <div class="flex items-center gap-3">
            <div class="w-8 h-8 rounded-xl bg-blue-500/20 border border-blue-500/30 flex items-center justify-center text-blue-400">
              <Download class="w-4 h-4" />
            </div>
            <div>
              <div class="text-xs font-bold text-white">Доступна новая версия {{ updateInfo.latest_version }}!</div>
              <div class="text-[10px] text-white/50">Текущая версия {{ updateInfo.current_version }}</div>
            </div>
          </div>
          <button 
            @click="invoke('open_url', { url: updateInfo.release_url })"
            class="px-3.5 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-white text-xs font-semibold shadow-md transition cursor-pointer"
          >
            Скачать
          </button>
        </div>

        <!-- VIEW 1: УПРАВЛЕНИЕ -->
        <div v-if="activeTab === 'main'" class="space-y-4 max-w-2xl mx-auto">
          <!-- Status Banner Card -->
          <div class="bg-[#1c2027] border border-white/5 rounded-2xl p-5 shadow-lg flex items-center justify-between">
            <div class="space-y-1">
              <span class="text-[10px] font-bold uppercase tracking-wider text-emerald-400 flex items-center gap-1.5">
                <span class="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse"></span>
                СЕРВИС ВСЕГДА ВКЛЮЧЕН
              </span>
              <h2 class="text-xl font-bold text-white tracking-tight">
                Локальный мост активен
              </h2>
              <p class="text-xs text-white/50 max-w-sm leading-relaxed">
                MTProto WebSocket мост работает в фоновом режиме на {{ config.host }}:{{ config.port }} и маршрутизирует трафик в Telegram.
              </p>
            </div>

            <!-- Always Active Indicator Pill -->
            <div class="flex items-center gap-2.5 px-4 py-2.5 rounded-xl bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 text-xs font-semibold shadow-md shadow-emerald-500/10">
              <span class="w-2.5 h-2.5 rounded-full bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.8)] animate-pulse"></span>
              <span>Мост работает</span>
            </div>
          </div>

          <!-- 4 Telemetry Metrics Grid -->
          <div class="grid grid-cols-4 gap-3">
            <!-- Metric 1: Upload -->
            <div class="bg-gradient-to-b from-[#1c212b] to-[#161922] border border-white/[0.06] hover:border-white/10 transition duration-200 rounded-2xl p-4 space-y-2 shadow-lg shadow-black/20">
              <div class="flex items-center justify-between text-[11px] text-white/50 font-medium">
                <span>Отдача</span>
                <div class="w-6 h-6 rounded-lg bg-blue-500/10 border border-blue-500/20 flex items-center justify-center text-blue-400">
                  <ArrowUp class="w-3.5 h-3.5" />
                </div>
              </div>
              <div class="text-lg font-bold text-white font-mono tracking-tight">
                {{ telemetry.speed_up_kbps.toFixed(1) }} <span class="text-xs font-normal text-white/40">Кб/с</span>
              </div>
              <div class="text-[10px] text-white/40 font-mono">
                Всего: {{ formatBytes(telemetry.bytes_up) }}
              </div>
            </div>

            <!-- Metric 2: Download -->
            <div class="bg-gradient-to-b from-[#1c212b] to-[#161922] border border-white/[0.06] hover:border-white/10 transition duration-200 rounded-2xl p-4 space-y-2 shadow-lg shadow-black/20">
              <div class="flex items-center justify-between text-[11px] text-white/50 font-medium">
                <span>Загрузка</span>
                <div class="w-6 h-6 rounded-lg bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center text-emerald-400">
                  <ArrowDown class="w-3.5 h-3.5" />
                </div>
              </div>
              <div class="text-lg font-bold text-emerald-400 font-mono tracking-tight">
                {{ telemetry.speed_down_kbps.toFixed(1) }} <span class="text-xs font-normal text-white/40">Кб/с</span>
              </div>
              <div class="text-[10px] text-white/40 font-mono">
                Всего: {{ formatBytes(telemetry.bytes_down) }}
              </div>
            </div>

            <!-- Metric 3: Active Sockets -->
            <div class="bg-gradient-to-b from-[#1c212b] to-[#161922] border border-white/[0.06] hover:border-white/10 transition duration-200 rounded-2xl p-4 space-y-2 shadow-lg shadow-black/20">
              <div class="flex items-center justify-between text-[11px] text-white/50 font-medium">
                <span>Активные сокеты</span>
                <div class="w-6 h-6 rounded-lg bg-amber-500/10 border border-amber-500/20 flex items-center justify-center text-amber-400">
                  <Activity class="w-3.5 h-3.5" />
                </div>
              </div>
              <div class="text-lg font-bold text-white font-mono tracking-tight">
                {{ telemetry.connections_active }}
              </div>
              <div class="text-[10px] text-white/40">
                Всего сессий: {{ telemetry.connections_total }}
              </div>
            </div>

            <!-- Metric 4: Transport -->
            <div class="bg-gradient-to-b from-[#1c212b] to-[#161922] border border-white/[0.06] hover:border-white/10 transition duration-200 rounded-2xl p-4 space-y-2 shadow-lg shadow-black/20">
              <div class="flex items-center justify-between text-[11px] text-white/50 font-medium">
                <span>Транспорт</span>
                <div class="w-6 h-6 rounded-lg bg-purple-500/10 border border-purple-500/20 flex items-center justify-center text-purple-400 font-bold text-[10px] font-mono">
                  CF
                </div>
              </div>
              <div class="text-base font-bold text-purple-300 font-mono tracking-tight">
                WS + H2
              </div>
              <div class="text-[10px] text-white/40 truncate">
                Cloudflare CDN
              </div>
            </div>
          </div>

          <!-- Telegram Connection Link Card -->
          <div class="bg-gradient-to-b from-[#1b2230] via-[#161b25] to-[#13161f] border border-blue-500/25 rounded-2xl p-5 space-y-4 shadow-xl shadow-blue-950/20">
            <div class="flex items-start justify-between">
              <div>
                <h3 class="text-sm font-bold text-white flex items-center gap-2">
                  <span>Ссылка для подключения Telegram</span>
                  <span class="px-2 py-0.5 rounded-full text-[10px] font-medium bg-blue-500/20 text-blue-400 border border-blue-500/30 font-mono">MTProto</span>
                </h3>
                <p class="text-xs text-white/50 mt-1">
                  Нажмите «Открыть в Telegram» для мгновенного добавления прокси в клиент или скопируйте ссылку.
                </p>
              </div>
            </div>

            <!-- Link Bar with Buttons -->
            <div class="flex items-center gap-2 bg-[#0e1117] p-2 rounded-xl border border-white/5 shadow-inner">
              <input 
                readonly
                :value="tgLink"
                class="bg-transparent flex-1 text-xs font-mono text-white/80 outline-none px-3 select-all truncate selection:bg-blue-600/40"
              />
              
              <!-- Copy button -->
              <button 
                @click="copyLink"
                :class="[
                  'flex items-center gap-1.5 px-3 py-2 rounded-lg text-xs font-medium transition duration-200 active:scale-95 cursor-pointer',
                  copied 
                    ? 'bg-emerald-500/20 text-emerald-300 border border-emerald-500/30' 
                    : 'bg-[#1c222e] hover:bg-[#252c3c] text-white/70 hover:text-white border border-white/5'
                ]"
                title="Скопировать ссылку"
              >
                <component :is="copied ? Check : Copy" class="w-3.5 h-3.5" :class="copied ? 'text-emerald-400 stroke-[3]' : ''" />
                <span>{{ copied ? 'Скопировано!' : 'Копировать' }}</span>
              </button>

              <!-- Open in Telegram button -->
              <button 
                @click="openTelegram"
                class="flex items-center gap-2 px-4 py-2 rounded-lg bg-gradient-to-r from-blue-600 via-blue-500 to-indigo-600 hover:from-blue-500 hover:to-indigo-500 text-white text-xs font-semibold shadow-lg shadow-blue-600/30 hover:shadow-blue-500/40 active:scale-95 transition-all duration-200 cursor-pointer"
              >
                <ExternalLink class="w-3.5 h-3.5 text-white" />
                <span>Открыть в Telegram</span>
              </button>
            </div>
          </div>
        </div>

        <!-- VIEW 2: ПОЛНЫЕ НАСТРОЙКИ (С красивыми кастомными чекбоксами) -->
        <div v-else class="space-y-4 max-w-2xl mx-auto pb-4">
          <!-- 1. MTProto Connection Settings -->
          <div class="bg-[#1c2027] border border-white/5 rounded-2xl p-4 space-y-3 shadow-md">
            <div class="text-[11px] font-bold text-white/60 uppercase tracking-wider">Подключение (MTProto)</div>
            
            <div class="grid grid-cols-3 gap-3">
              <div class="col-span-2">
                <label class="block text-[11px] text-white/40 mb-1">Хост</label>
                <input 
                  v-model="config.host" 
                  class="w-full bg-[#12151a] border border-white/5 rounded-xl px-3 py-1.5 text-xs text-white outline-none focus:border-blue-500/50"
                />
              </div>
              <div>
                <label class="block text-[11px] text-white/40 mb-1">Порт</label>
                <div class="relative flex items-center bg-[#12151a] border border-white/5 focus-within:border-blue-500/50 rounded-xl px-2.5 py-1 transition group">
                  <input 
                    type="number" 
                    v-model.number="config.port" 
                    class="w-full bg-transparent text-xs font-mono text-white outline-none"
                  />
                  <div class="flex flex-col -mr-1">
                    <button 
                      type="button" 
                      @click="config.port = Math.min(65535, (config.port || 0) + 1)"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Увеличить порт"
                    >
                      <ChevronUp class="w-3 h-3" />
                    </button>
                    <button 
                      type="button" 
                      @click="config.port = Math.max(1, (config.port || 0) - 1)"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Уменьшить порт"
                    >
                      <ChevronDown class="w-3 h-3" />
                    </button>
                  </div>
                </div>
              </div>
            </div>

            <div>
              <label class="block text-[11px] text-white/40 mb-1">Секрет (16 байт в hex)</label>
              <div class="flex gap-2">
                <input 
                  v-model="config.secret" 
                  class="flex-1 bg-[#12151a] border border-white/5 rounded-xl px-3 py-1.5 text-xs font-mono text-white outline-none focus:border-blue-500/50"
                />
                <button 
                  @click="generateSecret"
                  class="px-3 py-1.5 rounded-xl bg-[#202530] hover:bg-[#2a3040] text-white/80 text-xs flex items-center gap-1.5 transition border border-white/5"
                  title="Сгенерировать случайный секрет"
                >
                  <RefreshCw class="w-3 h-3" />
                  <span>Случайный</span>
                </button>
              </div>
            </div>

            <div>
              <label class="block text-[11px] text-white/40 mb-1">Целевые IP дата-центров (DC:IP перенаправление)</label>
              <input 
                v-model="dcRedirectsInput" 
                placeholder="2: 149.154.167.220, 4: 149.154.167.220"
                class="w-full bg-[#12151a] border border-white/5 rounded-xl px-3 py-1.5 text-xs font-mono text-white outline-none focus:border-blue-500/50"
              />
              <div class="text-[10px] text-white/30 mt-1">Прямые IP адреса для маршрутизации трафика к серверам Telegram CDN</div>
            </div>
          </div>

          <!-- 2. Cloudflare Proxy & Workers -->
          <div class="bg-[#1c2027] border border-white/5 rounded-2xl p-4 space-y-3 shadow-md">
            <div class="text-[11px] font-bold text-white/60 uppercase tracking-wider">Cloudflare Прокси и Воркеры</div>
            
            <div class="space-y-2.5">
              <!-- Toggle 1: Cloudflare Fallback -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">Cloudflare Fallback</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Обход блокировок через сеть Cloudflare при сбоях прямого соединения</div>
                </div>
                <button
                  type="button"
                  @click="config.fallback_cfproxy = !config.fallback_cfproxy"
                  :class="[
                    'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                    config.fallback_cfproxy ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                  ]"
                >
                  <div
                    :class="[
                      'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                      config.fallback_cfproxy ? 'translate-x-4' : 'translate-x-0'
                    ]"
                  />
                </button>
              </div>

              <!-- Toggle 2: H2 Media Multiplexing -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">HTTP/2 мультиплексирование</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Ускорение загрузки медиа, картинок, видео и файлов в Telegram</div>
                </div>
                <button
                  type="button"
                  @click="config.cfproxy_h2_media = !config.cfproxy_h2_media"
                  :class="[
                    'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                    config.cfproxy_h2_media ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                  ]"
                >
                  <div
                    :class="[
                      'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                      config.cfproxy_h2_media ? 'translate-x-4' : 'translate-x-0'
                    ]"
                  />
                </button>
              </div>

              <!-- Toggle 3: Custom Domains -->
              <div class="p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition space-y-2.5">
                <div class="flex items-center justify-between">
                  <div class="pr-3">
                    <div class="text-xs font-semibold text-white/90">Собственные Cloudflare домены</div>
                    <div class="text-[10px] text-white/40 mt-0.5">Маршрутизация трафика через пользовательские домены</div>
                  </div>
                  <button
                    type="button"
                    @click="config.cfproxy_user_domain_enabled = !config.cfproxy_user_domain_enabled"
                    :class="[
                      'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                      config.cfproxy_user_domain_enabled ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                    ]"
                  >
                    <div
                      :class="[
                        'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                        config.cfproxy_user_domain_enabled ? 'translate-x-4' : 'translate-x-0'
                      ]"
                    />
                  </button>
                </div>
                <input 
                  v-if="config.cfproxy_user_domain_enabled"
                  v-model="userDomainsInput"
                  placeholder="proxy.example.com, cdn.mysite.org"
                  class="w-full bg-[#12151a] border border-white/10 rounded-xl px-3 py-1.5 text-xs text-white outline-none focus:border-blue-500/50 mt-1"
                />
              </div>

              <!-- Toggle 4: Worker Domains -->
              <div class="p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition space-y-2.5">
                <div class="flex items-center justify-between">
                  <div class="pr-3">
                    <div class="text-xs font-semibold text-white/90">Cloudflare Worker воркеры</div>
                    <div class="text-[10px] text-white/40 mt-0.5">Использование бессерверных воркеров Cloudflare Workers</div>
                  </div>
                  <button
                    type="button"
                    @click="config.cfproxy_worker_enabled = !config.cfproxy_worker_enabled"
                    :class="[
                      'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                      config.cfproxy_worker_enabled ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                    ]"
                  >
                    <div
                      :class="[
                        'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                        config.cfproxy_worker_enabled ? 'translate-x-4' : 'translate-x-0'
                      ]"
                    />
                  </button>
                </div>
                <input 
                  v-if="config.cfproxy_worker_enabled"
                  v-model="workerDomainsInput"
                  placeholder="my-worker.subdomain.workers.dev"
                  class="w-full bg-[#12151a] border border-white/10 rounded-xl px-3 py-1.5 text-xs text-white outline-none focus:border-blue-500/50 mt-1"
                />
              </div>
            </div>
          </div>

          <!-- 3. Masking & Fake TLS -->
          <div class="bg-[#1c2027] border border-white/5 rounded-2xl p-4 space-y-3 shadow-md">
            <div class="text-[11px] font-bold text-white/60 uppercase tracking-wider">Маскировка (Fake TLS)</div>

            <div>
              <label class="block text-[11px] text-white/40 mb-1">SNI маскирующий домен (DPI bypass)</label>
              <input 
                v-model="config.fake_tls_domain" 
                placeholder="например, cloudflare.com, yandex.ru"
                class="w-full bg-[#12151a] border border-white/5 rounded-xl px-3 py-1.5 text-xs font-mono text-white outline-none focus:border-blue-500/50"
              />
            </div>

            <!-- Toggle 5: PROXY Protocol v1 -->
            <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
              <div class="pr-3">
                <div class="text-xs font-semibold text-white/90">PROXY Protocol v1</div>
                <div class="text-[10px] text-white/40 mt-0.5">Передача заголовка с реальным адресом клиента</div>
              </div>
              <button
                type="button"
                @click="config.proxy_protocol = !config.proxy_protocol"
                :class="[
                  'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                  config.proxy_protocol ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                ]"
              >
                <div
                  :class="[
                    'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                    config.proxy_protocol ? 'translate-x-4' : 'translate-x-0'
                  ]"
                />
              </button>
            </div>
          </div>

          <!-- 4. Advanced Connection Parameters & Autostart -->
          <div class="bg-[#1c2027] border border-white/5 rounded-2xl p-4 space-y-3 shadow-md">
            <div class="text-[11px] font-bold text-white/60 uppercase tracking-wider">Система и оптимизация</div>

            <div class="grid grid-cols-3 gap-3">
              <div>
                <label class="block text-[11px] text-white/40 mb-1">Буфер сокета (байт)</label>
                <div class="relative flex items-center bg-[#12151a] border border-white/5 focus-within:border-blue-500/50 rounded-xl px-2.5 py-1 transition group">
                  <input 
                    type="number" 
                    step="65536"
                    v-model.number="config.buffer_size" 
                    class="w-full bg-transparent text-xs font-mono text-white outline-none"
                  />
                  <div class="flex flex-col -mr-1">
                    <button 
                      type="button" 
                      @click="config.buffer_size = (config.buffer_size || 0) + 65536"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Увеличить буфер"
                    >
                      <ChevronUp class="w-3 h-3" />
                    </button>
                    <button 
                      type="button" 
                      @click="config.buffer_size = Math.max(4096, (config.buffer_size || 0) - 65536)"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Уменьшить буфер"
                    >
                      <ChevronDown class="w-3 h-3" />
                    </button>
                  </div>
                </div>
              </div>

              <div>
                <label class="block text-[11px] text-white/40 mb-1">Размер пула сокетов</label>
                <div class="relative flex items-center bg-[#12151a] border border-white/5 focus-within:border-blue-500/50 rounded-xl px-2.5 py-1 transition group">
                  <input 
                    type="number" 
                    v-model.number="config.pool_size" 
                    class="w-full bg-transparent text-xs font-mono text-white outline-none"
                  />
                  <div class="flex flex-col -mr-1">
                    <button 
                      type="button" 
                      @click="config.pool_size = Math.min(64, (config.pool_size || 0) + 1)"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Увеличить размер пула"
                    >
                      <ChevronUp class="w-3 h-3" />
                    </button>
                    <button 
                      type="button" 
                      @click="config.pool_size = Math.max(0, (config.pool_size || 0) - 1)"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Уменьшить размер пула"
                    >
                      <ChevronDown class="w-3 h-3" />
                    </button>
                  </div>
                </div>
              </div>

              <div>
                <label class="block text-[11px] text-white/40 mb-1">Лог файл (МБ)</label>
                <div class="relative flex items-center bg-[#12151a] border border-white/5 focus-within:border-blue-500/50 rounded-xl px-2.5 py-1 transition group">
                  <input 
                    type="number" 
                    v-model.number="config.log_max_mb" 
                    class="w-full bg-transparent text-xs font-mono text-white outline-none"
                  />
                  <div class="flex flex-col -mr-1">
                    <button 
                      type="button" 
                      @click="config.log_max_mb = (config.log_max_mb || 0) + 1"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Увеличить размер логов"
                    >
                      <ChevronUp class="w-3 h-3" />
                    </button>
                    <button 
                      type="button" 
                      @click="config.log_max_mb = Math.max(1, (config.log_max_mb || 0) - 1)"
                      class="p-0.5 text-white/40 hover:text-white hover:bg-white/10 rounded transition cursor-pointer active:scale-90"
                      title="Уменьшить размер логов"
                    >
                      <ChevronDown class="w-3 h-3" />
                    </button>
                  </div>
                </div>
              </div>
            </div>

            <div class="space-y-2.5 pt-1">
              <!-- Toggle 6: No Secure -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">Режим No-Secure</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Порт 80 без SSL шифрования для CF proxy/worker</div>
                </div>
                <button
                  type="button"
                  @click="config.disable_secure = !config.disable_secure"
                  :class="[
                    'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                    config.disable_secure ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                  ]"
                >
                  <div
                    :class="[
                      'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                      config.disable_secure ? 'translate-x-4' : 'translate-x-0'
                    ]"
                  />
                </button>
              </div>

              <!-- Toggle 7: Force Test DC -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">Тестовые дата-центры Telegram</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Использовать Test DC для тестирования и разработки</div>
                </div>
                <button
                  type="button"
                  @click="config.force_test_dc = !config.force_test_dc"
                  :class="[
                    'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                    config.force_test_dc ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                  ]"
                >
                  <div
                    :class="[
                      'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                      config.force_test_dc ? 'translate-x-4' : 'translate-x-0'
                    ]"
                  />
                </button>
              </div>

              <!-- Toggle 8: Verbose Logging -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">Подробные логи (Verbose)</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Вывод расширенной отладочной информации в журнал</div>
                </div>
                <button
                  type="button"
                  @click="config.verbose = !config.verbose"
                  :class="[
                    'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                    config.verbose ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                  ]"
                >
                  <div
                    :class="[
                      'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                      config.verbose ? 'translate-x-4' : 'translate-x-0'
                    ]"
                  />
                </button>
              </div>

              <!-- Toggle 9: Autostart switch -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">Автозапуск с Windows</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Запускать мост в фоновом режиме при старте системы</div>
                </div>
                <button
                  type="button"
                  @click="toggleAutostart"
                  :class="[
                    'w-10 h-6 flex items-center rounded-full p-0.5 transition-all duration-200 shrink-0 cursor-pointer',
                    autostartActive ? 'bg-blue-600 shadow-sm shadow-blue-500/40 ring-1 ring-blue-400/30' : 'bg-[#202530] border border-white/10 hover:border-white/20'
                  ]"
                >
                  <div
                    :class="[
                      'bg-white w-5 h-5 rounded-full shadow-md transform transition-all duration-200',
                      autostartActive ? 'translate-x-4' : 'translate-x-0'
                    ]"
                  />
                </button>
              </div>

              <!-- Action 1: DC Connectivity Test -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">Тест связности дата-центров</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Проверить отклик и доступность серверов Telegram DC1–DC5</div>
                </div>
                <button
                  type="button"
                  @click="runConnectivityTest"
                  :disabled="isTestingConnectivity"
                  class="px-3 py-1.5 rounded-xl bg-[#202530] hover:bg-[#2a3040] text-xs font-medium text-white/90 flex items-center gap-1.5 transition border border-white/5 cursor-pointer disabled:opacity-50"
                >
                  <Wifi class="w-3.5 h-3.5 text-emerald-400" />
                  <span>{{ isTestingConnectivity ? 'Тестирование...' : 'Запустить тест' }}</span>
                </button>
              </div>

              <!-- Action 2: Update Check -->
              <div class="flex items-center justify-between p-3 rounded-xl bg-[#14171d]/60 border border-white/[0.04] hover:border-white/[0.08] transition">
                <div class="pr-3">
                  <div class="text-xs font-semibold text-white/90">Обновления программы</div>
                  <div class="text-[10px] text-white/40 mt-0.5">Текущая версия v0.1.0</div>
                </div>
                <button
                  type="button"
                  @click="checkForUpdates(true)"
                  :disabled="isCheckingUpdate"
                  class="px-3 py-1.5 rounded-xl bg-[#202530] hover:bg-[#2a3040] text-xs font-medium text-white/90 flex items-center gap-1.5 transition border border-white/5 cursor-pointer disabled:opacity-50"
                >
                  <RefreshCw class="w-3.5 h-3.5 text-blue-400" :class="isCheckingUpdate ? 'animate-spin' : ''" />
                  <span>{{ isCheckingUpdate ? 'Проверка...' : 'Проверить' }}</span>
                </button>
              </div>
            </div>
          </div>

          <!-- Save Button -->
          <div class="flex justify-end pt-2">
            <button 
              @click="saveSettings"
              class="px-5 py-2.5 rounded-xl bg-gradient-to-r from-blue-600 to-blue-500 hover:from-blue-500 hover:to-blue-400 text-white font-medium text-xs shadow-lg shadow-blue-500/20 transition cursor-pointer"
            >
              Сохранить настройки
            </button>
          </div>
        </div>
      </main>
    </div>

    <!-- Connectivity Test Modal Dialog -->
    <div 
      v-if="showTestModal" 
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4"
    >
      <div class="bg-[#181c24] border border-white/10 rounded-2xl w-full max-w-lg shadow-2xl p-5 space-y-4">
        <div class="flex items-center justify-between border-b border-white/5 pb-3">
          <div class="flex items-center gap-2">
            <Wifi class="w-5 h-5 text-emerald-400" />
            <h3 class="text-sm font-bold text-white">Доступность дата-центров Telegram</h3>
          </div>
          <button @click="showTestModal = false" class="text-white/40 hover:text-white transition cursor-pointer">
            <X class="w-4 h-4" />
          </button>
        </div>

        <div v-if="isTestingConnectivity" class="py-8 flex flex-col items-center justify-center gap-3">
          <RefreshCw class="w-6 h-6 text-emerald-400 animate-spin" />
          <span class="text-xs text-white/60">Отправка тестовых WebSocket пакетов к DC1-DC5...</span>
        </div>

        <div v-else class="space-y-2 max-h-72 overflow-y-auto pr-1">
          <div 
            v-for="item in testResults" 
            :key="item.dc + item.host" 
            class="flex items-center justify-between p-3 rounded-xl bg-[#13161c] border border-white/5"
          >
            <div class="flex items-center gap-3">
              <div 
                :class="[
                  'w-2.5 h-2.5 rounded-full',
                  item.ok ? 'bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.8)]' : 'bg-rose-400 shadow-[0_0_8px_rgba(244,63,94,0.8)]'
                ]" 
              />
              <div>
                <div class="text-xs font-bold text-white">{{ item.dc }}</div>
                <div class="text-[10px] text-white/40 font-mono">{{ item.host }}</div>
              </div>
            </div>

            <div class="text-right">
              <span 
                :class="[
                  'px-2 py-0.5 rounded-lg text-[10px] font-mono font-semibold',
                  item.ok ? 'bg-emerald-500/10 text-emerald-400 border border-emerald-500/20' : 'bg-rose-500/10 text-rose-400 border border-rose-500/20'
                ]"
              >
                {{ item.message }}
              </span>
            </div>
          </div>
        </div>

        <div class="flex items-center justify-between pt-2 border-t border-white/5">
          <button 
            @click="runConnectivityTest" 
            :disabled="isTestingConnectivity"
            class="px-3.5 py-1.5 rounded-xl bg-[#222834] hover:bg-[#2b3342] text-xs text-white flex items-center gap-1.5 transition border border-white/5 cursor-pointer disabled:opacity-50"
          >
            <RefreshCw class="w-3.5 h-3.5" :class="isTestingConnectivity ? 'animate-spin' : ''" />
            <span>Повторить</span>
          </button>

          <button 
            @click="showTestModal = false"
            class="px-4 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-medium shadow-md transition cursor-pointer"
          >
            Закрыть
          </button>
        </div>
      </div>
    </div>

    <!-- Floating Toast Notification -->
    <transition
      enter-active-class="transition duration-300 ease-out transform"
      enter-from-class="opacity-0 translate-y-3 scale-95"
      enter-to-class="opacity-100 translate-y-0 scale-100"
      leave-active-class="transition duration-200 ease-in transform"
      leave-from-class="opacity-100 translate-y-0 scale-100"
      leave-to-class="opacity-0 translate-y-2 scale-95"
    >
      <div 
        v-if="toast.show" 
        class="fixed bottom-6 right-6 z-50 flex items-center gap-3 px-4 py-2.5 rounded-xl bg-[#1b212c] border border-emerald-500/30 text-white text-xs shadow-2xl shadow-black/80 ring-1 ring-emerald-500/20 backdrop-blur-md"
      >
        <div class="w-5 h-5 rounded-full bg-emerald-500/20 flex items-center justify-center text-emerald-400">
          <Check class="w-3.5 h-3.5 stroke-[3]" />
        </div>
        <span class="font-medium text-white/90">{{ toast.message }}</span>
      </div>
    </transition>
  </div>
</template>
