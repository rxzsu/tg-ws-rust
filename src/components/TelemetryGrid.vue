<script setup lang="ts">
import { ArrowUp, ArrowDown, Activity, Waypoints } from 'lucide-vue-next'
import { formatBytes } from '../utils/format'
import type { TelemetrySnapshot } from '../composables/useProxy'

defineProps<{
  telemetry: TelemetrySnapshot
}>()
</script>

<template>
  <div class="grid grid-cols-2 gap-2.5 sm:grid-cols-4">
    <!-- Upload -->
    <div class="glass min-w-0 rounded-xl px-3.5 py-2.5 transition duration-200 hover:border-white/[0.16]">
      <div class="flex items-center justify-between text-[10px] font-medium text-ink-2">
        <span>Отдача</span>
        <ArrowUp class="h-3 w-3 text-[#5bc5f5]" />
      </div>
      <div class="mt-1 font-mono text-sm font-bold tracking-tight text-white">
        {{ telemetry.speed_up_kbps.toFixed(1) }}
        <span class="text-[10px] font-normal text-ink-3">Кб/с</span>
      </div>
      <div class="font-mono text-[9.5px] text-ink-3">
        {{ formatBytes(telemetry.bytes_up) }}
      </div>
    </div>

    <!-- Download -->
    <div class="glass min-w-0 rounded-xl px-3.5 py-2.5 transition duration-200 hover:border-white/[0.16]">
      <div class="flex items-center justify-between text-[10px] font-medium text-ink-2">
        <span>Загрузка</span>
        <ArrowDown class="h-3 w-3 text-emerald-400" />
      </div>
      <div class="mt-1 font-mono text-sm font-bold tracking-tight text-emerald-300">
        {{ telemetry.speed_down_kbps.toFixed(1) }}
        <span class="text-[10px] font-normal text-ink-3">Кб/с</span>
      </div>
      <div class="font-mono text-[9.5px] text-ink-3">
        {{ formatBytes(telemetry.bytes_down) }}
      </div>
    </div>

    <!-- Active sockets -->
    <div class="glass min-w-0 rounded-xl px-3.5 py-2.5 transition duration-200 hover:border-white/[0.16]">
      <div class="flex items-center justify-between text-[10px] font-medium text-ink-2">
        <span>Сокеты</span>
        <Activity class="h-3 w-3 text-amber-400" />
      </div>
      <div class="mt-1 font-mono text-sm font-bold tracking-tight text-white">
        {{ telemetry.connections_active }}
      </div>
      <div class="text-[9.5px] text-ink-3">
        Всего: {{ telemetry.connections_total }}
      </div>
    </div>

    <!-- Transport -->
    <div class="glass min-w-0 rounded-xl px-3.5 py-2.5 transition duration-200 hover:border-white/[0.16]">
      <div class="flex items-center justify-between gap-1 text-[10px] font-medium text-ink-2">
        <span class="truncate">Транспорт</span>
        <Waypoints class="h-3 w-3 shrink-0 text-violet-300" />
      </div>
      <div class="mt-1 truncate font-mono text-sm font-bold tracking-tight text-violet-300">
        WS + H2
      </div>
      <div class="truncate text-[9.5px] text-ink-3">
        {{ telemetry.connections_quic > 0 ? `QUIC: ${telemetry.connections_quic}` : 'Cloudflare CDN' }}
      </div>
    </div>
  </div>
</template>
