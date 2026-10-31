<script setup lang="ts">
import { computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  Settings2,
  KeyRound,
  Globe,
  MonitorPlay,
  LifeBuoy,
  Bug,
  ExternalLink,
  Gauge,
  ShieldAlert,
  Rocket,
} from "lucide-vue-next";
import { useProxy } from "../composables/useProxy";
import { pushToast } from "../composables/useToast";
import { formatInvokeError } from "../utils/format";

const { config } = useProxy();

interface Guide {
  icon: typeof Settings2;
  title: string;
  steps: string[];
}

const guides = computed<Guide[]>(() => [
  {
    icon: Rocket,
    title: "Быстрый старт",
    steps: [
      "Откройте раздел «Настройки».",
      "В блоке «Подключение» задайте хост и порт (по умолчанию 127.0.0.1:1443).",
      "Нажмите «Случайный» рядом с полем «Секрет».",
      "Нажмите «Сохранить настройки».",
      "На «Дашборде» нажмите круглую кнопку питания — статус станет «Мост активен».",
      "На «Дашборде» нажмите «Открыть в Telegram» или отсканируйте QR-код.",
    ],
  },
  {
    icon: KeyRound,
    title: "Что такое MTProto Secret",
    steps: [
      "Secret — это 32 hex-символа (16 случайных байт).",
      "Клиент и сервер используют его как «пароль» шифрования MTProto.",
      "Секрет виден только в ссылке подключения — никому его не показывайте.",
      "Кнопка «Случайный» генерирует криптостойкий секрет через CryptoAPI.",
      "Если хотите фиксированный секрет — впишите свои 32 hex-символа.",
    ],
  },
  {
    icon: ShieldAlert,
    title: "Fake TLS / SNI (обход DPI)",
    steps: [
      "DPI часто детектит MTProto-пакеты по сигнатуре.",
      `Поле «SNI маскирующий домен» сейчас: ${config.fake_tls_domain || "(пусто)"}.`,
      "Укажите домен вроде cloudflare.com, yandex.ru, google.com — трафик будет замаскирован под TLS-рукопожатие к этому сайту.",
      "В ссылке появится префикс ee — клиент поймёт, что включён Fake TLS.",
      "Без SNI секрет помечается dd (обычный MTProto).",
    ],
  },
  {
    icon: Globe,
    title: "Cloudflare фолбэк",
    steps: [
      "Если прямое соединение к Telegram заблокировано, трафик идёт через сеть Cloudflare.",
      "Cloudflare Fallback включён по умолчанию — он восстанавливает связь при жёстких блокировках.",
      "Собственные Cloudflare-домены позволят маршрутизировать через ваш домен, привязанный к CF.",
      "Cloudflare Workers — бессерверный режим: задеплойте воркер и укажите его адрес.",
      "Режим No-Secure снижает задержку (порт 80, без SSL), но менее приватен.",
    ],
  },
  {
    icon: Gauge,
    title: "Производительность",
    steps: [
      "Буфер сокета — сколько байт читается за раз; больше = быстрее на быстрых каналах.",
      "Размер пула сокетов — сколько TCP-соединений держится наготове.",
      "TTL сокета в пуле — время жизни соединения; короткий TTL свежее, но дороже.",
      "Таймаут апстрима — сколько ждать ответа от Telegram перед фолбэком.",
      "Фрагментация (Splitter) защищает от DPI, но чуть замедляет старт соединения.",
    ],
  },
  {
    icon: MonitorPlay,
    title: "QUIC-транспорт",
    steps: [
      "Экспериментально. В «Настройках» выберите режим апстрима «QUIC в первую очередь».",
      "Укажите свои QUIC-релеи (host:port) с ALPN tg-ws-proxy-quic-v1.",
      "При недоступности релея происходит тихий фолбэк на WS → CF → TCP.",
      "Не указали релеи — поведение прежнее (только WS/CF/TCP).",
    ],
  },
  {
    icon: Bug,
    title: "Известные ограничения",
    steps: [
      "На слабых сетях при «Макс. скорости» возможны разрывы — попробуйте пресет «Баланс».",
      "Виртуальные машины и некоторые антивирусы могут мешать перехвату локального порта.",
      "Тестовые DC (force_test_dc) — только для разработки, не включайте в проде.",
    ],
  },
  {
    icon: LifeBuoy,
    title: "Если ничего не помогает",
    steps: [
      "Откройте раздел «Логи» и нажмите «Открыть в Блокноте».",
      "Приватные домены в логе уже скрыты (***).",
      "Проверьте порт брандмауэром: netstat -ano | findstr 1443.",
      "Смените порт на 443 или 8443 — они реже блокируются.",
      "Создайте Issue на GitHub с прикреплённым логом.",
    ],
  },
]);

const issuesUrl = "https://github.com/rxzsu/tg-ws-rust/issues";
const repoUrl = "https://github.com/rxzsu/tg-ws-rust";

async function openIssues() {
  try {
    await invoke("open_url", { url: issuesUrl });
  } catch (e) {
    console.error("Failed to open issues page:", e);
    pushToast(formatInvokeError(e), "error");
  }
}
</script>

<template>
  <div class="space-y-4">
    <!-- Hero -->
    <section class="glass relative overflow-hidden rounded-2xl p-6">
      <div
        class="pointer-events-none absolute -right-20 -top-20 h-56 w-56 rounded-full bg-sky-500/10 blur-3xl"
      />
      <div class="relative flex flex-wrap items-center gap-4">
        <div class="icon-chip h-12 w-12">
          <LifeBuoy class="h-5 w-5" />
        </div>
        <div class="min-w-0">
          <h1 class="text-xl font-bold tracking-tight text-white">
            Центр помощи
          </h1>
          <p class="mt-1 max-w-lg text-xs leading-relaxed text-ink-2">
            Пошаговые инструкции по каждому блоку настроек: от быстрого старта
            до QUIC-транспорта и отладки.
          </p>
        </div>
        <a
          :href="issuesUrl"
          class="btn btn-ghost ml-auto"
          @click.prevent="openIssues"
        >
          <ExternalLink class="h-3.5 w-3.5" />
          <span>GitHub Issues</span>
        </a>
      </div>
    </section>

    <!-- Guides grid -->
    <div class="grid grid-cols-1 gap-3 xl:grid-cols-2">
      <section
        v-for="g in guides"
        :key="g.title"
        class="glass space-y-3.5 rounded-2xl p-5 transition duration-200 hover:border-white/[0.16]"
      >
        <header class="flex items-center gap-2.5">
          <div class="icon-chip">
            <component :is="g.icon" class="h-4 w-4" />
          </div>
          <h3 class="text-sm font-bold text-white">{{ g.title }}</h3>
        </header>

        <ol class="space-y-2.5">
          <li
            v-for="(step, i) in g.steps"
            :key="i"
            class="flex gap-2.5 text-xs leading-relaxed text-ink-2"
          >
            <span class="step-num">{{ i + 1 }}</span>
            <span>{{ step }}</span>
          </li>
        </ol>
      </section>
    </div>
  </div>
</template>

<style scoped>
.step-num {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 1.2rem;
  height: 1.2rem;
  margin-top: 0.05rem;
  flex-shrink: 0;
  border-radius: 999px;
  font-size: 0.6rem;
  font-weight: 700;
  background: rgba(42, 171, 238, 0.12);
  border: 1px solid rgba(42, 171, 238, 0.25);
  color: #5bc5f5;
}
</style>
