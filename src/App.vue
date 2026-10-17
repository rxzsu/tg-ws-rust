<script setup lang="ts">
import { ref, onMounted, type Component } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Download, LayoutDashboard, Settings2, ScrollText, LifeBuoy, Wifi } from 'lucide-vue-next'
import TitleBar from './components/layout/TitleBar.vue'
import SideNav from './components/layout/SideNav.vue'
import ToastHost from './components/ToastHost.vue'
import DcTestModal from './components/DcTestModal.vue'
import OnboardingWizard from './components/OnboardingWizard.vue'
import DashboardView from './views/DashboardView.vue'
import SettingsView from './views/SettingsView.vue'
import LogsView from './views/LogsView.vue'
import GuidesView from './views/GuidesView.vue'
import { useProxy, initProxy, runConnectivityTest, type TabId } from './composables/useProxy'
import { pushToast } from './composables/useToast'

const activeTab = ref<TabId>('main')
const showWizard = ref(false)
const sidebarCollapsed = ref(false)

const {
  isRunning,
  updateInfo,
  isTestingConnectivity,
  testResults,
  showTestModal,
} = useProxy()

onMounted(async () => {
  const firstRun = await initProxy()
  showWizard.value = firstRun

  listen<string>('proxy-error', (event) => {
    console.error('Proxy error:', event.payload)
    isRunning.value = false
    pushToast(event.payload, 'error', 7000)
  })

  listen('open-settings', () => {
    activeTab.value = 'settings'
  })
})

const views: Record<TabId, Component> = {
  main: DashboardView,
  settings: SettingsView,
  logs: LogsView,
  help: GuidesView,
}

const mobileItems: Array<{ id: TabId; label: string; icon: Component }> = [
  { id: 'main', label: 'Мост', icon: LayoutDashboard },
  { id: 'settings', label: 'Настройки', icon: Settings2 },
  { id: 'logs', label: 'Логи', icon: ScrollText },
  { id: 'help', label: 'Помощь', icon: LifeBuoy },
]

async function openRelease() {
  if (updateInfo.value) {
    await invoke('open_url', { url: updateInfo.value.release_url })
  }
}
</script>

<template>
  <div class="flex h-screen flex-col overflow-hidden">
    <TitleBar />

    <div class="flex min-h-0 flex-1 gap-3 px-3 pb-3">
      <SideNav
        v-model:active="activeTab"
        v-model:collapsed="sidebarCollapsed"
        @dc-test="runConnectivityTest"
      />

      <main class="glass min-w-0 flex-1 overflow-y-auto rounded-2xl p-4 md:p-6">
        <div class="mx-auto w-full max-w-3xl space-y-4">
          <!-- Update banner -->
          <div v-if="updateInfo?.has_update" class="update-banner">
            <div class="flex min-w-0 items-center gap-3">
              <span class="icon-chip h-8 w-8">
                <Download class="h-4 w-4" />
              </span>
              <div class="min-w-0">
                <div class="text-xs font-bold text-white">
                  Доступна новая версия {{ updateInfo.latest_version }}!
                </div>
                <div class="text-[10px] text-ink-2">
                  Текущая версия {{ updateInfo.current_version }}
                </div>
              </div>
            </div>
            <button class="btn btn-primary shrink-0" @click="openRelease">
              Скачать
            </button>
          </div>

          <Transition name="fade" mode="out-in">
            <component :is="views[activeTab]" />
          </Transition>
        </div>
      </main>
    </div>

    <!-- Mobile bottom navigation -->
    <nav class="glass-strong mx-3 mb-3 flex items-center justify-around rounded-2xl px-2 py-2 md:hidden">
      <button
        v-for="item in mobileItems"
        :key="item.id"
        class="mobile-tab"
        :class="{ active: activeTab === item.id }"
        @click="activeTab = item.id"
      >
        <component :is="item.icon" class="h-5 w-5" />
        <span>{{ item.label }}</span>
      </button>
      <button
        class="mobile-tab"
        :disabled="isTestingConnectivity"
        @click="runConnectivityTest"
      >
        <Wifi class="h-5 w-5 text-emerald-400" :class="isTestingConnectivity ? 'animate-pulse' : ''" />
        <span>{{ isTestingConnectivity ? 'Тест…' : 'DC' }}</span>
      </button>
    </nav>

    <!-- Overlays -->
    <DcTestModal
      :show="showTestModal"
      :testing="isTestingConnectivity"
      :results="testResults"
      @close="showTestModal = false"
      @retry="runConnectivityTest"
    />

    <OnboardingWizard
      :show="showWizard"
      @close="showWizard = false"
    />

    <ToastHost />
  </div>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.16s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.mobile-tab {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.15rem;
  padding: 0.35rem 0.8rem;
  border-radius: 0.8rem;
  font-size: 0.6rem;
  font-weight: 500;
  color: var(--color-ink-3);
  cursor: pointer;
  transition: all 0.15s ease;
}

.mobile-tab:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.mobile-tab.active {
  color: #5bc5f5;
  background: rgba(42, 171, 238, 0.12);
}
</style>
