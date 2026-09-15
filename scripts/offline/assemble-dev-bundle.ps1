<#
.SYNOPSIS
  Create an offline development bundle for air-gapped Windows PCs.

.DESCRIPTION
  Run this on an internet-connected PC. It produces an `offline-bundle/` folder
  containing source, vendored Rust crates, node_modules, and the Rust toolchain
  so the project can be built on a machine with no internet access.

  Steps performed:
    1. Copy source tree (excluding build artifacts and large dirs)
    2. Vendor Rust crate dependencies via `cargo vendor`
    3. Write .cargo/config.toml for vendored source
    4. Copy Rust toolchain (stable) and cargo binaries
    5. Copy node_modules from the repo
    6. Create prereqs/ with a helper note
    7. Write README.md into the bundle

.NOTES
  PowerShell 5.1 compatible. ASCII only.
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot   = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$BundleRoot = Join-Path $RepoRoot "offline-bundle"

# ── helpers ──────────────────────────────────────────────────────────────────
function Write-Step([string]$msg) {
    Write-Host ""
    Write-Host ">> $msg" -ForegroundColor Cyan
}

function Write-OK([string]$msg) {
    Write-Host "   OK: $msg" -ForegroundColor Green
}

function Write-Warn([string]$msg) {
    Write-Host "   WARN: $msg" -ForegroundColor Yellow
}

# ── 0. Clean previous bundle ────────────────────────────────────────────────
Write-Step "Cleaning previous offline-bundle (if any)"
if (Test-Path $BundleRoot) {
    Remove-Item -Recurse -Force $BundleRoot
    Write-OK "Removed old offline-bundle"
} else {
    Write-OK "No previous bundle to remove"
}

# ── 1. Copy source tree ─────────────────────────────────────────────────────
Write-Step "Copying source tree to offline-bundle/src/"

$SrcDir = Join-Path $BundleRoot "src"
New-Item -ItemType Directory -Path $SrcDir -Force | Out-Null

# Directories to exclude from the copy
$ExcludeDirs = @(
    ".git"
    "node_modules"
    "target"
    "dist"
    "offline-bundle"
    "deploy"
    "vendor"
)

# Use robocopy for speed (mirrors tree, skips excluded dirs via /XD)
$XdArgs = @()
foreach ($d in $ExcludeDirs) {
    $XdArgs += "/XD"
    $XdArgs += $d
}
$XdArgs += "/NFL"   # no file listing
$XdArgs += "/NDL"   # no dir listing
$XdArgs += "/NJH"   # no job header
$XdArgs += "/NJS"   # no job summary
$XdArgs += "/NC"    # no class names
$XdArgs += "/NS"    # no sizes
$XdArgs += "/NP"    # no progress

& robocopy $RepoRoot $SrcDir /E /XF "*.exe" "*.dll" "*.pdb" $XdArgs | Out-Null

Write-OK "Source copied (excluding: $($ExcludeDirs -join ', '))"

# ── 2. Vendor Rust crate dependencies ───────────────────────────────────────
Write-Step "Vendoring Rust crate dependencies"

$SrcTauri = Join-Path $SrcDir "src-tauri"
if (-not (Test-Path $SrcTauri)) {
    Write-Host "ERROR: src-tauri not found in copied source. Aborting." -ForegroundColor Red
    exit 1
}

$VendorDir = Join-Path $SrcTauri "vendor"
Push-Location $SrcTauri
try {
    cargo vendor --versioned-dirs 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ERROR: cargo vendor failed (exit code $LASTEXITCODE)." -ForegroundColor Red
        Write-Host "       Make sure Rust toolchain is installed on this machine." -ForegroundColor Red
        exit 1
    }
    Write-OK "Crates vendored to src-tauri/vendor/"
} finally {
    Pop-Location
}

# ── 3. Write .cargo/config.toml ─────────────────────────────────────────────
Write-Step "Writing .cargo/config.toml for vendored sources"

$CargoConfigDir = Join-Path $SrcTauri ".cargo"
if (-not (Test-Path $CargoConfigDir)) {
    New-Item -ItemType Directory -Path $CargoConfigDir -Force | Out-Null
}

$CargoConfig = Join-Path $CargoConfigDir "config.toml"
 @"
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
"@ | Set-Content -Path $CargoConfig -Encoding UTF8

Write-OK "Written to $CargoConfig"

# ── 4. Copy Rust toolchain ──────────────────────────────────────────────────
Write-Step "Copying Rust toolchain"

$RustupDir  = Join-Path $env:USERPROFILE ".rustup"
$Toolchains = Join-Path $RustupDir "toolchains"
$CargoBin   = Join-Path $env:USERPROFILE ".cargo\bin"

$BundleRust = Join-Path $BundleRoot "rust"
New-Item -ItemType Directory -Path $BundleRust -Force | Out-Null

