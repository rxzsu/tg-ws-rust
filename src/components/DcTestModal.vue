<script setup lang="ts">
import { Wifi, RefreshCw } from "lucide-vue-next";
import BaseModal from "./ui/BaseModal.vue";
import type { DcTestResult } from "../composables/useProxy";

defineProps<{
  show: boolean;
  testing: boolean;
  results: DcTestResult[];
}>();

const emit = defineEmits<{
  close: [];
  retry: [];
}>();
</script>

<template>
  <BaseModal
    :show="show"
    title="Доступность дата-центров Telegram"
    @close="emit('close')"
  >
    <template #actions>
      <Wifi class="h-4 w-4 text-emerald-400" />
    </template>

    <div
      v-if="testing"
      class="flex flex-col items-center justify-center gap-3 py-8"
    >
      <RefreshCw class="h-6 w-6 animate-spin text-emerald-400" />
      <span class="text-xs text-ink-2"
        >Проверка WS-фронтов DC2/DC4 и пользовательских доменов…</span
      >
    </div>

    <div v-else class="max-h-72 space-y-2 overflow-y-auto pr-1">
      <div
        v-for="item in results"
        :key="item.dc + item.host"
        class="glass-deep flex items-center justify-between rounded-xl p-3"
      >
        <div class="flex items-center gap-3">
          <span
            class="h-2.5 w-2.5 rounded-full"
            :class="
              item.ok
                ? 'bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.9)]'
                : item.no_front
                  ? 'bg-ink-3'
                  : 'bg-rose-400 shadow-[0_0_8px_rgba(251,113,133,0.9)]'
            "
          />
          <div>
            <div class="text-xs font-bold text-white">{{ item.dc }}</div>
            <div class="font-mono text-[10px] text-ink-3">{{ item.host }}</div>
          </div>
        </div>

        <span
          class="rounded-lg px-2 py-0.5 font-mono text-[10px] font-semibold"
          :class="
            item.ok
              ? 'border border-emerald-500/20 bg-emerald-500/10 text-emerald-400'
              : item.no_front
                ? 'border border-white/10 bg-white/[0.04] text-ink-3'
                : 'border border-rose-500/20 bg-rose-500/10 text-rose-400'
          "
        >
          {{ item.message }}
        </span>
      </div>
    </div>

    <div
      class="flex items-center justify-between border-t border-white/[0.07] pt-3"
    >
      <button class="btn btn-ghost" :disabled="testing" @click="emit('retry')">
        <RefreshCw class="h-3.5 w-3.5" :class="testing ? 'animate-spin' : ''" />
        <span>Повторить</span>
      </button>

      <button class="btn btn-primary" @click="emit('close')">Закрыть</button>
    </div>
  </BaseModal>
</template>
