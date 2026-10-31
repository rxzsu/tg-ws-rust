<script setup lang="ts">
import { ref, watch, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { Copy, Check, ExternalLink, QrCode, X } from "lucide-vue-next";
import BaseModal from "./ui/BaseModal.vue";
import { pushToast } from "../composables/useToast";

const props = defineProps<{
  link: string;
}>();

const copied = ref(false);
const showQr = ref(false);
const qrCanvas = ref<HTMLCanvasElement | null>(null);
const qrError = ref("");

async function copyLink() {
  if (!props.link) return;
  try {
    await navigator.clipboard.writeText(props.link);
  } catch (e) {
    // Clipboard can be blocked by webview policy; previously this left an
    // unhandled rejection and a button stuck without feedback.
    console.error("Clipboard write failed:", e);
    pushToast("Не удалось скопировать — выделите ссылку вручную", "error");
    return;
  }
  copied.value = true;
  setTimeout(() => {
    copied.value = false;
  }, 2000);
}

async function openTelegram() {
  if (!props.link) return;
  try {
    await invoke("open_url", { url: props.link });
  } catch (err) {
    console.error("Failed to open tg link via backend:", err);
    window.open(
      props.link.replace("tg://proxy?", "https://t.me/proxy?"),
      "_blank",
    );
  }
}

async function renderQr() {
  qrError.value = "";
  await nextTick();
  if (!qrCanvas.value || !props.link) return;
  try {
    // Loaded on demand: the library is only needed when the QR modal opens,
    // keeping it out of the startup bundle (noticeable on Android).
    const { default: QRCode } = await import("qrcode");
    await QRCode.toCanvas(qrCanvas.value, props.link, {
      width: 232,
      margin: 2,
      color: { dark: "#0e1117", light: "#ffffff" },
    });
  } catch (e) {
    console.error("QR render failed:", e);
    qrError.value = "Не удалось построить QR-код для этой ссылки";
  }
}

watch(showQr, (open) => {
  if (open) renderQr();
});
</script>

<template>
  <div
    class="glass relative flex h-full flex-col gap-3 overflow-hidden rounded-2xl p-4"
  >
    <!-- accent edge -->
    <div
      class="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-[#2aabee]/60 to-transparent"
    />

    <div>
      <h3 class="flex items-center gap-2 text-xs font-bold text-white">
        <span>Подключение</span>
        <span
          class="chip mono border-[#2aabee]/30 bg-[#2aabee]/15 px-1.5 py-0 text-[9px] text-[#5bc5f5]"
          >MTProto</span
        >
      </h3>
      <p class="mt-1 text-[11px] leading-relaxed text-ink-2">
        Откройте в Telegram или покажите QR-код телефону.
      </p>
    </div>

    <!-- Link -->
    <div class="glass-deep min-w-0 rounded-xl px-2.5 py-2">
      <input
        readonly
        :value="link"
        class="w-full cursor-default select-all truncate bg-transparent font-mono text-[10.5px] text-white/75 outline-none"
      />
    </div>

    <div class="mt-auto flex flex-col gap-2">
      <div class="grid grid-cols-2 gap-2">
        <button
          class="btn btn-ghost justify-center"
          :disabled="!link"
          title="Показать QR-код для телефона"
          @click="showQr = true"
        >
          <QrCode class="h-3.5 w-3.5" />
          <span>QR-код</span>
        </button>

        <button
          class="btn justify-center"
          :class="
            copied
              ? 'border-emerald-500/30 bg-emerald-500/20 text-emerald-300'
              : 'btn-ghost'
          "
          title="Скопировать ссылку"
          @click="copyLink"
        >
          <component
            :is="copied ? Check : Copy"
            class="h-3.5 w-3.5"
            :class="copied ? 'stroke-[3]' : ''"
          />
          <span>{{ copied ? "Готово!" : "Копировать" }}</span>
        </button>
      </div>

      <button
        class="btn btn-primary w-full justify-center"
        @click="openTelegram"
      >
        <ExternalLink class="h-3.5 w-3.5" />
        <span>Открыть в Telegram</span>
      </button>
    </div>
  </div>

  <!-- QR Modal -->
  <BaseModal :show="showQr" title="QR-код прокси" @close="showQr = false">
    <p class="text-[11px] leading-relaxed text-ink-2">
      Наведите камеру телефона — Telegram предложит добавить прокси.
    </p>
    <div
      class="flex min-h-60 items-center justify-center rounded-xl bg-white p-3"
    >
      <canvas ref="qrCanvas" />
    </div>
    <p v-if="qrError" class="text-[11px] text-err">{{ qrError }}</p>
    <p class="break-all font-mono text-[10px] leading-relaxed text-ink-3">
      {{ link }}
    </p>
    <button class="btn btn-primary w-full" @click="showQr = false">
      Закрыть
    </button>
  </BaseModal>
</template>
