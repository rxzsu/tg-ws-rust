<script setup lang="ts">
import type { Component } from "vue";

defineProps<{
  title: string;
  subtitle?: string;
  icon?: Component;
}>();
</script>

<template>
  <section class="glass space-y-4 rounded-2xl p-5">
    <header class="flex items-center gap-3">
      <div v-if="icon" class="icon-chip">
        <component :is="icon" class="h-4 w-4" />
      </div>
      <div class="min-w-0">
        <h3 class="text-sm font-bold text-white">{{ title }}</h3>
        <p
          v-if="subtitle"
          class="mt-0.5 text-[11px] leading-relaxed text-ink-2"
        >
          {{ subtitle }}
        </p>
      </div>
      <div class="ml-auto shrink-0">
        <slot name="actions" />
      </div>
    </header>
    <slot />
  </section>
</template>
