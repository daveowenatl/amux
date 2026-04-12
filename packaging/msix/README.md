# MSIX Packaging for amux

Package amux as an MSIX installer for Windows distribution with clean
install/uninstall, automatic updates, and optional Microsoft Store publishing.

## Directory Layout

```text
packaging/msix/
  AppxManifest.xml          # Package manifest (binaries, capabilities, aliases)
  amux.appinstaller         # Auto-update configuration via GitHub Releases
  Assets/                   # MSIX icon assets (orange Menlo "a" on Monokai dark bg)
    amux.ico                # Multi-size ICO for .exe embedding (256/48/32/16px)
    icon-1024.png           # 1024px master for other platform uses
    Square44x44Logo.*       # Taskbar icon (scale-100, scale-200, unplated)
    Square150x150Logo.*     # Start menu tile (scale-100, scale-200)
    Wide310x150Logo.*       # Wide tile (scale-100, scale-200)
    StoreLogo.*             # Store listing icon (scale-100)
```

## CI Integration

The release workflow (`.github/workflows/release.yml`) automatically builds
an unsigned `.msix` package on every Windows release build:

1. Stages binaries + `AppxManifest.xml` + `Assets/` into a staging directory
2. Updates the manifest version from the git tag (`v1.2.3` → `1.2.3.0`)
3. Generates `resources.pri` via `MakePri.exe` (Windows SDK)
4. Builds the `.msix` via `MakeAppx.exe`
5. Uploads the `.msix` as a release artifact

**The package is currently unsigned.** See "Code Signing" below.

## Prerequisites (Manual Builds)

### 1. Code Signing Certificate

MSIX packages **must** be signed. Options:

- **Azure Trusted Signing** (recommended for production)
  - Create an Azure Trusted Signing account
  - Set up a certificate profile
  - GitHub Actions secrets needed:
    - `AZURE_TENANT_ID`
    - `AZURE_CLIENT_ID`
    - `AZURE_CLIENT_SECRET`
    - `SIGNING_ACCOUNT`
    - `CERT_PROFILE`

- **Self-signed certificate** (for development/testing only)
  ```powershell
  $cert = New-SelfSignedCertificate `
    -Type Custom `
    -Subject "CN=amux-dev, O=Dev, L=Local, S=Dev, C=US" `
    -KeyUsage DigitalSignature `
    -FriendlyName "amux Development" `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3")

  # Export as PFX
  $pwd = ConvertTo-SecureString -String "dev-password" -Force -AsPlainText
  Export-PfxCertificate -cert $cert -FilePath amux-dev.pfx -Password $pwd

  # Install in Trusted People (required for sideloading)
  Import-Certificate -FilePath (Export-Certificate -Cert $cert -FilePath amux-dev.cer) `
    -CertStoreLocation Cert:\LocalMachine\TrustedPeople
  ```

  **Important:** Update the `Publisher` field in `AppxManifest.xml` and
  `amux.appinstaller` to match the certificate subject exactly.

### 2. Windows SDK

The `MakeAppx.exe` and `MakePri.exe` tools from the Windows SDK are needed
to build the `.msix` package. Install via:
- Visual Studio Installer → Individual Components → "Windows 10 SDK" or "Windows 11 SDK"
- Or standalone: https://developer.microsoft.com/en-us/windows/downloads/windows-sdk/

Typical path: `C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\`

## Building an MSIX Package (Manual)

```powershell
# 1. Build release binaries
cargo build --release --workspace --target x86_64-pc-windows-msvc

# 2. Create staging directory
$staging = "amux-msix-staging"
New-Item -ItemType Directory -Force -Path $staging
Copy-Item target/x86_64-pc-windows-msvc/release/amux-app.exe $staging/
Copy-Item target/x86_64-pc-windows-msvc/release/amux.exe $staging/
Copy-Item target/x86_64-pc-windows-msvc/release/amux-agent-wrapper.exe $staging/ -ErrorAction SilentlyContinue

# Stage ghostty-vt.dll (required runtime dependency)
$dll = Get-ChildItem -Recurse target/x86_64-pc-windows-msvc/release/build `
  -Filter "ghostty-vt.dll" | Where-Object { $_.FullName -match "ghostty-install" } | Select-Object -First 1
if (-not $dll) { throw "ghostty-vt.dll not found — build may be incomplete" }
Copy-Item $dll.FullName $staging/

# Copy manifest and assets
Copy-Item packaging/msix/AppxManifest.xml $staging/
Copy-Item -Recurse packaging/msix/Assets $staging/

# 3. Generate resources.pri (required by MSIX)
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\MakePri.exe" `
  createconfig /cf "$staging\priconfig.xml" /dq en-US /o
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\MakePri.exe" `
  new /pr $staging /cf "$staging\priconfig.xml" /of "$staging\resources.pri" /o

# 4. Build the .msix
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\MakeAppx.exe" `
  pack /d $staging /p amux.msix /o

# 5. Sign (using self-signed cert for dev)
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\SignTool.exe" `
  sign /fd SHA256 /a /f amux-dev.pfx /p "dev-password" amux.msix
```

## Installing (Sideload)

```powershell
# Enable sideloading (Settings > Update & Security > For developers)
Add-AppxPackage -Path amux.msix

# Or with auto-update via .appinstaller:
Add-AppxPackage -AppInstallerFile amux.appinstaller
```

## Uninstalling

```powershell
Get-AppxPackage *amux* | Remove-AppxPackage
```

## Remaining Work

- [ ] Set up Azure Trusted Signing and wire secrets into CI
- [ ] Add signing step to the release workflow after MakeAppx
- [ ] Test sideloading and auto-update flow end-to-end
- [ ] Optional: submit to Microsoft Store ($19 one-time fee)
