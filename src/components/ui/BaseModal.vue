<script setup lang="ts">
import { ref, watch, nextTick, onMounted, onUnmounted } from "vue";

const props = withDefaults(
  defineProps<{
    show: boolean;
    title?: string;
    wide?: boolean;
    /** Allow closing via backdrop click / Escape */
    dismissible?: boolean;
  }>(),
  { dismissible: true },
);

const emit = defineEmits<{ close: [] }>();

const panel = ref<HTMLElement | null>(null);
let lastFocused: HTMLElement | null = null;

watch(
  () => props.show,
  async (open) => {
    if (open) {
      // Move focus into the dialog: keyboard users start inside it instead
      // of tabbing blind through the page behind the backdrop.
      lastFocused = document.activeElement as HTMLElement | null;
      await nextTick();
      panel.value?.focus();
    } else if (lastFocused) {
      lastFocused.focus();
      lastFocused = null;
    }
  },
);

function onKey(e: KeyboardEvent) {
  if (!props.show) return;
  if (e.key === "Escape" && props.dismissible) {
    emit("close");
    return;
  }
  // Trap Tab inside the panel — critical for the non-dismissible onboarding
  // wizard, whose backdrop cannot be clicked away.
  if (e.key === "Tab" && panel.value) {
    const focusables = panel.value.querySelectorAll<HTMLElement>(
      'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
    );
    if (focusables.length === 0) return;
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }
}

onMounted(() => window.addEventListener("keydown", onKey));
onUnmounted(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <Teleport to="body">
    <Transition name="modal">
      <div
        v-if="show"
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-md"
        @click.self="dismissible && emit('close')"
      >
        <div
          ref="panel"
          role="dialog"
          aria-modal="true"
          :aria-label="title"
          tabindex="-1"
          class="glass-strong modal-panel w-full space-y-4 rounded-2xl p-5 outline-none"
          :class="wide ? 'max-w-xl' : 'max-w-md'"
        >
          <header
            v-if="title"
            class="flex items-center justify-between border-b border-white/[0.07] pb-3"
          >
            <h3 class="text-sm font-bold text-white">{{ title }}</h3>
            <slot name="actions" />
          </header>
          <slot />
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.modal-enter-active {
  transition: opacity 0.22s ease;
}
.modal-enter-active .modal-panel {
  transition: transform 0.26s cubic-bezier(0.34, 1.56, 0.64, 1);
}
.modal-enter-from {
  opacity: 0;
}
.modal-enter-from .modal-panel {
  transform: scale(0.94) translateY(10px);
}
.modal-leave-active {
  transition: opacity 0.15s ease;
}
.modal-leave-to {
  opacity: 0;
}
</style>
