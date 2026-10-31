<script setup lang="ts">
import { useToasts, dismissToast } from "../composables/useToast";
import { Check, AlertCircle, Info, X } from "lucide-vue-next";

const toasts = useToasts();
</script>

<template>
  <div
    aria-live="polite"
    role="status"
    class="pointer-events-none fixed bottom-5 right-5 z-[60] flex flex-col items-end gap-2"
  >
    <TransitionGroup
      enter-active-class="transition duration-300 ease-out"
      enter-from-class="opacity-0 translate-y-3 scale-95"
      enter-to-class="opacity-100 translate-y-0 scale-100"
      leave-active-class="transition duration-200 ease-in"
      leave-from-class="opacity-100"
      leave-to-class="opacity-0 translate-y-2 scale-95"
    >
      <div v-for="t in toasts" :key="t.id" class="toast-pill" :class="t.kind">
        <span
          class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full"
          :class="{
            'bg-emerald-500/20 text-emerald-400': t.kind === 'ok',
            'bg-rose-500/20 text-rose-400': t.kind === 'error',
            'bg-sky-500/20 text-sky-400': t.kind === 'info',
          }"
        >
          <Check v-if="t.kind === 'ok'" class="h-3.5 w-3.5 stroke-[3]" />
          <AlertCircle v-else-if="t.kind === 'error'" class="h-3.5 w-3.5" />
          <Info v-else class="h-3.5 w-3.5" />
        </span>
        <span class="min-w-0 flex-1 leading-snug">{{ t.message }}</span>
        <button
          class="shrink-0 text-ink-3 transition hover:text-white"
          @click="dismissToast(t.id)"
        >
          <X class="h-3.5 w-3.5" />
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>
