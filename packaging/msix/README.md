# MSIX Packaging for amux

Package amux as an MSIX installer for Windows distribution with clean
install/uninstall, automatic updates, and optional Microsoft Store publishing.

## Directory Layout

```
packaging/msix/
  AppxManifest.xml          # Package manifest (binaries, capabilities, aliases)
  amux.appinstaller         # Auto-update configuration via GitHub Releases
  generate-icons.rs         # Placeholder icon generator (run from repo root)
  generate-placeholder-icons.py  # Alternative Python icon generator
  Assets/                   # MSIX icon assets (placeholder PNGs, replace before release)
```

## Prerequisites

Before building an MSIX package, you need:

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

### 3. Real Icon Assets

Replace the placeholder PNGs in `Assets/` with proper amux branding.
Required sizes are listed in `generate-icons.rs`.

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
Copy-Item $dll.FullName $staging/

# Copy manifest and assets
Copy-Item packaging/msix/AppxManifest.xml $staging/
Copy-Item -Recurse packaging/msix/Assets $staging/

# 3. Generate resources.pri (required by MSIX)
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\MakePri.exe" `
  new /pr $staging /cf packaging/msix/priconfig.xml /of "$staging\resources.pri" /o

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

## CI Integration (TODO)

The release workflow at `.github/workflows/release.yml` should be extended
to build and sign the MSIX on Windows runners. See issue #141 for the
remaining tasks:

- [ ] Add signing secrets to GitHub repository settings
- [ ] Add MSIX build step to release workflow
- [ ] Upload signed `.msix` and `.appinstaller` to GitHub Release
- [ ] Test sideloading and auto-update flow
- [ ] Optional: submit to Microsoft Store ($19 one-time fee)
