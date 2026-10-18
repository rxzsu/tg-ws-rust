<script setup lang="ts">
import { nextTick, ref, computed, onBeforeUnmount, onMounted } from 'vue'
import { ChevronDown, Check } from 'lucide-vue-next'

export interface SelectOption {
  value: string
  label: string
  /** Secondary line, shown under the label in the menu. */
  desc?: string
}

const props = withDefaults(defineProps<{
  modelValue: string
  options: SelectOption[]
  disabled?: boolean
  /** Rendered instead of the selected label when nothing is picked. */
  placeholder?: string
}>(), { disabled: false, placeholder: '—' })

const emit = defineEmits<{
  'update:modelValue': [string]
}>()

const open = ref(false)
const activeIndex = ref(-1)
const triggerRef = ref<HTMLElement | null>(null)
const menuRef = ref<HTMLElement | null>(null)
const menuStyle = ref<Record<string, string>>({})

const selected = computed(() => props.options.find((o) => o.value === props.modelValue))
const selectedLabel = computed(() => selected.value?.label ?? props.placeholder)

/** Keep the highlighted row inside the visible area of the menu. */
function scrollActiveIntoView() {
  const menu = menuRef.value
  if (!menu) return
  const node = menu.querySelector<HTMLElement>('[data-active="true"]')
  if (!node) return
  const top = node.offsetTop
  const bottom = top + node.offsetHeight
  if (top < menu.scrollTop) menu.scrollTop = top - 4
  else if (bottom > menu.scrollTop + menu.clientHeight) {
    menu.scrollTop = bottom - menu.clientHeight + 4
  }
}

/** Anchors the teleported menu under the trigger, flipping above if needed. */
function updatePosition() {
  const trigger = triggerRef.value
  const menu = menuRef.value
  if (!trigger || !menu) return

  const rect = trigger.getBoundingClientRect()
  const gap = 6
  const menuHeight = menu.offsetHeight
  const below = window.innerHeight - rect.bottom
  const above = rect.top
  const flip = below < menuHeight + gap * 2 && above > below

  const top = flip ? rect.top - menuHeight - gap : rect.bottom + gap
  // Never let the menu overflow the viewport horizontally.
  const left = Math.min(rect.left, window.innerWidth - rect.width - 8)

  menuStyle.value = {
    top: `${Math.max(8, top)}px`,
    left: `${Math.max(8, left)}px`,
    width: `${rect.width}px`,
  }
}

async function openMenu() {
  if (props.disabled) return
  open.value = true
  const current = props.options.findIndex((o) => o.value === props.modelValue)
  activeIndex.value = current >= 0 ? current : 0
  await nextTick()
  updatePosition()
  scrollActiveIntoView()
}

function closeMenu(focusTrigger = false) {
  if (!open.value) return
  open.value = false
  if (focusTrigger) triggerRef.value?.focus()
}

function toggle() {
  if (open.value) closeMenu(true)
  else void openMenu()
}

function choose(option: SelectOption) {
  emit('update:modelValue', option.value)
  closeMenu(true)
}

function move(delta: number) {
  if (!props.options.length) return
  let next = activeIndex.value
  for (let step = 0; step < props.options.length; step += 1) {
    next = (next + delta + props.options.length) % props.options.length
    if (next >= 0) {
      activeIndex.value = next
      break
    }
  }
  nextTick(scrollActiveIntoView)
}

function edge(index: number) {
  if (!props.options.length) return
  activeIndex.value = index
  nextTick(scrollActiveIntoView)
}

function onTriggerKeydown(event: KeyboardEvent) {
  if (props.disabled) return
  if (event.key === 'Enter' || event.key === ' ' || event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    if (open.value) {
      move(event.key === 'ArrowUp' ? -1 : 1)
    } else {
      void openMenu()
    }
    return
  }
  if (event.key === 'Escape') closeMenu()
}

function onMenuKeydown(event: KeyboardEvent) {
  switch (event.key) {
    case 'ArrowDown':
      event.preventDefault()
      move(1)
      break
    case 'ArrowUp':
      event.preventDefault()
      move(-1)
      break
    case 'Home':
      event.preventDefault()
      edge(0)
      break
    case 'End':
      event.preventDefault()
      edge(props.options.length - 1)
      break
    case 'Enter':
    case ' ':
      event.preventDefault()
      if (activeIndex.value >= 0) choose(props.options[activeIndex.value])
      break
    case 'Escape':
      event.preventDefault()
      closeMenu(true)
      break
    case 'Tab':
      closeMenu()
      break
  }
}

function onPointerDown(event: MouseEvent) {
  const target = event.target as Node
  if (triggerRef.value?.contains(target) || menuRef.value?.contains(target)) return
  closeMenu()
}

function repositionOrClose() {
  if (!open.value) return
  const trigger = triggerRef.value
  if (!trigger) return closeMenu()
  updatePosition()
}

