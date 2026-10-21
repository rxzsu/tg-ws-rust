<script setup lang="ts">
import { computed, ref } from 'vue'
import { RefreshCw, Wifi, Zap, Scale, EyeOff, Cable, Globe, ShieldCheck, Cloud, Gauge, AppWindow } from 'lucide-vue-next'
import SectionCard from '../components/ui/SectionCard.vue'
import ToggleRow from '../components/ui/ToggleRow.vue'
import NumberField from '../components/ui/NumberField.vue'
import SelectField, { type SelectOption } from '../components/ui/SelectField.vue'
import {
  useProxy,
  splitList,
  parseDcRedirects,
  dcRedirectsToText,
  PRESETS,
  type PresetId,
} from '../composables/useProxy'
import { invoke } from '@tauri-apps/api/core'
// Actions live at module scope, not on the useProxy() return value.
import { toggleAutostart, applyPreset, saveSettings, generateSecret, checkForUpdates, runConnectivityTest } from '../composables/useProxy'

const {
  config,
  appVersion,
  autostartActive,
  activePreset,
  isMobile,
  isCheckingUpdate,
  isTestingConnectivity,
} = useProxy()

const showAdvanced = ref(false)

const transportOptions: SelectOption[] = [
  {
    value: 'auto',
    label: 'Авто (WS → QUIC/H3 → CF → TCP)',
    desc: 'Пробуем цепочку сверху вниз, продолжаем до первого успеха',
  },
  { value: 'ws', label: 'Только WebSocket', desc: 'Классический путь через домены Telegram CDN' },
  { value: 'quic', label: 'QUIC в первую очередь', desc: 'Кастомный ALPN, UDP-релеи из списка' },
  { value: 'h3', label: 'HTTP/3 в первую очередь', desc: 'POST /mtproto по QUIC — любой h3-сервер' },
]

const transportHint = computed(
  () => transportOptions.find((o) => o.value === config.value.transport)?.desc ?? '',
)

// List inputs as live computed <-> config bindings
const userDomainsInput = computed({
  get: () => (config.value.cfproxy_user_domains ?? []).join(', '),
  set: (v: string) => { config.value.cfproxy_user_domains = splitList(v) },
})

const workerDomainsInput = computed({
  get: () => (config.value.cfproxy_worker_domains ?? []).join(', '),
  set: (v: string) => { config.value.cfproxy_worker_domains = splitList(v) },
})

const quicRelaysInput = computed({
  get: () => (config.value.quic_relays ?? []).join(', '),
  set: (v: string) => { config.value.quic_relays = splitList(v) },
})

const dcRedirectsInput = computed({
  get: () => dcRedirectsToText(config.value.dc_redirects),
  set: (v: string) => { config.value.dc_redirects = parseDcRedirects(v) },
})

const sections = [
  { id: 'sec-presets', label: 'Пресеты' },
  { id: 'sec-connection', label: 'Подключение' },
  { id: 'sec-transport', label: 'Транспорт' },
  { id: 'sec-masking', label: 'Маскировка' },
  { id: 'sec-cloudflare', label: 'Cloudflare' },
  { id: 'sec-perf', label: 'Производительность' },
  { id: 'sec-app', label: 'Приложение' },
]

function scrollToSection(id: string) {
  document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}
</script>

