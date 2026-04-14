#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Download the latest MSIX artifact, sign it with the dev cert, and install.
    Run from an elevated PowerShell in the amux repo root.

.DESCRIPTION
    1. Finds amux-dev.pfx in the repo (or creates the dev cert if missing)
    2. Looks for the .msix in the current directory or Downloads
    3. Signs it with the dev cert
    4. Uninstalls any existing amux MSIX package
    5. Installs the newly signed package

.EXAMPLE
    # From elevated PowerShell, in C:\Users\DAVEOWEN\src\amux:
    .\packaging\msix\install-dev.ps1 C:\Users\DAVEOWEN\Downloads\amux-x86_64-pc-windows-msvc-msix\amux-x86_64-pc-windows-msvc.msix
#>
param(
    [Parameter(Position = 0)]
    [string]$MsixPath
)

$ErrorActionPreference = 'Stop'

# --- Find the .msix ---
if (-not $MsixPath) {
    # Try common locations
    $candidates = @(
        "amux-x86_64-pc-windows-msvc.msix",
        "$HOME\Downloads\amux-x86_64-pc-windows-msvc.msix",
        "$HOME\Downloads\amux-x86_64-pc-windows-msvc-msix\amux-x86_64-pc-windows-msvc.msix"
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { $MsixPath = $c; break }
    }
    if (-not $MsixPath) {
        Write-Error "No .msix found. Pass the path as an argument: .\install-dev.ps1 <path-to.msix>"
        exit 1
    }
}
if (-not (Test-Path $MsixPath)) {
    Write-Error "File not found: $MsixPath"
    exit 1
}
Write-Host "MSIX: $MsixPath" -ForegroundColor Cyan

# --- Find or create the dev signing cert ---
$pfxPath = Join-Path $PSScriptRoot "amux-dev.pfx"
$cerPath = Join-Path $PSScriptRoot "amux-dev.cer"
$pfxPassword = "dev"
$certSubject = "CN=amux-dev, O=Dev, L=Local, S=Dev, C=US"

if (-not (Test-Path $pfxPath)) {
    Write-Host "Creating self-signed dev certificate..." -ForegroundColor Yellow

    $cert = New-SelfSignedCertificate `
        -Type Custom `
        -Subject $certSubject `
        -KeyUsage DigitalSignature `
        -FriendlyName "amux Development" `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3")

    $pwd = ConvertTo-SecureString -String $pfxPassword -Force -AsPlainText
    Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $pwd | Out-Null
    Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
    Import-Certificate -FilePath $cerPath -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null

    Write-Host "  Created and trusted: $pfxPath" -ForegroundColor Green
} else {
    Write-Host "Using existing cert: $pfxPath" -ForegroundColor Green

    # Make sure it's trusted (idempotent)
    if (Test-Path $cerPath) {
        Import-Certificate -FilePath $cerPath -CertStoreLocation Cert:\LocalMachine\TrustedPeople -ErrorAction SilentlyContinue | Out-Null
    }
}

# --- Find SignTool ---
$signTool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\10.*\x64\SignTool.exe" -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
    Select-Object -First 1

if (-not $signTool) {
    Write-Error "SignTool.exe not found. Install the Windows SDK."
    exit 1
}
Write-Host "SignTool: $($signTool.FullName)" -ForegroundColor Cyan

# --- Sign the .msix ---
Write-Host "Signing..." -ForegroundColor Yellow
& $signTool.FullName sign /fd SHA256 /a /f $pfxPath /p $pfxPassword $MsixPath
if ($LASTEXITCODE -ne 0) {
    Write-Error "Signing failed (exit code $LASTEXITCODE)"
    exit 1
}
Write-Host "  Signed successfully" -ForegroundColor Green

# --- Install or upgrade ---
$existing = Get-AppxPackage *amux* -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "Upgrading amux $($existing.Version) → new build..." -ForegroundColor Yellow
    Add-AppxPackage -Path $MsixPath -Update
} else {
    Write-Host "Installing..." -ForegroundColor Yellow
    Add-AppxPackage -Path $MsixPath
}
Write-Host "  Done!" -ForegroundColor Green

# --- Verify ---
$pkg = Get-AppxPackage *amux*
if ($pkg) {
    Write-Host "`namux installed:" -ForegroundColor Cyan
    Write-Host "  Version:  $($pkg.Version)"
    Write-Host "  Location: $($pkg.InstallLocation)"
    Write-Host "`nLaunch from Start menu or run: amux-app" -ForegroundColor Green
} else {
    Write-Error "Installation may have failed — Get-AppxPackage returned nothing"
}
