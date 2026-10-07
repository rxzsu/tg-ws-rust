<script setup lang="ts">
import { ArrowUp, ArrowDown, Activity } from 'lucide-vue-next'
import { formatBytes } from '../utils/format'

export interface TelemetryData {
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

defineProps<{
  telemetry: TelemetryData
}>()
</script>

<template>
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
</template>
