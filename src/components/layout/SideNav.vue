<script setup lang="ts">
import { LayoutDashboard, Settings2, ScrollText, LifeBuoy, Wifi, PanelLeftClose, PanelLeftOpen } from 'lucide-vue-next'
import { useProxy, type TabId } from '../../composables/useProxy'

defineProps<{
  active: TabId
  collapsed: boolean
}>()

const emit = defineEmits<{
  'update:active': [TabId]
  'update:collapsed': [boolean]
  'dc-test': []
}>()

const { isRunning, config, appVersion, isTestingConnectivity } = useProxy()

const items: Array<{ id: TabId; label: string; icon: typeof LayoutDashboard }> = [
  { id: 'main', label: 'Дашборд', icon: LayoutDashboard },
  { id: 'settings', label: 'Настройки', icon: Settings2 },
  { id: 'logs', label: 'Логи', icon: ScrollText },
  { id: 'help', label: 'Помощь', icon: LifeBuoy },
]
</script>

<template>
  <!-- Rail (collapsed state) -->
  <aside
    v-if="collapsed"
    class="glass hidden w-14 shrink-0 flex-col items-center gap-1 rounded-2xl py-3.5 md:flex"
  >
    <button
      class="rail-btn"
      title="Развернуть панель"
      @click="emit('update:collapsed', false)"
    >
      <PanelLeftOpen class="h-4 w-4" />
    </button>

    <div class="my-2 h-px w-8 bg-white/10" />

    <button
      v-for="item in items"
      :key="item.id"
      class="rail-btn"
      :class="{ active: active === item.id }"
      :title="item.label"
      @click="emit('update:active', item.id)"
    >
      <component :is="item.icon" class="h-4 w-4" />
    </button>

    <button
      class="rail-btn"
      :disabled="isTestingConnectivity"
      title="Тест WS-фронтов DC2/DC4"
      @click="emit('dc-test')"
    >
      <Wifi class="h-4 w-4 text-emerald-400" :class="isTestingConnectivity ? 'animate-pulse' : ''" />
    </button>

    <div class="flex-1" />

    <span
      class="h-2 w-2 rounded-full"
      :class="isRunning
        ? 'animate-pulse bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.9)]'
        : 'bg-rose-400 shadow-[0_0_10px_rgba(251,113,133,0.9)]'"
      :title="isRunning ? 'Мост активен' : 'Мост остановлен'"
    />
  </aside>

  <!-- Full sidebar -->
  <aside v-else class="glass hidden w-60 shrink-0 flex-col rounded-2xl p-3.5 md:flex">
    <!-- Logo -->
    <div class="flex items-center gap-3 px-2 pb-5 pt-2">
      <img
        src="/Telegram_logo.svg.webp"
        class="h-10 w-10 rounded-xl object-contain shadow-[0_6px_20px_rgba(42,171,238,0.35)]"
        alt="TG WS Proxy"
      />
      <div class="min-w-0 flex-1">
        <h1 class="text-sm font-bold leading-tight text-white">TG WS Proxy</h1>
        <p class="font-mono text-[10px] text-ink-3">MTProto · WSS</p>
      </div>
      <button
        class="rail-btn shrink-0"
        title="Свернуть панель"
        @click="emit('update:collapsed', true)"
      >
        <PanelLeftClose class="h-4 w-4" />
      </button>
    </div>

    <!-- Navigation -->
    <nav class="space-y-1">
      <button
        v-for="item in items"
        :key="item.id"
        class="nav-item"
        :class="{ active: active === item.id }"
        @click="emit('update:active', item.id)"
      >
        <component :is="item.icon" class="h-4 w-4" />
        <span>{{ item.label }}</span>
      </button>

      <button
        class="nav-item mt-3"
        :disabled="isTestingConnectivity"
        title="Проверить WS-фронты DC2/DC4 и пользовательские домены"
        @click="emit('dc-test')"
      >
        <Wifi class="h-4 w-4 text-emerald-400" :class="isTestingConnectivity ? 'animate-pulse' : ''" />
        <span>{{ isTestingConnectivity ? 'Проверка…' : 'Тест связи DC' }}</span>
      </button>
    </nav>

    <div class="flex-1" />

    <!-- Status footer -->
    <div class="glass-deep space-y-1 rounded-xl p-3">
      <div class="flex items-center gap-2.5">
        <span
          class="h-2 w-2 rounded-full"
          :class="isRunning
            ? 'animate-pulse bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.9)]'
            : 'bg-rose-400 shadow-[0_0_10px_rgba(251,113,133,0.9)]'"
        />
        <span class="text-xs font-semibold text-white">
          {{ isRunning ? 'Мост активен' : 'Мост остановлен' }}
        </span>
      </div>
      <p class="font-mono text-[11px] text-ink-3">{{ config.host }}:{{ config.port }}</p>
      <p v-if="appVersion" class="font-mono text-[10px] text-ink-3">v{{ appVersion }}</p>
    </div>
  </aside>
</template>

<style scoped>
.nav-item {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  width: 100%;
  padding: 0.62rem 0.85rem;
  border-radius: 0.8rem;
  font-size: 0.78rem;
  font-weight: 500;
  color: var(--color-ink-2);
  cursor: pointer;
  transition: all 0.18s ease;
  border: 1px solid transparent;
  text-align: left;
}

.nav-item:hover:not(:disabled) {
  color: var(--color-ink-1);
  background: rgba(255, 255, 255, 0.05);
}

.nav-item:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.nav-item.active {
  color: #fff;
  background: linear-gradient(135deg, rgba(42, 171, 238, 0.18), rgba(42, 171, 238, 0.08));
  border-color: rgba(42, 171, 238, 0.25);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08);
}

.nav-item.active svg {
  color: #5bc5f5;
}

.rail-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 2.4rem;
  height: 2.4rem;
  border-radius: 0.7rem;
  color: var(--color-ink-2);
  cursor: pointer;
  transition: all 0.18s ease;
  border: 1px solid transparent;
  flex-shrink: 0;
}

.rail-btn:hover:not(:disabled) {
  color: var(--color-ink-1);
  background: rgba(255, 255, 255, 0.05);
}

.rail-btn:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.rail-btn.active {
  color: #5bc5f5;
  background: rgba(42, 171, 238, 0.14);
  border-color: rgba(42, 171, 238, 0.25);
}

.rail-btn.active svg {
  color: #5bc5f5;
}
</style>
