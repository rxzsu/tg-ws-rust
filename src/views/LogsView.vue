<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, Trash2, FolderOpen } from "lucide-vue-next";
import { pushToast } from "../composables/useToast";
import { formatInvokeError } from "../utils/format";

const logLines = ref<{ id: number; text: string }[]>([]);
const isLoadingLogs = ref(false);
const logContainer = ref<HTMLElement | null>(null);
let pollTimer: number | null = null;

interface LogTail {
  lines: string[];
  offset: number;
  truncated: boolean;
}

const MAX_LINES = 800;
// null = first load (full refresh); afterwards an incremental byte offset.
let logOffset: number | null = null;
// Monotonic per-line id: index keys shifted on every splice, so Vue patched
// all ~800 rows every poll; stable keys let it drop the leading nodes and
// touch nothing else.
let nextLineId = 0;
// One fetch at a time: an overlapping manual refresh and poll could apply
// their byte offsets out of order and duplicate lines on the next tick.
let inFlight = false;
// Stick to the live tail only while the user is already reading the bottom.
let stickToBottom = true;

async function fetchLogs(full = false) {
  if (inFlight) return;
  inFlight = true;
  // Only the manual refresh shows the spinner: the 2s poll must not flash
  // the button disabled/spinning twice a second.
  if (full) isLoadingLogs.value = true;
  try {
    const tail = await invoke<LogTail>("get_log_tail", {
      offset: full ? null : logOffset,
    });
    if (tail.truncated || logOffset === null) {
      nextLineId = 0;
      logLines.value = tail.lines.map((text) => ({ id: nextLineId++, text }));
    } else if (tail.lines.length) {
      for (const text of tail.lines) {
        logLines.value.push({ id: nextLineId++, text });
      }
      if (logLines.value.length > MAX_LINES) {
        logLines.value.splice(0, logLines.value.length - MAX_LINES);
      }
    }
    logOffset = tail.offset;
  } catch (e) {
    console.error("Failed to load logs:", e);
  } finally {
    inFlight = false;
    if (full) isLoadingLogs.value = false;
  }
}

function onLogScroll() {
  const el = logContainer.value;
  if (el) {
    stickToBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  }
}

async function openLogsFile() {
  try {
    await invoke("open_log_file");
  } catch (e) {
    pushToast(formatInvokeError(e), "error");
  }
}

async function clearLogs() {
  try {
    await invoke("clear_logs");
    logLines.value = [];
    logOffset = null;
    pushToast("Логи очищены!");
  } catch (e) {
    pushToast(formatInvokeError(e), "error");
  }
}

function lineClass(line: string): string {
  if (line.includes("ERROR")) return "text-err";
  if (line.includes("WARN")) return "text-warn";
  return "text-white/65";
}

// Auto-scroll on new lines, but only while the user is reading the bottom:
// scrolling up to inspect history must not be yanked back every 2 seconds.
watch(logLines, async () => {
  if (!stickToBottom) return;
  await nextTick();
  const el = logContainer.value;
  if (el) el.scrollTop = el.scrollHeight;
});

onMounted(() => {
  fetchLogs(true);
  pollTimer = window.setInterval(() => fetchLogs(false), 2000);
});

onUnmounted(() => {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
});
</script>

<template>
  <div class="glass space-y-3 rounded-2xl p-5">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex items-center gap-2.5">
        <span
          class="flex h-2 w-2 rounded-full bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.9)]"
        />
        <h3 class="text-sm font-bold text-white">Живой лог моста</h3>
        <span class="chip">домены скрыты</span>
      </div>
      <button
        class="btn btn-ghost"
        :disabled="isLoadingLogs"
        title="Обновить логи"
        @click="fetchLogs(true)"
      >
        <RefreshCw
          class="h-3.5 w-3.5"
          :class="isLoadingLogs ? 'animate-spin' : ''"
        />
        <span>Обновить</span>
      </button>
    </div>

    <div
      ref="logContainer"
      class="glass-deep h-80 overflow-y-auto rounded-xl p-3.5 font-mono text-[10.5px] leading-relaxed"
      @scroll="onLogScroll"
    >
      <div v-if="logLines.length">
        <div
          v-for="line in logLines"
          :key="line.id"
          class="whitespace-pre-wrap break-all"
          :class="lineClass(line.text)"
        >
          {{ line.text }}
        </div>
      </div>
      <p v-else class="flex h-full items-center justify-center text-ink-3">
        Логов пока нет. Запустите мост.
      </p>
    </div>

    <p class="text-[10px] leading-relaxed text-ink-3">
      Приватные Cloudflare-домены и воркеры в файле маскируются (***), лог можно
      безопасно прикладывать к Issue.
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
