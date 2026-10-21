<script setup lang="ts">
import { ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { X, ChevronLeft, ChevronRight, Rocket, KeyRound, Dices, ExternalLink } from 'lucide-vue-next'
import BaseModal from './ui/BaseModal.vue'
import TgLinkCard from './TgLinkCard.vue'
import { useProxy, saveSettings } from '../composables/useProxy'
import { formatInvokeError } from '../utils/format'

const props = defineProps<{
  show: boolean
}>()

const emit = defineEmits<{
  close: []
}>()

const { tgLink, config, isRunning } = useProxy()

const step = ref(1)
const port = ref(config.value.port)
const portError = ref('')
const saveError = ref('')
const saving = ref(false)

// Re-sync with the saved config every time the wizard opens: a hardcoded
// 1443 used to overwrite a port the user had already configured.
watch(
  () => props.show,
  (show) => {
    if (!show) return
    port.value = config.value.port
    portError.value = ''
    saveError.value = ''
  },
)

async function generateSecret() {
  const arr = new Uint8Array(16)
  window.crypto.getRandomValues(arr)
  const secret = Array.from(arr).map(b => b.toString(16).padStart(2, '0')).join('')
  config.value.secret = secret
  saveError.value = ''
  if (!(await saveSettings())) {
    saveError.value = 'Не удалось сохранить секрет. Проверьте права на config.json.'
  }
}

async function goStep3() {
  if (saving.value) return
  const p = Number(port.value)
  if (!Number.isInteger(p) || p < 1 || p > 65535) {
    portError.value = 'Порт должен быть числом от 1 до 65535'
    return
  }
  portError.value = ''
  saveError.value = ''
  saving.value = true
  try {
    // Don't let the user walk into a "port busy" error on the last screen.
    if (!isRunning.value) {
      try {
        await invoke('check_port', { host: config.value.host, port: p })
      } catch (e) {
        portError.value = formatInvokeError(e)
        return
      }
    }

    config.value.port = p
    if (!(await saveSettings())) {
      saveError.value = 'Настройки не сохранены — мост не запущен.'
      return
    }

    // Saving no longer starts a stopped bridge (that surprised users), but
    // onboarding explicitly ends with a running proxy.
    if (!isRunning.value) {
      try {
        await invoke<string>('start_proxy')
        isRunning.value = true
      } catch (e) {
        saveError.value = `Не удалось запустить мост: ${formatInvokeError(e)}`
        return
      }
    }

    step.value = 3
  } finally {
    saving.value = false
  }
}

async function openTelegram() {
  try {
    await invoke('open_url', { url: tgLink.value })
    emit('close')
  } catch (e) {
    console.error('Failed to open tg link:', e)
  }
}
</script>

<template>
  <BaseModal
    :show="show"
    :dismissible="false"
    wide
    @close="emit('close')"
  >
    <div class="flex items-start justify-between">
      <div class="space-y-1">
        <h2 class="flex items-center gap-2 text-base font-bold text-white">
          <Rocket class="h-4 w-4 text-[#5bc5f5]" />
          Добро пожаловать
        </h2>
        <p class="text-xs text-ink-2">Настройте мост за три шага — это займёт меньше минуты.</p>
      </div>
      <button class="text-ink-3 transition hover:text-white" @click="emit('close')">
        <X class="h-4 w-4" />
      </button>
    </div>

    <!-- Progress -->
    <div class="flex items-center gap-2">
      <span
        v-for="n in 3"
        :key="n"
        class="h-1 flex-1 rounded-full transition-all duration-300"
        :class="n <= step ? 'bg-[#2aabee] shadow-[0_0_8px_rgba(42,171,238,0.5)]' : 'bg-white/10'"
      />
    </div>

    <!-- Step 1: Intro -->
    <div v-if="step === 1" class="space-y-4 py-2">
      <p class="text-xs leading-relaxed text-ink-2">
        TG WS Proxy — локальный MTProto-мост, который маскирует трафик Telegram под обычный
        HTTPS и помогает обойти блокировки. Сейчас мы настроим порт и секрет.
      </p>
      <div class="glass-deep space-y-2 rounded-xl p-3.5 text-[11px] leading-relaxed text-ink-2">
        <p><span class="font-semibold text-white">Шаг 1.</span> Порт и секрет подключения</p>
        <p><span class="font-semibold text-white">Шаг 2.</span> Проверка настроек</p>
        <p><span class="font-semibold text-white">Шаг 3.</span> Подключение Telegram</p>
      </div>
    </div>

    <!-- Step 2: Port + Secret -->
    <div v-else-if="step === 2" class="space-y-4 py-2">
      <div>
        <label class="field-label">Порт моста</label>
        <input
          v-model.number="port"
          type="number"
          min="1"
          max="65535"
          class="input mono"
        />
        <p v-if="portError" class="mt-1.5 text-[11px] text-err">{{ portError }}</p>
        <p v-else class="mt-1 text-[10px] text-ink-3">Порт должен быть свободен и не занят другим приложением</p>
      </div>

      <div>
        <label class="field-label">Секрет MTProto</label>
        <div class="flex gap-2">
          <input
            :value="config.secret"
            readonly
            class="input mono flex-1"
          />
          <button class="btn btn-ghost" :disabled="saving" @click="generateSecret">
            <Dices class="h-3.5 w-3.5" />
            <span>Новый</span>
          </button>
        </div>
        <p class="mt-1 text-[10px] text-ink-3">Случайные 32 hex-символа — ваш ключ шифрования</p>
      </div>

      <p v-if="saveError" class="rounded-lg border border-rose-400/25 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-200/90">
        {{ saveError }}
      </p>
    </div>

    <!-- Step 3: Link -->
    <div v-else class="space-y-4 py-2">
      <p class="text-xs leading-relaxed text-ink-2">
        Готово! Мост настроен. Подключите Telegram — самое простое: нажмите
        «Открыть в Telegram» или отсканируйте QR-код с телефона.
      </p>
      <TgLinkCard :link="tgLink" />
    </div>

    <!-- Footer nav -->
    <div class="flex items-center justify-between border-t border-white/[0.07] pt-4">
      <button
        v-if="step > 1"
        class="btn btn-ghost"
        @click="step--"
      >
        <ChevronLeft class="h-3.5 w-3.5" />
        <span>Назад</span>
      </button>
      <div v-else />

      <div class="flex items-center gap-2">
        <button v-if="step < 3" class="btn btn-ghost" @click="emit('close')">
          Пропустить
        </button>
        <button
          v-if="step === 1"
          class="btn btn-primary"
          @click="step = 2"
        >
          <KeyRound class="h-3.5 w-3.5" />
          <span>Начать</span>
        </button>
        <button
          v-else-if="step === 2"
          class="btn btn-primary"
          :disabled="saving"
          @click="goStep3"
        >
          {{ saving ? 'Сохранение…' : 'Сохранить и открыть Telegram' }}
          <ChevronRight class="h-3.5 w-3.5" />
        </button>
        <button
          v-else
          class="btn btn-primary"
          @click="openTelegram"
        >
          <ExternalLink class="h-3.5 w-3.5" />
          <span>Готово</span>
        </button>
      </div>
    </div>
  </BaseModal>
</template>
