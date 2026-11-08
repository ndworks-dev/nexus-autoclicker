# Nexus Autoclicker installer.
#
#   irm https://raw.githubusercontent.com/ndworks-dev/nexus-autoclicker/main/install.ps1 | iex
#
# Installs the latest release for the current user (no admin needed) into
# %LOCALAPPDATA%\Programs\Nexus Autoclicker, adds Start Menu and Desktop
# shortcuts, and registers an uninstaller in "Installed apps".
# Running it again updates the app and keeps config.toml and input_images.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is very slow with the progress bar on PS 5.1
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$AppName  = 'Nexus Autoclicker'
$ExeName  = 'nexus-autoclicker.exe'
$Repo     = 'ndworks-dev/nexus-autoclicker'
$Url      = "https://github.com/$Repo/releases/latest/download/$ExeName"
$Dir      = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$Exe      = Join-Path $Dir $ExeName
$RegKey   = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\NexusAutoclicker'
$StartLnk = Join-Path ([Environment]::GetFolderPath('Programs')) "$AppName.lnk"
$DeskLnk  = Join-Path ([Environment]::GetFolderPath('Desktop')) "$AppName.lnk"

function New-Shortcut($Path) {
    $shell = New-Object -ComObject WScript.Shell
    $lnk = $shell.CreateShortcut($Path)
    $lnk.TargetPath = $Exe
    $lnk.WorkingDirectory = $Dir
    $lnk.IconLocation = "$Exe,0"
    $lnk.Description = 'Clicks the Nexus Mods download buttons for Wabbajack'
    $lnk.Save()
}

Write-Host "Installing $AppName..." -ForegroundColor Cyan

# The exe can't be replaced while it is running.
Get-Process -Name 'nexus-autoclicker' -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host 'Closing the running app...'
    $_ | Stop-Process -Force
    $_.WaitForExit(5000) | Out-Null
}

New-Item -ItemType Directory -Force -Path $Dir | Out-Null
$tmp = "$Exe.download"
Write-Host "Downloading $Url"
Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing
Move-Item -Force $tmp $Exe
Unblock-File $Exe

New-Shortcut $StartLnk
New-Shortcut $DeskLnk

# Uninstaller, also used by Settings > Apps > Installed apps.
$uninstall = Join-Path $Dir 'uninstall.ps1'
@"
`$ErrorActionPreference = 'SilentlyContinue'
Get-Process -Name 'nexus-autoclicker' | Stop-Process -Force
Start-Sleep -Milliseconds 500
Remove-Item -Force '$StartLnk', '$DeskLnk'
Remove-Item -Recurse -Force '$RegKey'
Set-Location `$env:TEMP
Remove-Item -Recurse -Force '$Dir'
Write-Host '$AppName was uninstalled.'
"@ | Set-Content -Path $uninstall -Encoding UTF8

$version = (Get-Item $Exe).VersionInfo.ProductVersion
New-Item -Force -Path $RegKey | Out-Null
$props = @{
    DisplayName     = $AppName
    DisplayVersion  = $version
    Publisher       = 'ndworks'
    DisplayIcon     = "$Exe,0"
    InstallLocation = $Dir
    URLInfoAbout    = "https://github.com/$Repo"
    UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$uninstall`""
}
foreach ($k in $props.Keys) { New-ItemProperty -Force -Path $RegKey -Name $k -Value $props[$k] | Out-Null }
New-ItemProperty -Force -Path $RegKey -Name NoModify -Value 1 -PropertyType DWord | Out-Null
New-ItemProperty -Force -Path $RegKey -Name NoRepair -Value 1 -PropertyType DWord | Out-Null

Write-Host "$AppName $version installed to $Dir" -ForegroundColor Green
Write-Host 'Shortcuts added to the Start Menu and Desktop. Uninstall from Settings > Apps > Installed apps.'
Start-Process -FilePath $Exe -WorkingDirectory $Dir
