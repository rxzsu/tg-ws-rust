<script setup lang="ts">
import { Wifi, X, RefreshCw } from 'lucide-vue-next'

export interface DcTestItem {
  dc: string
  host: string
  ok: boolean
  latency_ms: number
  message: string
}

defineProps<{
  show: boolean
  testing: boolean
  results: DcTestItem[]
}>()

const emit = defineEmits<{
  close: []
  retry: []
}>()
</script>

<template>
  <div
    v-if="show"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4"
  >
    <div class="bg-[#181c24] border border-white/10 rounded-2xl w-full max-w-lg shadow-2xl p-5 space-y-4">
      <div class="flex items-center justify-between border-b border-white/5 pb-3">
        <div class="flex items-center gap-2">
          <Wifi class="w-5 h-5 text-emerald-400" />
          <h3 class="text-sm font-bold text-white">Доступность дата-центров Telegram</h3>
        </div>
        <button @click="emit('close')" class="text-white/40 hover:text-white transition cursor-pointer">
          <X class="w-4 h-4" />
        </button>
      </div>

      <div v-if="testing" class="py-8 flex flex-col items-center justify-center gap-3">
        <RefreshCw class="w-6 h-6 text-emerald-400 animate-spin" />
        <span class="text-xs text-white/60">Отправка тестовых WebSocket пакетов к DC1-DC5...</span>
      </div>

      <div v-else class="space-y-2 max-h-72 overflow-y-auto pr-1">
        <div
          v-for="item in results"
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
          @click="emit('retry')"
          :disabled="testing"
          class="px-3.5 py-1.5 rounded-xl bg-[#222834] hover:bg-[#2b3342] text-xs text-white flex items-center gap-1.5 transition border border-white/5 cursor-pointer disabled:opacity-50"
        >
          <RefreshCw class="w-3.5 h-3.5" :class="testing ? 'animate-spin' : ''" />
          <span>Повторить</span>
        </button>

        <button
          @click="emit('close')"
          class="px-4 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-medium shadow-md transition cursor-pointer"
        >
          Закрыть
        </button>
      </div>
    </div>
  </div>
</template>
