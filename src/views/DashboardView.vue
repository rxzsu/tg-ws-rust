<script setup lang="ts">
import { Square, Power, RefreshCw } from 'lucide-vue-next'
import TelemetryGrid from '../components/TelemetryGrid.vue'
import SpeedChart from '../components/SpeedChart.vue'
import TgLinkCard from '../components/TgLinkCard.vue'
import GrainGradient from '../components/ui/GrainGradient.vue'
import { useProxy } from '../composables/useProxy'

const { isRunning, config, telemetry, tgLink, speedHistoryUp, speedHistoryDown, toggleProxy, restartProxy } = useProxy()
</script>

<template>
  <div class="grid grid-cols-1 items-stretch gap-3 md:grid-cols-2">
    <!-- LEFT: control (stretches to match right column height) -->
    <section class="glass relative overflow-hidden rounded-2xl px-5 py-4">
      <!-- Animated shader backdrop -->
      <GrainGradient
        :colors="isRunning
          ? ['#0ea5e9', '#2aabee', '#22d3ee', '#34d399']
          : ['#1e3a5f', '#2aabee', '#312e81', '#0f172a']"
        color-back="#05070c"
        :softness="0.55"
        :intensity="0.45"
        :noise="0.3"
        shape="corners"
        :speed="isRunning ? 1 : 0.25"
        class="opacity-40 transition-opacity duration-700"
      />
      <div class="pointer-events-none absolute inset-0 bg-gradient-to-t from-[#090c12]/85 via-[#090c12]/30 to-transparent" />

      <div class="relative flex h-full flex-col justify-between gap-4">
        <div class="min-w-0 space-y-2.5">
          <span class="status-pill" :class="isRunning ? 'ok' : 'off'">
            <span class="dot" />
            {{ isRunning ? 'Мост активен' : 'Мост остановлен' }}
          </span>
          <h1 class="text-xl font-bold tracking-tight text-white">
            {{ isRunning ? 'Прокси работает' : 'Прокси не запущен' }}
          </h1>
          <p class="text-xs leading-relaxed text-ink-2">
            {{ isRunning
              ? 'MTProto-мост маскирует трафик под HTTPS/WSS и маршрутизирует его в Telegram.'
              : 'Запустите мост, чтобы Telegram-клиент мог подключаться через этот прокси.' }}
          </p>
        </div>

        <div class="space-y-3">
          <div class="flex flex-wrap items-center gap-1.5">
            <span class="chip mono">{{ config.host }}:{{ config.port }}</span>
            <span class="chip">MTProto · WSS</span>
            <span v-if="config.fake_tls_domain" class="chip emerald-border">SNI: {{ config.fake_tls_domain }}</span>
          </div>

          <div class="flex items-center gap-2">
            <button
              class="power-btn"
              :class="{ on: isRunning }"
              :title="isRunning ? 'Остановить мост' : 'Запустить мост'"
              @click="toggleProxy"
            >
              <component :is="isRunning ? Square : Power" class="h-4 w-4" />
            </button>
            <button
              class="btn btn-ghost"
              :disabled="!isRunning"
              title="Перезапустить прокси"
              @click="restartProxy"
            >
              <RefreshCw class="h-3.5 w-3.5" />
              <span>Рестарт</span>
            </button>
          </div>
        </div>
      </div>
    </section>

    <!-- RIGHT: info (metrics + chart + link) -->
    <div class="flex flex-col gap-3">
      <TelemetryGrid :telemetry="telemetry" />
      <SpeedChart :up="speedHistoryUp" :down="speedHistoryDown" />
      <TgLinkCard :link="tgLink" class="flex-1" />
    </div>
  </div>
</template>

<style scoped>
.chip.emerald-border {
  border-color: rgba(52, 211, 153, 0.3);
  background: rgba(16, 185, 129, 0.1);
  color: #6ee7b7;
}
</style>
