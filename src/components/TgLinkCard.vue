<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import QRCode from 'qrcode'
import { Copy, Check, ExternalLink, QrCode, X } from 'lucide-vue-next'

const props = defineProps<{
  link: string
}>()

const copied = ref(false)
const showQr = ref(false)
const qrCanvas = ref<HTMLCanvasElement | null>(null)
const qrError = ref('')

async function copyLink() {
  if (!props.link) return
  await navigator.clipboard.writeText(props.link)
  copied.value = true
  setTimeout(() => {
    copied.value = false
  }, 2000)
}

async function openTelegram() {
  if (!props.link) return
  try {
    await invoke('open_url', { url: props.link })
  } catch (err) {
    console.error('Failed to open tg link via backend:', err)
    window.open(props.link.replace('tg://proxy?', 'https://t.me/proxy?'), '_blank')
  }
}

async function renderQr() {
  qrError.value = ''
  await nextTick()
  if (!qrCanvas.value || !props.link) return
  try {
    await QRCode.toCanvas(qrCanvas.value, props.link, {
      width: 232,
      margin: 2,
      color: { dark: '#0e1117', light: '#ffffff' },
    })
  } catch (e) {
    console.error('QR render failed:', e)
    qrError.value = 'Не удалось построить QR-код для этой ссылки'
  }
}

watch(showQr, (open) => {
  if (open) renderQr()
})
</script>

<template>
  <div class="bg-gradient-to-b from-[#1b2230] via-[#161b25] to-[#13161f] border border-blue-500/25 rounded-2xl p-5 space-y-4 shadow-xl shadow-blue-950/20">
    <div class="flex items-start justify-between">
      <div>
        <h3 class="text-sm font-bold text-white flex items-center gap-2">
          <span>Ссылка для подключения Telegram</span>
          <span class="px-2 py-0.5 rounded-full text-[10px] font-medium bg-blue-500/20 text-blue-400 border border-blue-500/30 font-mono">MTProto</span>
        </h3>
        <p class="text-xs text-white/50 mt-1">
          Нажмите «Открыть в Telegram» для мгновенного добавления прокси в клиент, покажите QR-код телефону или скопируйте ссылку.
        </p>
      </div>
    </div>

    <!-- Link Bar with Buttons -->
    <div class="flex items-center gap-2 bg-[#0e1117] p-2 rounded-xl border border-white/5 shadow-inner">
      <input
        readonly
        :value="link"
        class="bg-transparent flex-1 text-xs font-mono text-white/80 outline-none px-3 select-all truncate selection:bg-blue-600/40"
      />

      <!-- QR button -->
      <button
        @click="showQr = true"
        :disabled="!link"
        class="flex items-center gap-1.5 px-3 py-2 rounded-lg text-xs font-medium transition duration-200 active:scale-95 cursor-pointer bg-[#1c222e] hover:bg-[#252c3c] text-white/70 hover:text-white border border-white/5 disabled:opacity-40"
        title="Показать QR-код для телефона"
      >
        <QrCode class="w-3.5 h-3.5" />
        <span>QR-код</span>
      </button>

      <!-- Copy button -->
      <button
        @click="copyLink"
        :class="[
          'flex items-center gap-1.5 px-3 py-2 rounded-lg text-xs font-medium transition duration-200 active:scale-95 cursor-pointer',
          copied
            ? 'bg-emerald-500/20 text-emerald-300 border border-emerald-500/30'
            : 'bg-[#1c222e] hover:bg-[#252c3c] text-white/70 hover:text-white border border-white/5'
        ]"
        title="Скопировать ссылку"
      >
        <component :is="copied ? Check : Copy" class="w-3.5 h-3.5" :class="copied ? 'text-emerald-400 stroke-[3]' : ''" />
        <span>{{ copied ? 'Скопировано!' : 'Копировать' }}</span>
      </button>

      <!-- Open in Telegram button -->
      <button
        @click="openTelegram"
        class="flex items-center gap-2 px-4 py-2 rounded-lg bg-gradient-to-r from-blue-600 via-blue-500 to-indigo-600 hover:from-blue-500 hover:to-indigo-500 text-white text-xs font-semibold shadow-lg shadow-blue-600/30 hover:shadow-blue-500/40 active:scale-95 transition-all duration-200 cursor-pointer"
      >
        <ExternalLink class="w-3.5 h-3.5 text-white" />
        <span>Открыть в Telegram</span>
      </button>
    </div>
  </div>

  <!-- QR Modal -->
  <div
    v-if="showQr"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4"
    @click.self="showQr = false"
  >
    <div class="bg-[#181c24] border border-white/10 rounded-2xl shadow-2xl p-5 space-y-4 w-full max-w-xs">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-bold text-white">QR-код прокси</h3>
        <button @click="showQr = false" class="text-white/40 hover:text-white transition cursor-pointer">
          <X class="w-4 h-4" />
        </button>
      </div>
      <p class="text-[11px] text-white/50 leading-relaxed">Наведите камеру телефона — Telegram предложит добавить прокси.</p>
      <div class="bg-white rounded-xl p-3 flex items-center justify-center min-h-60">
        <canvas ref="qrCanvas" />
      </div>
      <p v-if="qrError" class="text-[11px] text-rose-400">{{ qrError }}</p>
      <p class="text-[10px] font-mono text-white/40 break-all leading-relaxed">{{ link }}</p>
      <button
        @click="showQr = false"
        class="w-full px-4 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-xs text-white font-medium shadow-md transition cursor-pointer"
      >
        Закрыть
      </button>
    </div>
  </div>
</template>
