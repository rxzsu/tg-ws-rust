# Builds an installable Android APK with a consistent signing key.
#
#   .\scripts\build-android.ps1 -Debug                 # debug-signed, package .debug
#   .\scripts\build-android.ps1 -Release -GenerateKey  # first signed release build
#   .\scripts\build-android.ps1 -Release               # subsequent signed releases
#
# The keystore and its passwords live in %USERPROFILE%\.tgwsproxy\ and are
# never written into the repository. Back that directory up: Android only
# accepts an update if it is signed with the same key as the installed app.
param(
    [switch]$Debug,
    [switch]$Release,
    [switch]$GenerateKey
)

$ErrorActionPreference = 'Stop'

if (-not $Debug -and -not $Release) { $Release = $true }
if ($Debug -and $Release) { throw 'Choose either -Debug or -Release' }

$root = Split-Path -Parent $PSScriptRoot
$keyDir = Join-Path $env:USERPROFILE '.tgwsproxy'
$keystore = Join-Path $keyDir 'release.keystore'
$propsFile = Join-Path $keyDir 'keystore.properties'

# Tauri needs the NDK path when it is not exported globally.
if (-not $env:NDK_HOME -and -not $env:ANDROID_NDK_HOME) {
    $ndkRoot = Join-Path $env:LOCALAPPDATA 'Android\Sdk\ndk'
    if (Test-Path $ndkRoot) {
        $latestNdk = Get-ChildItem $ndkRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
        if ($latestNdk) {
            $env:NDK_HOME = $latestNdk.FullName
            $env:ANDROID_NDK_HOME = $latestNdk.FullName
        }
    }
}

if ($Release) {
    if (-not (Test-Path $keystore) -or -not (Test-Path $propsFile)) {
        if (-not $GenerateKey) {
            throw "No signing key found. Run: .\scripts\build-android.ps1 -Release -GenerateKey"
        }
        New-Item -ItemType Directory -Force $keyDir | Out-Null

        # Random password from a real CSPRNG. `Get-Random` in Windows
        # PowerShell 5.1 is seeded from a PRNG with ~2^31 of state, so a
        # password for a key that must stay valid for ~27 years needs
        # crypto-grade entropy instead.
        $alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789'
        $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
        try {
            $indexes = New-Object byte[] 40
            $rng.GetBytes($indexes)
            $password = -join ($indexes | ForEach-Object { $alphabet[$_ % $alphabet.Length] })
        } finally {
            $rng.Dispose()
        }

        # Only this user may read the keystore directory (default ACLs on a
        # shared machine leave it readable by other local processes).
        & icacls $keyDir /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" | Out-Null

        & keytool -genkeypair -v `
            -keystore $keystore `
            -alias tgwsproxy `
            -keyalg RSA -keysize 4096 -validity 10000 `
            -storetype PKCS12 `
            -storepass $password -keypass $password `
            -dname 'CN=TG WS Proxy, O=TG WS Proxy, C=US'
        if ($LASTEXITCODE -ne 0) { throw 'keytool failed to create the keystore' }

        @(
            "ANDROID_KEYSTORE_PATH=$keystore"
            "ANDROID_KEYSTORE_PASSWORD=$password"
            "ANDROID_KEY_ALIAS=tgwsproxy"
            "ANDROID_KEY_PASSWORD=$password"
        ) | Set-Content -Encoding utf8 $propsFile

        Write-Host ''
        Write-Host "Release keystore created: $keystore" -ForegroundColor Green
        Write-Host "Passwords saved to:       $propsFile" -ForegroundColor Green
        Write-Host 'Back this directory up. Losing it means future APKs cannot be installed as updates.' -ForegroundColor Yellow
        Write-Host ''
    }

    Get-Content $propsFile | ForEach-Object {
        if ($_ -match '^([^=]+)=(.*)$') {
            Set-Item -Path "env:$($Matches[1])" -Value $Matches[2]
        }
    }
}

$tauriArgs = @('tauri', 'android', 'build', '--apk', '--target', 'aarch64', '--ci')
if ($Debug) { $tauriArgs += '--debug' }

Push-Location $root
try {
    & bunx @tauriArgs
    if ($LASTEXITCODE -ne 0) { throw "tauri android build failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}

$apkRoot = Join-Path $root 'src-tauri\gen\android\app\build\outputs\apk'
Get-ChildItem $apkRoot -Recurse -Filter *.apk |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1 |
    ForEach-Object { Write-Host "`nAPK: $($_.FullName)" -ForegroundColor Cyan }
