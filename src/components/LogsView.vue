<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { RefreshCw, Trash2, FolderOpen } from 'lucide-vue-next'

const emit = defineEmits<{
  notify: [message: string]
  'notify-error': [message: string]
}>()

const logLines = ref<string[]>([])
const isLoadingLogs = ref(false)
let pollTimer: number | null = null

function formatError(err: unknown): string {
  if (typeof err === 'string') return err
  if (err && typeof err === 'object' && 'message' in err) return String((err as { message: unknown }).message)
  return String(err)
}

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
    emit('notify-error', formatError(e))
  }
}

async function clearLogs() {
  try {
    await invoke('clear_logs')
    logLines.value = []
    emit('notify', 'Логи очищены!')
  } catch (e) {
    emit('notify-error', formatError(e))
  }
}

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
  <div class="space-y-4 max-w-2xl mx-auto pb-4">
    <div class="bg-[#1c2027] border border-white/5 rounded-2xl p-4 space-y-3 shadow-md">
      <div class="flex items-center justify-between">
        <div class="text-[11px] font-bold text-white/60 uppercase tracking-wider">Живой лог моста (файл, домены скрыты)</div>
        <button
          @click="fetchLogs"
          :disabled="isLoadingLogs"
          class="p-1.5 rounded-lg bg-[#202530] hover:bg-[#2a3040] text-white/70 transition border border-white/5 cursor-pointer disabled:opacity-50"
          title="Обновить логи"
        >
          <RefreshCw class="w-3.5 h-3.5" :class="isLoadingLogs ? 'animate-spin' : ''" />
        </button>
      </div>
      <pre class="bg-[#0e1117] border border-white/5 rounded-xl p-3 text-[10px] font-mono text-white/70 h-72 overflow-y-auto whitespace-pre-wrap break-all">{{ logLines.length ? logLines.join('\n') : 'Логов пока нет. Запустите мост.' }}</pre>
      <div class="text-[10px] text-white/30">Приватные Cloudflare-домены и воркеры в файле маскируются (***), лог можно безопасно прикладывать к Issue.</div>
      <div class="flex items-center justify-end gap-2">
        <button
          @click="clearLogs"
          class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-[#202530] hover:bg-[#2a3040] text-xs text-white/80 transition border border-white/5 cursor-pointer"
        >
          <Trash2 class="w-3.5 h-3.5 text-rose-400" />
          <span>Очистить</span>
        </button>
        <button
          @click="openLogsFile"
          class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-medium shadow-md transition cursor-pointer"
        >
          <FolderOpen class="w-3.5 h-3.5" />
          <span>Открыть в Блокноте</span>
        </button>
      </div>
    </div>
  </div>
</template>
