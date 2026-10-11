<script setup lang="ts">
import { Monitor, Smartphone, Apple, QrCode, GraduationCap } from 'lucide-vue-next'

const emit = defineEmits<{
  'go-main': []
  'rerun-onboarding': []
}>()

const guides = [
  {
    icon: Monitor,
    title: 'Telegram Desktop',
    steps: [
      'Откройте Настройки → Продвинутые настройки → Тип соединения.',
      'Нажмите «Добавить прокси» и выберите MTProto.',
      'Нажмите «Открыть в Telegram» на главном экране — клиент подхватит всё сам.',
      'Либо введите сервер, порт и секрет вручную из карточки ссылки.',
    ],
  },
  {
    icon: Smartphone,
    title: 'Telegram Android',
    steps: [
      'Откройте меню → Настройки → Данные и память → Прокси.',
      'Включите прокси и нажмите «Добавить прокси».',
      'Проще всего: на главном экране нажмите «QR-код» и наведите камеру телефона.',
      'Мост должен быть доступен по сети: host 0.0.0.0 вместо 127.0.0.1.',
    ],
  },
  {
    icon: Apple,
    title: 'Telegram iOS / macOS',
    steps: [
      'Откройте Настройки → Данные и память → Прокси → Добавить прокси.',
      'Выберите MTProto и введите сервер, порт и секрет.',
      'На macOS работает и кнопка «Открыть в Telegram».',
      'Для iOS используйте QR-код или ручной ввод.',
    ],
  },
]
</script>

<template>
  <div class="space-y-4 max-w-2xl mx-auto pb-4">
    <div class="bg-[#1c2027] border border-white/5 rounded-2xl p-4 space-y-3 shadow-md">
      <div class="flex items-center justify-between">
        <div class="text-[11px] font-bold text-white/60 uppercase tracking-wider flex items-center gap-2">
          <GraduationCap class="w-4 h-4 text-blue-400" />
          Как подключить Telegram
        </div>
        <button
          @click="emit('rerun-onboarding')"
          class="px-3 py-1.5 rounded-xl bg-[#202530] hover:bg-[#2a3040] text-xs text-white/80 transition border border-white/5 cursor-pointer"
        >
          Пройти обучение заново
        </button>
      </div>
      <p class="text-xs text-white/50 leading-relaxed">
        Мост слушает локальный порт и выглядит для сети как обычный HTTPS-трафик.
        Добавьте его как MTProto-прокси в любом клиенте Telegram — инструкция ниже.
      </p>
      <div class="flex items-center gap-2 bg-[#0e1117] p-3 rounded-xl border border-white/5">
        <QrCode class="w-4 h-4 text-emerald-400 shrink-0" />
        <p class="text-[11px] text-white/50">
          Быстрее всего — QR-код из карточки ссылки на главном экране.
        </p>
        <button
          @click="emit('go-main')"
          class="ml-auto shrink-0 px-3 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-medium transition cursor-pointer"
        >
          К ссылке
        </button>
      </div>
    </div>

    <div
      v-for="g in guides"
      :key="g.title"
      class="bg-[#1c2027] border border-white/5 rounded-2xl p-4 space-y-3 shadow-md"
    >
      <div class="flex items-center gap-2.5">
        <div class="w-8 h-8 rounded-xl bg-blue-500/10 border border-blue-500/20 flex items-center justify-center text-blue-400">
          <component :is="g.icon" class="w-4 h-4" />
        </div>
        <div class="text-sm font-bold text-white">{{ g.title }}</div>
      </div>
      <ol class="space-y-2">
        <li
          v-for="(s, i) in g.steps"
          :key="i"
          class="flex items-start gap-2.5 text-xs text-white/60 leading-relaxed"
        >
          <span class="mt-0.5 w-5 h-5 shrink-0 rounded-lg bg-white/5 border border-white/10 flex items-center justify-center text-[10px] font-mono text-white/70">{{ i + 1 }}</span>
          <span>{{ s }}</span>
        </li>
      </ol>
    </div>
  </div>
</template>
