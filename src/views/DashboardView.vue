<script setup lang="ts">
import { computed } from "vue";
import { Square, Power, RefreshCw } from "lucide-vue-next";
import TelemetryGrid from "../components/TelemetryGrid.vue";
import SpeedChart from "../components/SpeedChart.vue";
import TgLinkCard from "../components/TgLinkCard.vue";
import DcRouteStrip from "../components/DcRouteStrip.vue";
import GrainGradient from "../components/ui/GrainGradient.vue";
import {
  useProxy,
  toggleProxy,
  restartProxy,
  clearLastError,
} from "../composables/useProxy";

const {
  isRunning,
  config,
  telemetry,
  tgLink,
  speedHistoryUp,
  speedHistoryDown,
  isToggling,
} = useProxy();

// Module-level constants keep a stable array identity: an inline literal in
// the template created a new reference on every render (once a second from
// telemetry), so the shader rebuilt its uniforms every tick.
const COLORS_ON = ["#0ea5e9", "#2aabee", "#22d3ee", "#34d399"];
const COLORS_OFF = ["#1e3a5f", "#2aabee", "#312e81", "#0f172a"];
const gradientColors = computed(() =>
  isRunning.value ? COLORS_ON : COLORS_OFF,
);

async function copyError() {
  try {
    await navigator.clipboard.writeText(telemetry.value.last_error);
  } catch {
    /* clipboard blocked: nothing else to do */
  }
}

/** Compact uptime: 42с → 7м 12с → 3ч 07м. */
const uptimeLabel = computed(() => {
  const total = telemetry.value.uptime_secs;
  if (!isRunning.value || total <= 0) return "";
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}ч ${String(m).padStart(2, "0")}м`;
  if (m > 0) return `${m}м ${String(s).padStart(2, "0")}с`;
  return `${s}с`;
});
</script>

<template>
  <div class="grid grid-cols-1 items-stretch gap-3 md:grid-cols-2">
    <!-- LEFT: control (stretches to match right column height) -->
    <section class="glass relative overflow-hidden rounded-2xl px-5 py-4">
      <!-- Animated shader backdrop -->
      <GrainGradient
        :colors="gradientColors"
        color-back="#05070c"
        :softness="0.55"
        :intensity="0.45"
        :noise="0.3"
        shape="corners"
        :speed="isRunning ? 1 : 0.25"
        class="opacity-40 transition-opacity duration-700"
      />
      <div
        class="pointer-events-none absolute inset-0 bg-gradient-to-t from-[#090c12]/85 via-[#090c12]/30 to-transparent"
      />

      <div class="relative flex h-full flex-col justify-between gap-4">
        <div class="min-w-0 space-y-2.5">
          <span class="status-pill" :class="isRunning ? 'ok' : 'off'">
            <span class="dot" />
            {{ isRunning ? "Мост активен" : "Мост остановлен" }}
          </span>
          <h1 class="text-xl font-bold tracking-tight text-white">
            {{ isRunning ? "Прокси работает" : "Прокси не запущен" }}
          </h1>
          <p class="text-xs leading-relaxed text-ink-2">
            {{
              isRunning
                ? "MTProto-мост маскирует трафик под HTTPS/WSS и маршрутизирует его в Telegram."
                : "Запустите мост, чтобы Telegram-клиент мог подключаться через этот прокси."
            }}
          </p>
        </div>

        <div class="space-y-3">
          <div class="flex flex-wrap items-center gap-1.5">
            <span class="chip mono">{{ config.host }}:{{ config.port }}</span>
            <span class="chip">MTProto · WSS</span>
            <span v-if="uptimeLabel" class="chip mono emerald-border"
              >● {{ uptimeLabel }}</span
            >
            <span v-if="config.fake_tls_domain" class="chip emerald-border"
              >SNI: {{ config.fake_tls_domain }}</span
            >
          </div>

          <div
            v-if="telemetry.last_error"
            class="flex items-start gap-2 rounded-lg border border-rose-400/25 bg-rose-500/10 px-2.5 py-1.5"
          >
            <span
              class="min-w-0 flex-1 truncate font-mono text-[10px] leading-4 text-rose-200/90"
              :title="telemetry.last_error"
            >
              ⚠ {{ telemetry.last_error }}
            </span>
            <button
              class="shrink-0 rounded px-1.5 text-[10px] text-rose-200/70 transition hover:bg-rose-400/15 hover:text-rose-100"
              title="Скопировать текст ошибки для issue"
              @click="copyError"
            >
              копировать
            </button>
            <button
              class="shrink-0 rounded px-1.5 text-[10px] text-rose-200/70 transition hover:bg-rose-400/15 hover:text-rose-100"
              title="Скрыть"
              @click="clearLastError"
            >
              ✕
            </button>
          </div>

          <div class="flex items-center gap-2">
            <button
              class="power-btn"
              :class="{ on: isRunning }"
              :disabled="isToggling"
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
      <DcRouteStrip :routes="telemetry.dc_routes ?? []" />
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
