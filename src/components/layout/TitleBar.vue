<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { Minus, Maximize2, X } from "lucide-vue-next";
import { useProxy } from "../../composables/useProxy";

const { isRunning, appVersion } = useProxy();

async function minimizeApp() {
  await invoke("minimize_window");
}

async function toggleMaximizeApp() {
  await invoke("toggle_maximize_window");
}

async function closeApp() {
  await invoke("close_window");
}
</script>

<template>
  <header
    data-tauri-drag-region
    class="flex h-11 shrink-0 cursor-move items-center justify-between px-3"
  >
    <!-- Left: version -->
    <div class="w-24 font-mono text-[11px] text-ink-3">
      v{{ appVersion || "—" }}
    </div>

    <!-- Centered title with status dot -->
    <div data-tauri-drag-region class="flex items-center gap-2.5">
      <span
        class="h-2 w-2 rounded-full transition-all duration-300"
        :class="
          isRunning
            ? 'animate-pulse bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.9)]'
            : 'bg-rose-400 shadow-[0_0_10px_rgba(251,113,133,0.9)]'
        "
      />
      <span class="text-xs font-semibold tracking-[0.16em] text-white"
        >TG WS PROXY</span
      >
    </div>

    <!-- Window controls -->
    <div class="flex w-24 items-center justify-end gap-1">
      <button class="win-btn" title="Свернуть" @click="minimizeApp">
        <Minus class="h-3.5 w-3.5" />
      </button>
      <button class="win-btn" title="Развернуть" @click="toggleMaximizeApp">
        <Maximize2 class="h-3 w-3" />
      </button>
      <button class="win-btn win-btn-close" title="В трей" @click="closeApp">
        <X class="h-3.5 w-3.5" />
      </button>
    </div>
  </header>
</template>

<style scoped>
.win-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  min-width: 28px;
  height: 24px;
  border-radius: 0.5rem;
  color: var(--color-ink-2);
  cursor: pointer;
  transition: all 0.15s ease;
}

.win-btn:hover {
  color: #fff;
  background: rgba(255, 255, 255, 0.1);
}

.win-btn-close:hover {
  color: #fda4af;
  background: rgba(244, 63, 94, 0.15);
}

.win-btn:active {
  transform: scale(0.92);
}
</style>