<template>
  <div class="space-y-4">
    <!-- Section quick nav -->
    <div class="glass-strong flex flex-wrap gap-1.5 rounded-2xl p-2">
      <button
        v-for="s in sections"
        :key="s.id"
        class="section-chip"
        @click="scrollToSection(s.id)"
      >
        {{ s.label }}
      </button>
    </div>

    <!-- 0. Presets -->
    <SectionCard
      id="sec-presets"
      title="Пресеты"
      subtitle="Подставляют значения в поля ниже — затем нажмите «Сохранить»"
      :icon="Zap"
    >
      <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
        <button
          v-for="(p, id) in PRESETS"
          :key="id"
          class="preset-tile"
          :class="{ active: activePreset === id }"
          @click="applyPreset(id as PresetId)"
        >
          <component :is="p.icon" class="mb-1.5 h-4 w-4 text-[#5bc5f5] transition group-hover:scale-110" />
          <div class="text-xs font-semibold text-white">{{ p.name }}</div>
          <div class="mt-0.5 text-[10px] leading-snug text-ink-3">{{ p.desc }}</div>
        </button>
      </div>
    </SectionCard>

    <!-- 1. Connection -->
    <SectionCard
      id="sec-connection"
      title="Подключение"
      subtitle="Адрес, порт и секрет MTProto"
      :icon="Cable"
    >
      <div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <div class="sm:col-span-2">
          <label class="field-label">Хост</label>
          <input v-model="config.host" class="input mono" />
        </div>
        <div>
          <label class="field-label">Порт</label>
          <NumberField v-model="config.port" :min="1" :max="65535" />
        </div>
      </div>

      <div>
        <label class="field-label">Секрет (16 байт в hex)</label>
        <div class="flex gap-2">
          <input v-model="config.secret" class="input mono flex-1" />
          <button class="btn btn-ghost" title="Сгенерировать случайный секрет" @click="generateSecret">
            <RefreshCw class="h-3 w-3" />
            <span>Случайный</span>
          </button>
        </div>
      </div>

      <div>
        <label class="field-label">Целевые IP дата-центров (DC:IP)</label>
        <input v-model="dcRedirectsInput" placeholder="2: 149.154.167.220, 4: 149.154.167.220" class="input mono" />
        <p class="mt-1 text-[10px] text-ink-3">Прямые IP-адреса для маршрутизации трафика к серверам Telegram</p>
      </div>

      <ToggleRow
        v-model="config.force_test_dc"
        title="Тестовые дата-центры"
        desc="Использовать Test DC — только для разработки и отладки"
      />
    </SectionCard>

    <!-- 2. Transport -->
    <SectionCard
      id="sec-transport"
      title="Транспорт"
      subtitle="QUIC и HTTP/3 — экспериментально"
      :icon="Globe"
    >
      <div>
        <label class="field-label">Режим апстрима</label>
        <SelectField v-model="config.transport" :options="transportOptions" />
        <p class="mt-1 text-[10px] leading-relaxed text-ink-3">{{ transportHint }}</p>
      </div>

      <div v-if="config.transport !== 'ws'">
        <label class="field-label">Релеи (host:port, через запятую)</label>
        <input v-model="quicRelaysInput" placeholder="relay.example.com:443" class="input mono" />
        <p class="mt-1 text-[10px] leading-relaxed text-ink-3">
          Свои UDP-релеи. QUIC — ALPN <span class="font-mono">tg-ws-proxy-quic-v1</span>; HTTP/3 — обычный
          <span class="font-mono">POST /mtproto</span> (подойдёт nginx, Caddy или Worker). Без релеев поведение прежнее.
        </p>
      </div>
    </SectionCard>

    <!-- 3. Masking -->
    <SectionCard
      id="sec-masking"
      title="Маскировка"
      subtitle="Fake TLS и обход DPI"
      :icon="ShieldCheck"
    >
      <div>
        <label class="field-label">SNI маскирующий домен (DPI bypass)</label>
        <input v-model="config.fake_tls_domain" placeholder="например, cloudflare.com, yandex.ru" class="input mono" />
      </div>

      <ToggleRow
        v-model="config.proxy_protocol"
        title="PROXY Protocol v1"
        desc="Передача заголовка с реальным адресом клиента"
      />

      <ToggleRow
        v-model="config.splitter_enabled"
        title="Фрагментация пакетов (Splitter)"
        desc="Разбивка первых пакетов для обхода DPI; выключение ускоряет, но упрощает детект"
      />
    </SectionCard>

    <!-- 4. Cloudflare -->
    <SectionCard
      id="sec-cloudflare"
      title="Cloudflare"
      subtitle="Фолбэк, воркеры и пользовательские домены"
      :icon="Cloud"
    >
      <div class="space-y-2.5">
        <ToggleRow
          v-model="config.fallback_cfproxy"
          title="Cloudflare Fallback"
          desc="Обход блокировок через сеть Cloudflare при сбоях прямого соединения"
        />

        <ToggleRow
          v-model="config.use_builtin_cf_domains"
          title="Встроенные сторонние CF-домены"
          desc="Пул чужих Cloudflare-доменов (и их обновление с GitHub). Трафик идёт через них — отключите, если нужен только ваш домен/воркер"
        />

        <div class="space-y-2.5">
          <ToggleRow
            v-model="config.cfproxy_user_domain_enabled"
            title="Собственные Cloudflare домены"
            desc="Маршрутизация трафика через пользовательские домены"
          />
          <input
            v-if="config.cfproxy_user_domain_enabled"
            v-model="userDomainsInput"
            placeholder="proxy.example.com, cdn.mysite.org"
            class="input"
          />
        </div>

        <div class="space-y-2.5">
          <ToggleRow
            v-model="config.cfproxy_worker_enabled"
            title="Cloudflare Worker воркеры"
            desc="Использование бессерверных воркеров Cloudflare Workers"
          />
          <input
            v-if="config.cfproxy_worker_enabled"
            v-model="workerDomainsInput"
            placeholder="my-worker.subdomain.workers.dev"
            class="input"
          />
        </div>

        <ToggleRow
          v-model="config.disable_secure"
          title="Режим No-Secure"
          desc="Порт 80 без SSL-шифрования для CF proxy/worker"
        />
      </div>
    </SectionCard>

    <!-- 5. Performance -->
    <SectionCard
      id="sec-perf"
      title="Производительность"
      subtitle="Буферы, пул соединений и логирование"
      :icon="Gauge"
    >
      <div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <div>
          <label class="field-label">Буфер сокета (байт)</label>
          <NumberField v-model="config.buffer_size" :min="4096" :max="1048576" :step="65536" />
        </div>
        <div>
          <label class="field-label">Размер пула сокетов</label>
          <NumberField v-model="config.pool_size" :min="0" :max="64" />
        </div>
        <div>
          <label class="field-label">Лог-файл (МБ)</label>
          <NumberField v-model="config.log_max_mb" :min="1" :max="500" />
        </div>
      </div>

      <button
        class="flex w-full cursor-pointer items-center justify-between text-[11px] font-bold uppercase tracking-wider text-ink-2 transition hover:text-ink-1"
        @click="showAdvanced = !showAdvanced"
      >
        <span>Продвинутые</span>
        <span>{{ showAdvanced ? '−' : '+' }}</span>
      </button>

      <div v-if="showAdvanced" class="grid grid-cols-2 gap-3">
        <div>
          <label class="field-label">TTL сокета в пуле (сек)</label>
          <NumberField v-model="config.pool_max_age_secs" :min="10" :max="3600" :step="10" />
        </div>
        <div>
          <label class="field-label">Keep-alive пула (сек, 0 — выкл)</label>
          <NumberField v-model="config.pool_keepalive_secs" :min="0" :max="3600" :step="5" />
        </div>
        <div>
          <label class="field-label">Таймаут апстрима (сек)</label>
          <NumberField v-model="config.connect_timeout_secs" :min="2" :max="120" />
        </div>
      </div>

      <ToggleRow
        v-model="config.verbose"
        title="Подробные логи (Verbose)"
        desc="Расширенная отладочная информация в журнале"
      />
    </SectionCard>

    <!-- 6. App -->
    <SectionCard
      id="sec-app"
      title="Приложение"
      subtitle="Автозапуск, обновления и диагностика"
      :icon="AppWindow"
    >
      <ToggleRow
        v-if="!isMobile"
        :model-value="autostartActive"
        title="Автозапуск с системой"
        desc="Запускать мост в фоновом режиме при старте ОС"
        @update:model-value="toggleAutostart"
      />

      <div class="toggle-row">
        <div class="min-w-0 pr-3">
          <div class="text-xs font-semibold text-ink-1">Обновления программы</div>
          <div class="mt-0.5 text-[11px] text-ink-2">
            Текущая версия <span v-if="appVersion" class="font-mono">v{{ appVersion }}</span>
          </div>
        </div>
        <button
          class="btn btn-ghost"
          :disabled="isCheckingUpdate"
          @click="checkForUpdates(true)"
        >
          <RefreshCw class="h-3.5 w-3.5 text-[#5bc5f5]" :class="isCheckingUpdate ? 'animate-spin' : ''" />
          <span>{{ isCheckingUpdate ? 'Проверка…' : 'Проверить' }}</span>
        </button>
      </div>

      <div class="toggle-row">
        <div class="min-w-0 pr-3">
          <div class="text-xs font-semibold text-ink-1">Тест связности дата-центров</div>
          <div class="mt-0.5 text-[11px] text-ink-2">Проверить WS-фронты DC2/DC4 и пользовательские домены</div>
        </div>
        <button
          class="btn btn-ghost"
          :disabled="isTestingConnectivity"
          @click="runConnectivityTest"
        >
          <Wifi class="h-3.5 w-3.5 text-emerald-400" />
          <span>{{ isTestingConnectivity ? 'Тестирование…' : 'Запустить' }}</span>
        </button>
      </div>
    </SectionCard>

    <!-- Save bar -->
    <div class="pb-1 pt-2">
      <div class="glass-strong flex items-center justify-between gap-4 rounded-2xl px-4 py-3">
        <p class="hidden text-[11px] leading-snug text-ink-2 sm:block">
          Настройки сохраняются в config.json — активный мост перезапускается автоматически
        </p>
        <button class="btn btn-primary ml-auto" @click="saveSettings">
          Сохранить настройки
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.section-chip {
  padding: 0.35rem 0.75rem;
  border-radius: 999px;
  font-size: 0.68rem;
  font-weight: 600;
  color: var(--color-ink-2);
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.07);
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.section-chip:hover {
  color: #fff;
  background: rgba(42, 171, 238, 0.14);
  border-color: rgba(42, 171, 238, 0.3);
}

.preset-tile {
  text-align: left;
  padding: 0.8rem;
  border-radius: 0.9rem;
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid rgba(255, 255, 255, 0.06);
  cursor: pointer;
  transition: all 0.18s ease;
}

.preset-tile:hover {
  border-color: rgba(42, 171, 238, 0.4);
  background: rgba(42, 171, 238, 0.07);
}

.preset-tile.active {
  border-color: rgba(42, 171, 238, 0.55);
  background: rgba(42, 171, 238, 0.1);
  box-shadow: 0 0 16px rgba(42, 171, 238, 0.15);
}
</style>