# Copy stable toolchain(s)
if (Test-Path $Toolchains) {
    $StableChains = Get-ChildItem -Path $Toolchains -Directory |
        Where-Object { $_.Name -like "stable-*" }

    if ($StableChains.Count -eq 0) {
        Write-Warn "No stable-* toolchain found in $Toolchains. Skipping toolchain copy."
    } else {
        $ToolchainDest = Join-Path $BundleRust "toolchain"
        New-Item -ItemType Directory -Path $ToolchainDest -Force | Out-Null

        foreach ($chain in $StableChains) {
            $Dest = Join-Path $ToolchainDest $chain.Name
            Write-Host "   Copying $($chain.Name) ..."
            Copy-Item -Path $chain.FullName -Destination $Dest -Recurse -Force
            Write-OK "Copied $($chain.Name)"
        }
    }
} else {
    Write-Warn ".rustup/toolchains not found. Skipping toolchain copy."
}

# Copy cargo binaries
if (Test-Path $CargoBin) {
    $CargoBinDest = Join-Path $BundleRust "bin"
    Copy-Item -Path $CargoBin -Destination $CargoBinDest -Recurse -Force
    Write-OK "Copied cargo binaries"
} else {
    Write-Warn ".cargo/bin not found. Skipping cargo bin copy."
}

# ── 5. Copy node_modules ────────────────────────────────────────────────────
Write-Step "Copying node_modules"

$NodeModulesSrc = Join-Path $RepoRoot "node_modules"
$NodeModulesDst = Join-Path $SrcDir "node_modules"

if (Test-Path $NodeModulesSrc) {
    Write-Host "   This may take a while (node_modules can be large) ..."
    Copy-Item -Path $NodeModulesSrc -Destination $NodeModulesDst -Recurse -Force
    Write-OK "node_modules copied"
} else {
    Write-Warn "node_modules not found in repo root. Skipping."
    Write-Warn "Run 'npm install' in the repo first, or copy node_modules manually."
}

# ── 6. Create prereqs/ folder ───────────────────────────────────────────────
Write-Step "Creating prereqs/ folder"

$PrereqsDir = Join-Path $BundleRoot "prereqs"
New-Item -ItemType Directory -Path $PrereqsDir -Force | Out-Null

$PrereqsNote = @"
Place the following offline installers in this folder before building on the
air-gapped PC:

  1. Node.js LTS installer (node-v20.x.x-x64.msi or similar)
     Download from: https://nodejs.org/en/download/

  2. Microsoft Visual Studio Build Tools
     Required components: "Desktop development with C++" workload
     Download the offline installer from:
     https://learn.microsoft.com/en-us/visualstudio/install/create-an-offline-installation-of-visual-studio

  3. Microsoft Edge WebView2 Runtime (offline installer)
     Download from: https://developer.microsoft.com/en-us/microsoft-edge/webview2/

Place all installers in this directory and run them on the target PC before
attempting to build the project.
"@

Set-Content -Path (Join-Path $PrereqsDir "README.txt") -Value $PrereqsNote -Encoding UTF8
Write-OK "Created prereqs/README.txt"

# ── 7. Write bundle README ──────────────────────────────────────────────────
Write-Step "Writing offline-bundle/README.md"

$BundleReadme = @"
# Offline Development Bundle

This bundle was generated by `scripts/offline/assemble-dev-bundle.ps1` on an
internet-connected PC. It contains everything needed to build the project on an
air-gapped (no internet) Windows machine.

## Contents

    offline-bundle/
      src/              - Full source tree (including node_modules and vendored crates)
      rust/
        toolchain/      - Stable Rust toolchain
        bin/            - Cargo and related binaries
      prereqs/          - Put offline installers here (see prereqs/README.txt)
      README.md         - This file

## Setup on the Air-Gapped PC

### 1. Install prerequisites

Run the installers from `prereqs/`:

  - Node.js LTS
  - Visual Studio Build Tools (Desktop development with C++)
  - Microsoft Edge WebView2 Runtime

### 2. Configure PATH

Open a terminal and add the following to your PATH (adjust version as needed):

    set PATH=%~dp0rust\toolchain\stable-x86_64-pc-windows-msvc\bin;%~dp0rust\bin;%PATH%

Or set them permanently via System Properties > Environment Variables.

### 3. Build the project

    cd offline-bundle\src
    npm run tauri dev          # for development (hot-reload)
    npm run tauri build        # for production build

## Notes

- The .cargo/config.toml in src-tauri/ is pre-configured to use vendored crates.
- Rust toolchain path may vary. Check `rust\toolchain\` for the exact directory name.
"@

Set-Content -Path (Join-Path $BundleRoot "README.md") -Value $BundleReadme -Encoding UTF8
Write-OK "Written offline-bundle/README.md"

# ── Done ─────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== Offline bundle created successfully ===" -ForegroundColor Green
Write-Host "Location: $BundleRoot"
Write-Host ""
Write-Host "Contents:"
Get-ChildItem -Path $BundleRoot -Recurse -Directory |
    ForEach-Object {
        $Rel = $_.FullName.Substring($BundleRoot.Length + 1)
        Write-Host "  $Rel/"
    }