const listeners: Array<() => void> = []
onMounted(() => {
  document.addEventListener('mousedown', onPointerDown, true)
  window.addEventListener('resize', repositionOrClose)
  window.addEventListener('scroll', repositionOrClose, true)
  listeners.push(() => document.removeEventListener('mousedown', onPointerDown, true))
  listeners.push(() => window.removeEventListener('resize', repositionOrClose))
  listeners.push(() => window.removeEventListener('scroll', repositionOrClose, true))
})

onBeforeUnmount(() => {
  listeners.forEach((off) => off())
})
</script>

<template>
  <div class="select-field" :class="{ open, disabled }">
    <button
      ref="triggerRef"
      type="button"
      class="select-trigger"
      role="combobox"
      :aria-expanded="open"
      :aria-disabled="disabled"
      aria-haspopup="listbox"
      :disabled="disabled"
      @click="toggle"
      @keydown="onTriggerKeydown"
    >
      <span class="select-value" :class="{ muted: !selected }">{{ selectedLabel }}</span>
      <ChevronDown class="select-chevron" />
    </button>

    <Teleport to="body">
      <Transition name="menu">
        <div
          v-if="open"
          ref="menuRef"
          class="select-menu"
          :style="menuStyle"
          role="listbox"
          tabindex="-1"
          @keydown="onMenuKeydown"
        >
          <button
            v-for="(option, index) in options"
            :key="option.value"
            type="button"
            class="select-option"
            role="option"
            :aria-selected="option.value === modelValue"
            :data-active="index === activeIndex"
            :class="{ active: index === activeIndex, chosen: option.value === modelValue }"
            @click="choose(option)"
            @mouseenter="activeIndex = index"
          >
            <span class="option-text">
              <span class="option-label">{{ option.label }}</span>
              <span v-if="option.desc" class="option-desc">{{ option.desc }}</span>
            </span>
            <Check v-if="option.value === modelValue" class="option-check" />
          </button>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<style scoped>
.select-field {
  position: relative;
  width: 100%;
}

.select-trigger {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  width: 100%;
  background: rgba(0, 0, 0, 0.3);
  border: 1px solid rgba(255, 255, 255, 0.09);
  border-radius: 0.8rem;
  padding: 0.5rem 0.7rem 0.5rem 0.8rem;
  font-size: 0.78rem;
  color: var(--color-ink-1);
  text-align: left;
  cursor: pointer;
  outline: none;
  transition: border-color 0.15s, box-shadow 0.15s, background 0.15s;
}

.select-trigger:hover:not(:disabled) {
  border-color: rgba(255, 255, 255, 0.16);
}

.select-trigger:focus-visible,
.select-field.open .select-trigger {
  border-color: rgba(42, 171, 238, 0.6);
  box-shadow: 0 0 0 3px rgba(42, 171, 238, 0.14);
}

.select-field.disabled .select-trigger {
  opacity: 0.5;
  cursor: not-allowed;
}

.select-value {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.select-value.muted {
  color: var(--color-ink-3);
}

.select-chevron {
  width: 0.85rem;
  height: 0.85rem;
  flex-shrink: 0;
  color: var(--color-ink-3);
  transition: transform 0.18s ease, color 0.15s;
}

.select-field.open .select-chevron {
  transform: rotate(180deg);
  color: var(--color-accent);
}

/* ── Teleported menu ───────────────────────────────── */
.select-menu {
  position: fixed;
  z-index: 60;
  max-height: 16rem;
  overflow-y: auto;
  padding: 0.3rem;
  border-radius: 0.9rem;
  background: linear-gradient(165deg, rgba(22, 30, 43, 0.97), rgba(12, 17, 25, 0.98));
  -webkit-backdrop-filter: blur(26px) saturate(160%);
  backdrop-filter: blur(26px) saturate(160%);
  border: 1px solid rgba(255, 255, 255, 0.12);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5), inset 0 1px 0 rgba(255, 255, 255, 0.1);
  transform-origin: top center;
  outline: none;
}

.select-option {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  width: 100%;
  padding: 0.45rem 0.55rem;
  border: none;
  border-radius: 0.6rem;
  background: transparent;
  color: var(--color-ink-2);
  font-size: 0.76rem;
  text-align: left;
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}

.select-option.active {
  background: rgba(42, 171, 238, 0.14);
  color: #fff;
}

.select-option.chosen {
  color: var(--color-ink-1);
}

.option-text {
  display: flex;
  flex-direction: column;
  gap: 0.1rem;
  min-width: 0;
  flex: 1;
}

.option-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.option-desc {
  font-size: 0.65rem;
  color: var(--color-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.option-check {
  width: 0.85rem;
  height: 0.85rem;
  flex-shrink: 0;
  color: var(--color-accent);
}

/* ── Transition ────────────────────────────────────── */
.menu-enter-active,
.menu-leave-active {
  transition: opacity 0.14s ease, transform 0.14s ease;
}

.menu-enter-from,
.menu-leave-to {
  opacity: 0;
  transform: scaleY(0.96) translateY(-3px);
}
</style>
