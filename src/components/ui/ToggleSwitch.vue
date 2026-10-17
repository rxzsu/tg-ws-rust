<script setup lang="ts">
defineProps<{
  modelValue: boolean
  disabled?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [boolean]
}>()
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="modelValue"
    :disabled="disabled"
    class="switch"
    :class="{ on: modelValue }"
    @click="emit('update:modelValue', !modelValue)"
  >
    <span class="knob" />
  </button>
</template>

<style scoped>
.switch {
  position: relative;
  display: inline-flex;
  width: 44px;
  height: 26px;
  flex-shrink: 0;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.1);
  border: 1px solid rgba(255, 255, 255, 0.1);
  cursor: pointer;
  transition: background 0.2s ease, border-color 0.2s ease, box-shadow 0.2s ease;
}

.switch:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.switch .knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 20px;
  height: 20px;
  border-radius: 999px;
  background: #fff;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.4);
  transition: transform 0.22s cubic-bezier(0.4, 0, 0.2, 1);
}

.switch.on {
  background: linear-gradient(135deg, #33b3f0, #1e96d2);
  border-color: rgba(42, 171, 238, 0.5);
  box-shadow: 0 0 12px rgba(42, 171, 238, 0.35);
}

.switch.on .knob {
  transform: translateX(18px);
}
</style>
