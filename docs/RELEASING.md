# Релизы и подпись

## Версия

Версия живёт в трёх местах и должна совпадать:

- `package.json` → `version`
- `src-tauri/tauri.conf.json` → `version`
- `src-tauri/Cargo.toml` → `[package].version`
- `src-tauri/core/Cargo.toml` → `[package].version`

Тег релиза — `v<version>`. Workflow `release.yml` вытаскивает тело релиза из
секции `## [<version>]` в `CHANGELOG.md`, поэтому секцию нужно добавить до
простановки тега.

## Android APK

### Локальная сборка с подписью

```powershell
# первый раз: создаёт ключ в %USERPROFILE%\.tgwsproxy\
.\scripts\build-android.ps1 -Release -GenerateKey

# дальше — тем же ключом
.\scripts\build-android.ps1 -Release

# быстрый debug-APK (пакет com.rxzsu.tg_ws_proxy.debug)
.\scripts\build-android.ps1 -Debug
```

Скрипт кладёт `release.keystore` и `keystore.properties` в
`%USERPROFILE%\.tgwsproxy\` (вне репозитория), выставляет переменные
`ANDROID_KEYSTORE_*` и запускает `tauri android build`. Готовый файл —
`src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk`.

**Важно:** Android принимает обновление только если новый APK подписан тем же
ключом, что и установленный. Если приложение уже стояло с другой подписью,
удалите его один раз (данные приложения при этом потеряются), затем ставьте
подписанный APK. Ключ из `%USERPROFILE%\.tgwsproxy\` нельзя терять — без него
следующие версии не установятся поверх.

Проверить подпись:

```powershell
& "$env:LOCALAPPDATA\Android\Sdk\build-tools\36.0.0\apksigner.bat" `
  verify --print-certs `
  src-tauri\gen\android\app\build\outputs\apk\universal\release\app-universal-release.apk
```

### CI

CI умеет собирать **подписанный** APK. Добавьте в Settings → Secrets and
variables → Actions четыре секрета:

| Секрет | Что это |
| :--- | :--- |
| `ANDROID_KEYSTORE_BASE64` | `base64 -w0 release.keystore` |
| `ANDROID_KEYSTORE_PASSWORD` | пароль хранилища |
| `ANDROID_KEY_ALIAS` | alias ключа |
| `ANDROID_KEY_PASSWORD` | пароль ключа |

Создание ключа (один раз, храните `.jks` и пароли вне репозитория):

```bash
keytool -genkeypair -v \
  -keystore release.keystore \
  -alias tgwsproxy \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -storetype JKS
base64 -w0 release.keystore
# Windows (PowerShell):
# [Convert]::ToBase64String([IO.File]::ReadAllBytes("release.keystore"))
```

`src-tauri/gen/android/app/build.gradle.kts` читает `ANDROID_KEYSTORE_PATH` и
подписывает release-сборку, только если файл существует. Без секретов сборка
не падает, но APK будет unsigned — его нельзя установить поверх подписанной
прошлой версии.

## Desktop (Windows/macOS)

`tauri-action` подписывает установщики, если заданы соответствующие секреты:

- Windows: сертификат Code Signing + `TAURI_SIGNING_PRIVATE_KEY` (для
  updater-артефактов), либо `bundle.windows.certificateThumbprint`.
- macOS: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` для
  нотаризации.

Пока секретов нет, установщики собираются unsigned и SmartScreen/Gatekeeper
могут показать предупреждение.

## Автообновление: текущий статус

Сейчас в приложении есть **только проверка** обновлений: команда
`check_updates` запрашивает последний GitHub Release, сравнивает версии
численно и показывает баннер со ссылкой на страницу релиза. Скачивания и
установки внутри приложения нет.

Полноценный `tauri-plugin-updater` сознательно не подключён: ему нужен
публичный ключ в `tauri.conf.json` и подписанные `.sig`-артефакты, а приватный
ключ должен лежать только в GitHub Secrets. Подключать плагин с фиктивным
ключом бессмысленно — обновления будут отвергнуты.

Если нужно включить автообновление:

1. `bunx tauri signer generate -w ~/.tauri/tgwsproxy.key` — получить пару ключей.
2. Публичный ключ положить в `tauri.conf.json` → `plugins.updater.pubkey`,
   эндпоинт — в `plugins.updater.endpoints`.
3. Приватный ключ и пароль — в секреты `TAURI_SIGNING_PRIVATE_KEY` /
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Добавить `tauri-plugin-updater` в Cargo/npm и заменить кнопку «Проверить»
   на вызов `check()`/`downloadAndInstall()`.
5. В `release.yml` включить `updaterJsonPreferNsis`/`includeUpdaterJson`
   (tauri-action сам сгенерирует `latest.json` и `.sig`).
