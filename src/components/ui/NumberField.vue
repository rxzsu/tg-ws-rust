<script setup lang="ts">
import { ChevronUp, ChevronDown } from "lucide-vue-next";

const props = withDefaults(
  defineProps<{
    modelValue: number;
    min?: number;
    max?: number;
    step?: number;
  }>(),
  { min: 0, max: 65535, step: 1 },
);

const emit = defineEmits<{
  "update:modelValue": [number];
}>();

function clamp(v: number): number {
  if (Number.isNaN(v)) return props.min;
  return Math.min(props.max, Math.max(props.min, v));
}

function inc() {
  emit("update:modelValue", clamp((props.modelValue || 0) + props.step));
}

function dec() {
  emit("update:modelValue", clamp((props.modelValue || 0) - props.step));
}

function onInput(e: Event) {
  const v = parseFloat((e.target as HTMLInputElement).value);
  if (Number.isNaN(v)) return;
  // Integer, clamped: "1443.5" / "1e5" used to slip into save_config and
  // die in serde with a cryptic "invalid args" for a u16 field.
  emit("update:modelValue", clamp(Math.round(v)));
}
</script>

<template>
  <div class="num-field">
    <input
      type="number"
      :value="modelValue"
      :step="step"
      :min="min"
      :max="max"
      @input="onInput"
    />
    <div class="steppers">
      <button type="button" tabindex="-1" title="Увеличить" @click="inc">
        <ChevronUp class="h-3 w-3" />
      </button>
      <button type="button" tabindex="-1" title="Уменьшить" @click="dec">
        <ChevronDown class="h-3 w-3" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.num-field {
  display: flex;
  align-items: center;
  width: 100%;
  background: rgba(0, 0, 0, 0.3);
  border: 1px solid rgba(255, 255, 255, 0.09);
  border-radius: 0.8rem;
  padding: 0.25rem 0.35rem 0.25rem 0.8rem;
  transition:
    border-color 0.15s,
    box-shadow 0.15s;
}

.num-field:focus-within {
  border-color: rgba(42, 171, 238, 0.6);
  box-shadow: 0 0 0 3px rgba(42, 171, 238, 0.14);
}

.num-field input {
  width: 100%;
  min-width: 0;
  background: transparent;
  border: none;
  outline: none;
  padding: 0.25rem 0;
  font-size: 0.78rem;
  font-family: var(--font-mono);
  color: var(--color-ink-1);
}

.steppers {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}

.steppers button {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0.1rem 0.3rem;
  color: var(--color-ink-3);
  border-radius: 0.4rem;
  cursor: pointer;
  transition:
    color 0.12s,
    background 0.12s;
}

.steppers button:hover {
  color: #fff;
  background: rgba(255, 255, 255, 0.1);
}

.steppers button:active {
  transform: scale(0.9);
}
</style>
