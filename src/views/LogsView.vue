<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { RefreshCw, Trash2, FolderOpen } from 'lucide-vue-next'
import { pushToast } from '../composables/useToast'
import { formatInvokeError } from '../utils/format'

const logLines = ref<string[]>([])
const isLoadingLogs = ref(false)
const logContainer = ref<HTMLElement | null>(null)
let pollTimer: number | null = null

async function fetchLogs() {
  isLoadingLogs.value = true
  try {
    logLines.value = await invoke<string[]>('get_recent_logs')
  } catch (e) {
    console.error('Failed to load logs:', e)
  } finally {
    isLoadingLogs.value = false
  }
}

async function openLogsFile() {
  try {
    await invoke('open_log_file')
  } catch (e) {
    pushToast(formatInvokeError(e), 'error')
  }
}

async function clearLogs() {
  try {
    await invoke('clear_logs')
    logLines.value = []
    pushToast('Логи очищены!')
  } catch (e) {
    pushToast(formatInvokeError(e), 'error')
  }
}

function lineClass(line: string): string {
  if (line.includes('ERROR')) return 'text-err'
  if (line.includes('WARN')) return 'text-warn'
  return 'text-white/65'
}

// Auto-scroll to bottom on new lines (live tail)
watch(logLines, async () => {
  await nextTick()
  const el = logContainer.value
  if (el) el.scrollTop = el.scrollHeight
})

onMounted(() => {
  fetchLogs()
  pollTimer = window.setInterval(fetchLogs, 2000)
})

onUnmounted(() => {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer)
    pollTimer = null
  }
})
</script>

<template>
  <div class="glass space-y-3 rounded-2xl p-5">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex items-center gap-2.5">
        <span class="flex h-2 w-2 rounded-full bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.9)]" />
        <h3 class="text-sm font-bold text-white">Живой лог моста</h3>
        <span class="chip">домены скрыты</span>
      </div>
      <button
        class="btn btn-ghost"
        :disabled="isLoadingLogs"
        title="Обновить логи"
        @click="fetchLogs"
      >
        <RefreshCw class="h-3.5 w-3.5" :class="isLoadingLogs ? 'animate-spin' : ''" />
        <span>Обновить</span>
      </button>
    </div>

    <div
      ref="logContainer"
      class="glass-deep h-80 overflow-y-auto rounded-xl p-3.5 font-mono text-[10.5px] leading-relaxed"
    >
      <div v-if="logLines.length">
        <div
          v-for="(line, i) in logLines"
          :key="i"
          class="whitespace-pre-wrap break-all"
          :class="lineClass(line)"
        >{{ line }}</div>
      </div>
      <p v-else class="flex h-full items-center justify-center text-ink-3">
        Логов пока нет. Запустите мост.
      </p>
    </div>

    <p class="text-[10px] leading-relaxed text-ink-3">
      Приватные Cloudflare-домены и воркеры в файле маскируются (***), лог можно безопасно прикладывать к Issue.
    </p>

    <div class="flex items-center justify-end gap-2">
      <button class="btn btn-ghost" @click="clearLogs">
        <Trash2 class="h-3.5 w-3.5 text-err" />
        <span>Очистить</span>
      </button>
      <button class="btn btn-primary" @click="openLogsFile">
        <FolderOpen class="h-3.5 w-3.5" />
        <span>Открыть в Блокноте</span>
      </button>
    </div>
  </div>
</template>
