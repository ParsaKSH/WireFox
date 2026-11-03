@echo off
:: WireFox exit-node installer for Windows (x64).
::
:: Installs wirefox-node in managed mode as a scheduled task that runs as
:: SYSTEM at startup and restarts it if it stops. It connects to your
:: wirefox-panel with the node key the panel generated.
::
::   install-node.bat                  asks for the key
::   install-node.bat --key KEY        (the key ends up in this window's history)
::   install-node.bat --version TAG --mirror URL
::   install-node.bat --uninstall
::
:: Run it again to upgrade; the key is kept unless a new one is given.

setlocal
net session >nul 2>&1
if errorlevel 1 (
    echo Asking for administrator rights...
    if "%~1"=="" (
        powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    ) else (
        powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -ArgumentList '%*' -Verb RunAs"
    )
    exit /b
)

set "WFX_SELF=%~f0"
set "WFX_ARGS=%*"
powershell -NoProfile -ExecutionPolicy Bypass -Command "$s = [IO.File]::ReadAllText($env:WFX_SELF); iex $s.Substring($s.IndexOf('#' + '<PS>'))"
set "RC=%ERRORLEVEL%"
echo.
pause
exit /b %RC%

#<PS>
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'ParsaKSH/WireFox'
$InstallDir = 'C:\WireFox'
$Exe = Join-Path $InstallDir 'wirefox-node.exe'
$KeyFile = Join-Path $InstallDir 'node.key'
$StateDir = Join-Path $env:ProgramData 'wirefox-node'
$TaskName = 'WireFox node'

function Info($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Ok($m) { Write-Host "OK  $m" -ForegroundColor Green }
function Warn($m) { Write-Host "!   $m" -ForegroundColor Yellow }
function Fail($m) { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }

Write-Host ''
Write-Host '  WireFox Exit-Node Installer (Windows)' -ForegroundColor Cyan
Write-Host ''

$key = $env:WIREFOX_NODE_KEY
$version = ''
$mirror = $env:WIREFOX_MIRROR
$uninstall = $false
$argv = @(([string]$env:WFX_ARGS).Split(' ', [StringSplitOptions]::RemoveEmptyEntries))
for ($i = 0; $i -lt $argv.Count; $i++) {
    switch ($argv[$i]) {
        '--key' { $key = $argv[++$i] }
        '--version' { $version = $argv[++$i] }
        '--mirror' { $mirror = $argv[++$i] }
        '--uninstall' { $uninstall = $true }
        default { Fail "unknown option: $($argv[$i])" }
    }
}

function Stop-Node {
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    }
    Get-Process wirefox-node -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 1
}

if ($uninstall) {
    Info 'Removing wirefox-node'
    Stop-Node
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $InstallDir, $StateDir -ErrorAction SilentlyContinue
    Ok 'wirefox-node removed. Delete the node in the panel too.'
    exit 0
}

$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne 'AMD64') { Fail "Only 64-bit x86 Windows is supported (this is $arch)." }

if (-not $key -and (Test-Path $KeyFile)) {
    $key = (Get-Content -Raw $KeyFile).Trim()
    Ok 'Keeping the existing node key'
}
if (-not $key) {
    $secure = Read-Host -AsSecureString 'Paste the node key from the panel (Nodes > Add node)'
    $key = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
}
$key = $key.Trim()
if (-not $key.StartsWith('wfx1.')) { Fail "That doesn't look like a node key (they start with wfx1.)." }

# The key says where the panel is; used below to check the node reached it.
$panel = $null
try {
    $b64 = $key.Substring(5).Replace('-', '+').Replace('_', '/')
    $b64 = $b64.PadRight($b64.Length + (4 - $b64.Length % 4) % 4, '=')
    $panel = ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b64)) | ConvertFrom-Json).panel
} catch {
    Fail 'The node key is damaged; copy it again from the panel.'
}

