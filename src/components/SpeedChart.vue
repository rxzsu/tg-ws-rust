<script setup lang="ts">
import { computed } from "vue";
import { ArrowDown, ArrowUp } from "lucide-vue-next";

const props = defineProps<{
  up: number[];
  down: number[];
}>();

const W = 600;
const H = 150;
const PAD = 8;

function smoothPath(pts: Array<[number, number]>): string {
  if (pts.length < 2) return "";
  let d = `M ${pts[0][0].toFixed(1)},${pts[0][1].toFixed(1)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[Math.min(pts.length - 1, i + 2)];
    const c1x = p1[0] + (p2[0] - p0[0]) / 6;
    const c1y = p1[1] + (p2[1] - p0[1]) / 6;
    const c2x = p2[0] - (p3[0] - p1[0]) / 6;
    const c2y = p2[1] - (p3[1] - p1[1]) / 6;
    d += ` C ${c1x.toFixed(1)},${c1y.toFixed(1)} ${c2x.toFixed(1)},${c2y.toFixed(1)} ${p2[0].toFixed(1)},${p2[1].toFixed(1)}`;
  }
  return d;
}

const chart = computed(() => {
  const up = props.up;
  const down = props.down;
  const n = Math.max(up.length, down.length);
  if (n < 2) return null;

  const max = Math.max(...up, ...down, 1) * 1.15;
  const toPoints = (arr: number[]): Array<[number, number]> =>
    arr.map((v, i) => [
      PAD + (i / (n - 1)) * (W - PAD * 2),
      H - PAD - (v / max) * (H - PAD * 2),
    ]);

  const upPts = toPoints(up);
  const downPts = toPoints(down);
  const upLine = smoothPath(upPts);
  const downLine = smoothPath(downPts);
  const bottom = H - PAD;
  const area = (line: string) =>
    line ? `${line} L ${W - PAD},${bottom} L ${PAD},${bottom} Z` : "";

  return {
    upLine,
    upArea: area(upLine),
    downLine,
    downArea: area(downLine),
  };
});

const hasData = computed(
  () => props.up.some((v) => v > 0) || props.down.some((v) => v > 0),
);

const lastUp = computed(() => props.up.at(-1) ?? 0);
const lastDown = computed(() => props.down.at(-1) ?? 0);
</script>

<template>
  <div class="glass space-y-2 rounded-2xl px-4 py-3">
    <div class="flex items-center justify-between">
      <h3 class="text-xs font-bold text-white">Скорость</h3>
      <div class="flex items-center gap-3 font-mono text-[10px]">
        <span class="flex items-center gap-1.5 text-ink-2">
          <span class="h-1 w-3 rounded-full bg-[#2aabee]" />
          {{ lastUp.toFixed(1) }}
        </span>
        <span class="flex items-center gap-1.5 text-ink-2">
          <span class="h-1 w-3 rounded-full bg-emerald-400" />
          {{ lastDown.toFixed(1) }}
        </span>
      </div>
    </div>

    <div v-if="chart && hasData" class="relative">
      <svg viewBox="0 0 600 150" preserveAspectRatio="none" class="h-24 w-full">
        <defs>
          <linearGradient id="grad-up" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="#2aabee" stop-opacity="0.35" />
            <stop offset="100%" stop-color="#2aabee" stop-opacity="0" />
          </linearGradient>
          <linearGradient id="grad-down" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="#34d399" stop-opacity="0.35" />
            <stop offset="100%" stop-color="#34d399" stop-opacity="0" />
          </linearGradient>
        </defs>

        <!-- grid -->
        <line
          x1="0"
          y1="37.5"
          x2="600"
          y2="37.5"
          stroke="rgba(255,255,255,0.05)"
          stroke-width="1"
        />
        <line
          x1="0"
          y1="75"
          x2="600"
          y2="75"
          stroke="rgba(255,255,255,0.05)"
          stroke-width="1"
        />
        <line
          x1="0"
          y1="112.5"
          x2="600"
          y2="112.5"
          stroke="rgba(255,255,255,0.05)"
          stroke-width="1"
        />

        <path :d="chart.downArea" fill="rgba(52,211,153,0.14)" />
        <path :d="chart.upArea" fill="rgba(42,171,238,0.14)" />
        <path
          :d="chart.downLine"
          fill="none"
          stroke="#34d399"
          stroke-width="2"
          vector-effect="non-scaling-stroke"
          stroke-linecap="round"
        />
        <path
          :d="chart.upLine"
          fill="none"
          stroke="#2aabee"
          stroke-width="2"
          vector-effect="non-scaling-stroke"
          stroke-linecap="round"
        />
      </svg>
    </div>

    <div
      v-else
      class="glass-deep flex h-24 items-center justify-center rounded-xl"
    >
      <p class="text-[11px] text-ink-3">
        {{ up.length < 2 ? "Сбор данных…" : "Ожидание трафика…" }}
      </p>
    </div>
  </div>
</template>
