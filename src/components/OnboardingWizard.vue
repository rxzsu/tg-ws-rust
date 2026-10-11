<script setup lang="ts">
import { ref } from 'vue'
import { X, ChevronLeft, ChevronRight, Rocket, KeyRound, Dices, Link2 } from 'lucide-vue-next'
import TgLinkCard from './TgLinkCard.vue'

const props = defineProps<{
  link: string
  initialPort: number
  initialSecret: string
}>()

const emit = defineEmits<{
  apply: [{ port: number; secret: string }]
  close: []
}>()

const step = ref(1)
const port = ref(props.initialPort)
const secret = ref(props.initialSecret)
const portError = ref('')

function generateSecret() {
  const arr = new Uint8Array(16)
  window.crypto.getRandomValues(arr)
  secret.value = Array.from(arr).map(b => b.toString(16).padStart(2, '0')).join('')
}

function goStep3() {
  const p = Number(port.value)
  if (!Number.isInteger(p) || p < 1 || p > 65535) {
    portError.value = 'Порт должен быть числом от 1 до 65535'
    return
  }
  portError.value = ''
  emit('apply', { port: p, secret: secret.value.trim() })
  step.value = 3
}
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4">
    <div class="bg-[#181c24] border border-white/10 rounded-2xl w-full max-w-lg shadow-2xl p-6 space-y-5">
      <!-- Progress -->
      <div class="flex items-center gap-2">
        <div
          v-for="n in 3"
          :key="n"
          :class="[
            'h-1 flex-1 rounded-full transition',
            n <= step ? 'bg-blue-500' : 'bg-white/10'
          ]"
        />
      </div>

      <!-- Step 1: Welcome -->
      <template v-if="step === 1">
        <div class="flex flex-col items-center text-center gap-3 pt-2">
          <div class="w-14 h-14 rounded-2xl bg-blue-500/15 border border-blue-500/30 flex items-center justify-center text-blue-400">
            <Rocket class="w-7 h-7" />
          </div>
          <h2 class="text-lg font-bold text-white">Добро пожаловать в TG WS Proxy!</h2>
          <p class="text-xs text-white/50 leading-relaxed max-w-sm">
            Это локальный MTProto-мост: Telegram подключается к нему, а он везёт
            трафик через WebSocket под видом обычного HTTPS. Три шага — и вы в сети.
          </p>
        </div>
        <div class="flex items-center justify-between pt-1">
          <button
            @click="emit('close')"
            class="text-xs text-white/40 hover:text-white/70 transition cursor-pointer"
          >
            Пропустить
          </button>
          <button
            @click="step = 2"
            class="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-semibold shadow-md transition cursor-pointer"
          >
            <span>Настроить</span>
            <ChevronRight class="w-3.5 h-3.5" />
          </button>
        </div>
      </template>

      <!-- Step 2: Port + Secret -->
      <template v-else-if="step === 2">
        <div class="space-y-1">
          <h2 class="text-base font-bold text-white flex items-center gap-2">
            <KeyRound class="w-4 h-4 text-blue-400" />
            Порт и секрет
          </h2>
          <p class="text-xs text-white/50">Порт — локальный адрес моста. Секрет — пароль вашего прокси, никому его не показывайте.</p>
        </div>

        <div class="space-y-3">
          <div>
            <label class="block text-[11px] text-white/40 mb-1">Локальный порт</label>
            <input
              type="number"
              v-model.number="port"
              class="w-full bg-[#12151a] border border-white/5 rounded-xl px-3 py-2 text-sm font-mono text-white outline-none focus:border-blue-500/50"
            />
            <p v-if="portError" class="text-[11px] text-rose-400 mt-1">{{ portError }}</p>
          </div>
          <div>
            <label class="block text-[11px] text-white/40 mb-1">Секрет (16 байт в hex)</label>
            <div class="flex gap-2">
              <input
                v-model="secret"
                class="flex-1 bg-[#12151a] border border-white/5 rounded-xl px-3 py-2 text-xs font-mono text-white outline-none focus:border-blue-500/50"
              />
              <button
                @click="generateSecret"
                class="px-3 py-2 rounded-xl bg-[#202530] hover:bg-[#2a3040] text-white/80 text-xs flex items-center gap-1.5 transition border border-white/5 cursor-pointer"
                title="Сгенерировать случайный секрет"
              >
                <Dices class="w-3.5 h-3.5" />
              </button>
            </div>
          </div>
        </div>

        <div class="flex items-center justify-between pt-1">
          <button
            @click="step = 1"
            class="flex items-center gap-1.5 px-3 py-2 rounded-xl text-xs text-white/60 hover:text-white hover:bg-white/5 transition cursor-pointer"
          >
            <ChevronLeft class="w-3.5 h-3.5" />
            <span>Назад</span>
          </button>
          <button
            @click="goStep3"
            class="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-semibold shadow-md transition cursor-pointer"
          >
            <span>Готово</span>
            <ChevronRight class="w-3.5 h-3.5" />
          </button>
        </div>
      </template>

      <!-- Step 3: Link -->
      <template v-else>
        <div class="space-y-1">
          <h2 class="text-base font-bold text-white flex items-center gap-2">
            <Link2 class="w-4 h-4 text-emerald-400" />
            Мост запущен!
          </h2>
          <p class="text-xs text-white/50">Настройки сохранены. Добавьте прокси в Telegram — ссылкой, QR-кодом или кнопкой.</p>
        </div>

        <TgLinkCard :link="link" />

        <div class="flex justify-end pt-1">
          <button
            @click="emit('close')"
            class="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-semibold shadow-md transition cursor-pointer"
          >
            <X class="w-3.5 h-3.5" />
            <span>К работе</span>
          </button>
        </div>
      </template>
    </div>
  </div>
</template>
