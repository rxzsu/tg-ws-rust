<script setup lang="ts">
import ToggleSwitch from './ToggleSwitch.vue'

defineProps<{
  title: string
  desc?: string
  modelValue: boolean
  disabled?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [boolean]
}>()
</script>

<template>
  <div class="toggle-row">
    <div class="min-w-0 pr-3">
      <div class="text-xs font-semibold text-ink-1">{{ title }}</div>
      <div v-if="desc" class="mt-0.5 text-[11px] leading-relaxed text-ink-2">{{ desc }}</div>
    </div>
    <ToggleSwitch
      :model-value="modelValue"
      :disabled="disabled"
      @update:model-value="emit('update:modelValue', $event)"
    />
  </div>
</template>