if (-not $version) {
    if ($mirror) { Fail '--mirror needs --version.' }
    Info 'Looking up the latest release'
    try {
        $version = (Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest").tag_name
    } catch {
        Fail 'Could not find the latest release (is GitHub reachable? try --mirror).'
    }
}
Ok "Version $version"

$base = if ($mirror) { $mirror.TrimEnd('/') } else { "https://github.com/$Repo/releases/download" }
$base = "$base/$version"
$pkg = "wirefox-$version-windows-amd64"
$tmp = Join-Path $env:TEMP ('wirefox-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Info "Downloading $pkg"
    Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS"
    Invoke-WebRequest -UseBasicParsing "$base/$pkg.zip" -OutFile "$tmp\$pkg.zip"
    $line = Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match " $([regex]::Escape("$pkg.zip"))$" }
    $expected = if ($line) { ($line -split '\s+')[0] } else { '' }
    $actual = (Get-FileHash -Algorithm SHA256 "$tmp\$pkg.zip").Hash
    if (-not $expected -or $expected -ne $actual.ToLower()) { Fail 'Checksum mismatch; the download is corrupt or was tampered with.' }
    Ok 'Checksum verified'
    Expand-Archive -Path "$tmp\$pkg.zip" -DestinationPath $tmp -Force

    Stop-Node
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    # Only SYSTEM and Administrators may touch the folder: it holds the key.
    & icacls $InstallDir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "Could not restrict access to $InstallDir." }
    Copy-Item -Force "$tmp\$pkg\wirefox-node.exe" $Exe
    Unblock-File $Exe
    Ok "Installed $Exe"
} catch {
    Fail $_.Exception.Message
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Set-Content -Path $KeyFile -Value $key -NoNewline -Encoding ASCII
# Older installs kept the key in a machine environment variable, which every local user can read.
[Environment]::SetEnvironmentVariable('WIREFOX_NODE_KEY', $null, 'Machine')

Info 'Registering the startup task'
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
$action = New-ScheduledTaskAction -Execute $Exe -Argument "run --key-file `"$KeyFile`"" -WorkingDirectory $InstallDir
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 `
    -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal | Out-Null
Start-ScheduledTask -TaskName $TaskName
Ok "Task '$TaskName' started (runs as SYSTEM at every boot)"

# Two separate links: the agent connection to the panel, and the tunnel to the
# WireFox server, whose address the node saves once the panel sends it.
Info 'Waiting for the node to reach the panel and the WireFox server'
$panelPort = [int]($panel -replace '^.*:', '')
$tunnelFile = Join-Path $StateDir 'tunnel.json'
$panelUp = $false
$tunnelUp = $false
for ($i = 0; $i -lt 40 -and -not ($panelUp -and $tunnelUp); $i++) {
    Start-Sleep -Seconds 1
    $proc = Get-Process wirefox-node -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $proc) { continue }
    $conns = @(Get-NetTCPConnection -OwningProcess $proc.Id -State Established -ErrorAction SilentlyContinue)
    $panelUp = [bool]($conns | Where-Object { $_.RemotePort -eq $panelPort })
    if (Test-Path $tunnelFile) {
        $server = (Get-Content -Raw $tunnelFile | ConvertFrom-Json).server
        $tunnelPort = [int]($server -replace '^.*:', '')
        $tunnelUp = [bool]($conns | Where-Object { $_.RemotePort -eq $tunnelPort })
    }
}

Write-Host ''
if ($panelUp -and $tunnelUp) {
    Write-Host "wirefox-node $version is connected to the panel and its tunnel is up." -ForegroundColor Green
    Write-Host 'It shows up as online on the panel''s Nodes page and is ready to carry traffic; its logs are there too.'
} elseif ($panelUp) {
    Warn 'Connected to the panel, but the tunnel to the WireFox server isn''t up yet.'
    Write-Host '  Check that the server''s tunnel port (7443 by default) is reachable from this machine; the node''s logs are in the panel.'
} elseif (Get-Process wirefox-node -ErrorAction SilentlyContinue) {
    Warn "wirefox-node is running but hasn't reached the panel ($panel) yet."
    Write-Host '  Check that the panel''s agent port is reachable from this machine.'
} else {
    Warn 'wirefox-node is not running. Try it in the foreground to see why:'
    Write-Host "  & '$Exe' run --key-file '$KeyFile'"
}
Write-Host "  Task:    Get-ScheduledTask '$TaskName'   (Start-/Stop-ScheduledTask to control it)"
Write-Host "  Remove:  install-node.bat --uninstall"
