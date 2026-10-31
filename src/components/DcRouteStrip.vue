<script setup lang="ts">
import { computed } from "vue";
import type { DcRoute } from "../composables/useProxy";

const props = defineProps<{
  routes: DcRoute[];
}>();

/** Newest-first list from the backend; keep it compact and stable. */
const routes = computed(() => props.routes.slice(0, 6));

function tone(state: string) {
  switch (state) {
    case "ok":
      return "is-ok";
    case "no_front":
      return "is-warn";
    case "no_cf":
      return "is-warn";
    default:
      return "is-fail";
  }
}

function label(state: string) {
  switch (state) {
    case "ok":
      return "ок";
    case "no_front":
      return "нет WS-фронта";
    case "no_cf":
      return "нет CF-маршрута";
    case "timeout":
      return "таймаут";
    default:
      return "ошибка";
  }
}
</script>

<template>
  <section v-if="routes.length" class="glass rounded-2xl px-3 py-2.5">
    <div
      class="mb-1.5 text-[10px] font-bold uppercase tracking-wider text-ink-3"
    >
      Маршруты DC
    </div>
    <div class="flex flex-wrap gap-1.5">
      <span
        v-for="r in routes"
        :key="r.dc"
        class="route-chip"
        :class="tone(r.state)"
        :title="r.detail"
      >
        <span class="font-bold">DC{{ r.dc }}</span>
        <span class="opacity-60">·</span>
        <span class="max-w-52 truncate">{{ label(r.state) }}</span>
      </span>
    </div>
  </section>
</template>

<style scoped>
.route-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  max-width: 100%;
  padding: 0.15rem 0.5rem;
  border-radius: 999px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  font-family: var(--font-mono, ui-monospace, monospace);
  font-size: 10px;
  color: var(--color-ink-2);
  background: rgba(255, 255, 255, 0.03);
}

.route-chip.is-ok {
  border-color: rgba(52, 211, 153, 0.3);
  background: rgba(16, 185, 129, 0.1);
  color: #6ee7b7;
}

.route-chip.is-warn {
  border-color: rgba(251, 191, 36, 0.3);
  background: rgba(245, 158, 11, 0.1);
  color: #fcd34d;
}

.route-chip.is-fail {
  border-color: rgba(251, 113, 133, 0.3);
  background: rgba(244, 63, 94, 0.1);
  color: #fda4af;
}
</style>
