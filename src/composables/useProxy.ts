import { computed, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { EyeOff, Scale, Zap } from 'lucide-vue-next'
import type { Component } from 'vue'
import { pushToast } from './useToast'
import { formatInvokeError } from '../utils/format'

export type TabId = 'main' | 'settings' | 'logs' | 'help'
export type PresetId = 'speed' | 'balanced' | 'stealth'

export interface ProxyConfig {
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
  pool_max_age_secs: number
  pool_keepalive_secs: number
  splitter_enabled: boolean
  connect_timeout_secs: number
  use_builtin_cf_domains: boolean
  transport: string
  quic_relays: string[]
}

export interface DcRoute {
  dc: number
  /** `ok` | `no_front` | `failed` | `timeout` | `no_cf` */
  state: string
  detail: string
}

export interface TelemetrySnapshot {
  connections_total: number
  connections_active: number
  connections_ws: number
  connections_quic: number
  connections_h3: number
  connections_cfproxy: number
  connections_tcp: number
  connections_bad: number
  bytes_up: number
  bytes_down: number
  speed_up_kbps: number
  speed_down_kbps: number
  uptime_secs: number
  last_error: string
  dc_routes: DcRoute[]
}

export interface UpdateCheckResult {
  has_update: boolean
  current_version: string
  latest_version: string
  release_url: string
}

export interface DcTestResult {
  dc: string
  host: string
  ok: boolean
  /** DC1/3/5 have no `/apiws` front; shown informationally, not as an error. */
  no_front: boolean
  latency_ms: number
  message: string
}

export const PRESETS: Record<PresetId, { name: string; desc: string; icon: Component; apply: Partial<ProxyConfig> }> = {
  speed: {
    name: 'Макс. скорость',
    desc: 'Большой пул и буфер 1 МБ, короткий таймаут, без фрагментации',
    icon: Zap,
    apply: { pool_size: 8, buffer_size: 1048576, fallback_cfproxy: true, cfproxy_h2_media: true, splitter_enabled: false, connect_timeout_secs: 5 },
  },
  balanced: {
    name: 'Баланс',
    desc: 'Заводские настройки: пул 4, буфер 256 КБ, фрагментация вкл',
    icon: Scale,
    apply: { pool_size: 4, buffer_size: 262144, fallback_cfproxy: true, cfproxy_h2_media: true, splitter_enabled: true, connect_timeout_secs: 10 },
  },
  stealth: {
    name: 'Макс. скрытность',
    desc: 'Малый пул и буфер 64 КБ, фрагментация вкл, долгий таймаут',
    icon: EyeOff,
    apply: { pool_size: 2, buffer_size: 65536, fallback_cfproxy: true, cfproxy_h2_media: false, splitter_enabled: true, connect_timeout_secs: 15 },
  },
}

const DEFAULT_CONFIG: ProxyConfig = {
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
  pool_max_age_secs: 120,
  pool_keepalive_secs: 15,
  splitter_enabled: true,
  connect_timeout_secs: 10,
  use_builtin_cf_domains: true,
  transport: 'auto',
  quic_relays: [],
}

const MAX_HISTORY = 90

// ── Singleton state (module scope, shared across components) ──────────
const isRunning = ref(false)
const config = ref<ProxyConfig>({ ...DEFAULT_CONFIG })
const telemetry = ref<TelemetrySnapshot>({
  connections_total: 0,
  connections_active: 0,
  connections_ws: 0,
  connections_quic: 0,
  connections_h3: 0,
  connections_cfproxy: 0,
  connections_tcp: 0,
  connections_bad: 0,
  bytes_up: 0,
  bytes_down: 0,
  speed_up_kbps: 0,
  speed_down_kbps: 0,
  uptime_secs: 0,
  last_error: '',
  dc_routes: [],
})
const tgLink = ref('')
const appVersion = ref('')
const platform = ref('')
/** True on Android/iOS: desktop chrome (window buttons, tray, autostart) is hidden. */
const isMobile = computed(() => platform.value === 'android' || platform.value === 'ios')
const autostartActive = ref(false)
const updateInfo = ref<UpdateCheckResult | null>(null)
const isCheckingUpdate = ref(false)
const activePreset = ref<PresetId | null>(null)

const speedHistoryUp = ref<number[]>([])
const speedHistoryDown = ref<number[]>([])

const isTestingConnectivity = ref(false)
const testResults = ref<DcTestResult[]>([])
const showTestModal = ref(false)

let initialized = false

// ── Helpers ───────────────────────────────────────────────────────────
export function splitList(text: string): string[] {
  return text.split(/[\s,;]+/).map(s => s.trim()).filter(Boolean)
}

export function parseDcRedirects(text: string): Record<number, string> {
  const result: Record<number, string> = {}
  for (const pair of splitList(text)) {
    const [dcStr, ip] = pair.split(':')
    if (dcStr && ip) {
      const dcNum = parseInt(dcStr.trim(), 10)
      if (!Number.isNaN(dcNum)) {
        result[dcNum] = ip.trim()
      }
    }
  }
  return result
}

export function dcRedirectsToText(rec: Record<number, string> | undefined): string {
  return Object.entries(rec ?? {}).map(([dc, ip]) => `${dc}: ${ip}`).join(', ')
}

async function updateTgLink(cfg: ProxyConfig) {
  let host = cfg.host
  if (host === '0.0.0.0') {
    try {
      host = await invoke<string>('get_link_host', { host: cfg.host })
    } catch {
      host = '127.0.0.1'
    }
  }

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

// ── Actions ───────────────────────────────────────────────────────────
export async function initProxy(): Promise<boolean> {
  if (initialized) return false
  initialized = true

  try {
    isRunning.value = await invoke<boolean>('is_running')
    const cfg = await invoke<ProxyConfig>('get_config')
    config.value = cfg
    await updateTgLink(cfg)
    try {
      autostartActive.value = await isEnabled()
    } catch {
      autostartActive.value = false
    }
    try {
      appVersion.value = await invoke<string>('get_app_version')
    } catch {
      /* keep default */
    }
    try {
      platform.value = await invoke<string>('get_platform')
    } catch {
      platform.value = 'unknown'
    }
  } catch (err) {
    console.error('Failed to load status:', err)
  }

  listen<TelemetrySnapshot>('telemetry-update', (event) => {
    const payload = event.payload
    if (!isRunning.value) {
      payload.connections_active = 0
      payload.speed_up_kbps = 0
      payload.speed_down_kbps = 0
    }
    telemetry.value = payload
    speedHistoryUp.value.push(payload.speed_up_kbps)
    speedHistoryDown.value.push(payload.speed_down_kbps)
    if (speedHistoryUp.value.length > MAX_HISTORY) speedHistoryUp.value.shift()
    if (speedHistoryDown.value.length > MAX_HISTORY) speedHistoryDown.value.shift()
  })

  checkForUpdates(false)

  try {
    return await invoke<boolean>('is_first_run')
  } catch (e) {
    console.error('First-run check failed:', e)
    return false
  }
}

export async function generateSecret() {
  const arr = new Uint8Array(16)
  window.crypto.getRandomValues(arr)
  config.value.secret = Array.from(arr).map(b => b.toString(16).padStart(2, '0')).join('')
  await updateTgLink(config.value)
}

export async function toggleProxy() {
  if (isRunning.value) {
    try {
      await invoke('stop_proxy')
      isRunning.value = false
      telemetry.value.connections_active = 0
      telemetry.value.speed_up_kbps = 0
      telemetry.value.speed_down_kbps = 0
    } catch (err) {
      console.error('Error stopping proxy:', err)
      pushToast(formatInvokeError(err), 'error', 6000)
    }
    return
  }

  try {
    await invoke<string>('start_proxy')
    await updateTgLink(config.value)
    isRunning.value = true
  } catch (err) {
    console.error('Error starting proxy:', err)
    isRunning.value = false
    pushToast(formatInvokeError(err), 'error', 7000)
  }
}

export async function restartProxy() {
  try {
    await invoke<string>('restart_proxy')
    isRunning.value = true
    pushToast('Прокси перезапущен!')
  } catch (err) {
    console.error('Error restarting proxy:', err)
    pushToast(formatInvokeError(err), 'error', 6000)
  }
}

/** Hide the "last error" banner without touching the running bridge. */
export async function clearLastError() {
  try {
    await invoke('clear_last_error')
    telemetry.value.last_error = ''
  } catch (err) {
    console.error('Error clearing last error:', err)
  }
}

/** Persist settings; returns false when the backend rejected them. */
export async function saveSettings(): Promise<boolean> {
  try {
    await invoke('save_config', { newConfig: config.value })
    await updateTgLink(config.value)
    isRunning.value = await invoke<boolean>('is_running')
    pushToast('Настройки применены и сохранены!')
    return true
  } catch (err) {
    console.error('Error saving settings:', err)
    try {
      isRunning.value = await invoke<boolean>('is_running')
    } catch { /* ignore */ }
    pushToast(formatInvokeError(err), 'error', 7000)
    return false
  }
}

export async function toggleAutostart(next: boolean) {
  try {
    if (next) {
      await enable()
    } else {
      await disable()
    }
    autostartActive.value = next
  } catch (err) {
    console.error('Error toggling autostart:', err)
    // Roll the switch back to the real state instead of lying to the user.
    try {
      autostartActive.value = await isEnabled()
    } catch {
      /* keep previous value */
    }
    pushToast('Не удалось переключить автозапуск', 'error')
  }
}

export async function checkForUpdates(manual = false) {
  isCheckingUpdate.value = true
  try {
    const res = await invoke<UpdateCheckResult>('check_updates')
    updateInfo.value = res
    if (manual) {
      if (res.has_update) {
        pushToast(`Доступна новая версия: ${res.latest_version}!`, 'info')
      } else {
        pushToast('У вас установлена самая актуальная версия!')
      }
    }
  } catch (e) {
    console.error('Update check error:', e)
    if (manual) pushToast('Ошибка при проверке обновлений', 'error')
  } finally {
    isCheckingUpdate.value = false
  }
}

export async function applyPreset(id: PresetId) {
  Object.assign(config.value, PRESETS[id].apply)
  await updateTgLink(config.value)
  activePreset.value = id
  pushToast(`Пресет «${PRESETS[id].name}» применён! Не забудьте сохранить.`, 'info')
}

export async function runConnectivityTest() {
  isTestingConnectivity.value = true
  showTestModal.value = true
  try {
    const custom = splitList(config.value.cfproxy_user_domains.join(', '))
    testResults.value = await invoke<DcTestResult[]>('test_connectivity', { customDomains: custom })
  } catch (e) {
    console.error('Connectivity test error:', e)
  } finally {
    isTestingConnectivity.value = false
  }
}

export function useProxy() {
  return {
    isRunning,
    config,
    telemetry,
    tgLink,
    appVersion,
    platform,
    isMobile,
    autostartActive,
    updateInfo,
    isCheckingUpdate,
    activePreset,
    speedHistoryUp,
    speedHistoryDown,
    isTestingConnectivity,
    testResults,
    showTestModal,
    // Actions are also exposed here: they used to be module-only, and views
    // kept destructuring them from the composable, which silently produced
    // `undefined` click handlers.
    toggleProxy,
    restartProxy,
    saveSettings,
    toggleAutostart,
    applyPreset,
    clearLastError,
    generateSecret,
    checkForUpdates,
    runConnectivityTest,
  }
}
