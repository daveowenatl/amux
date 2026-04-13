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
the `.msix` package on every Windows release build:

1. Stages binaries + `AppxManifest.xml` + `Assets/` into a staging directory
2. Updates the manifest version from the git tag (`v1.2.3` → `1.2.3.0`)
3. Generates `resources.pri` via `MakePri.exe` (Windows SDK)
4. Builds the `.msix` via `MakeAppx.exe`
5. Signs the `.msix` + standalone `.exe` files via Azure Trusted Signing
   (skipped if secrets aren't configured — produces an unsigned package)
6. Uploads the `.msix` as a release artifact

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

## Azure Trusted Signing Setup

The release workflow signs automatically when these GitHub Actions secrets
are configured. Without them, signing is skipped and the `.msix` is unsigned.

### 1. Create Azure resources

1. Create an Azure account at https://portal.azure.com
2. Create a **Trusted Signing** resource (search "Trusted Signing" in the portal)
3. Complete identity verification (personal or organization)
4. Create a **certificate profile** in the Trusted Signing resource

### 2. Create a service principal

```bash
# Create an app registration for GitHub Actions
az ad app create --display-name "amux-signing"

# Note the appId (CLIENT_ID) and tenant (TENANT_ID) from output
# Create a secret:
az ad app credential reset --id <appId>
# Note the password (CLIENT_SECRET)
```

Grant the service principal the "Trusted Signing Certificate Profile Signer"
role on your Trusted Signing account in the Azure portal.

### 3. Add GitHub secrets

Go to **Settings → Secrets and variables → Actions** in the amux repo:

| Secret | Value |
|--------|-------|
| `AZURE_TENANT_ID` | Your Azure AD tenant ID |
| `AZURE_CLIENT_ID` | Service principal app ID |
| `AZURE_CLIENT_SECRET` | Service principal password |
| `SIGNING_ACCOUNT` | Trusted Signing account name (just the name, not the full URL) |
| `CERT_PROFILE` | Certificate profile name |

### 4. Update the manifest publisher

Update the `Publisher` field in `AppxManifest.xml` and `amux.appinstaller`
to match the subject of the Trusted Signing certificate exactly. Azure
Trusted Signing certificates typically use a subject like
`CN=<your org name>, O=<your org name>`.

### 5. Verify

Push a tag (`git tag v0.1.0 && git push --tags`) or trigger a manual
workflow dispatch. The signing steps will run after MakeAppx and sign
both the `.msix` and the standalone `.exe` files.

## Remaining Work

- [ ] Test sideloading and auto-update flow end-to-end
- [ ] Optional: submit to Microsoft Store ($19 one-time fee)
