<#
.SYNOPSIS
  Prepare the deploy folder for air-gapped distribution.

.DESCRIPTION
  Run this after `npm run tauri build` (or `cargo tauri build`) completes.
  It creates a `deploy/` folder containing:
    - The built executable (from src-tauri/target/release/)
    - A models/ directory (placeholder for GGUF files)
    - A preset.json (copied from preset.example.json if not already present)

.NOTES
  PowerShell 5.1 compatible. ASCII only.
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot  = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$DeployDir = Join-Path $RepoRoot "deploy"
$TargetDir = Join-Path $RepoRoot "src-tauri\target\release"

# Product name from tauri.conf.json — the exe is named after productName.
# Tauri v2 keeps spaces: "SI Logit" -> "SI Logit.exe"
$ExeName = "SI Logit.exe"

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

# ── 1. Create deploy directory ──────────────────────────────────────────────
Write-Step "Creating deploy directory"

if (-not (Test-Path $DeployDir)) {
    New-Item -ItemType Directory -Path $DeployDir -Force | Out-Null
    Write-OK "Created $DeployDir"
} else {
    Write-OK "deploy/ already exists"
}

# ── 2. Copy executable ──────────────────────────────────────────────────────
Write-Step "Copying executable"

$ExeSrc = Join-Path $TargetDir $ExeName
$ExeDst = Join-Path $DeployDir $ExeName

if (Test-Path $ExeSrc) {
    Copy-Item -Path $ExeSrc -Destination $ExeDst -Force
    Write-OK "Copied $ExeName to deploy/"
} else {
    Write-Host "ERROR: $ExeSrc not found." -ForegroundColor Red
    Write-Host "       Run 'npm run tauri build' first." -ForegroundColor Red
    exit 1
}

# ── 3. Create models/ directory ─────────────────────────────────────────────
Write-Step "Creating models/ directory"

$ModelsDir = Join-Path $DeployDir "models"
if (-not (Test-Path $ModelsDir)) {
    New-Item -ItemType Directory -Path $ModelsDir -Force | Out-Null
}

$ModelsNote = @"
Place GGUF model files in this directory.

The application will scan this folder for .gguf files on startup.
Recommended models:
  - A text model (e.g. qwen2.5-3b-instruct-q4_k_m.gguf)
  - An optional mmproj model for vision support

Set the 'modelsPath' field in preset.json to point to this directory
(use an absolute path on the target machine).
"@

Set-Content -Path (Join-Path $ModelsDir "README.txt") -Value $ModelsNote -Encoding UTF8
Write-OK "Created models/ with README.txt"

# ── 4. Copy preset.json ─────────────────────────────────────────────────────
Write-Step "Setting up preset.json"

$PresetDst  = Join-Path $DeployDir "preset.json"
$PresetSrc  = Join-Path $PSScriptRoot "preset.example.json"

if (Test-Path $PresetDst) {
    Write-Warn "preset.json already exists in deploy/. Skipping copy (not overwriting)."
} else {
    if (Test-Path $PresetSrc) {
        Copy-Item -Path $PresetSrc -Destination $PresetDst -Force
        Write-OK "Copied preset.example.json -> deploy/preset.json"
        Write-Host "   Remember to edit preset.json with your Jira URL, PAT, and model path." -ForegroundColor Yellow
    } else {
        Write-Warn "preset.example.json not found at $PresetSrc. Skipping."
    }
}

# ── 5. Print folder structure ───────────────────────────────────────────────
Write-Step "Deploy folder structure"

function Show-Tree([string]$Path, [string]$Prefix) {
    $items = Get-ChildItem -Path $Path -Force | Sort-Object Name
    foreach ($item in $items) {
        $Name = $item.Name
        if ($item.PSIsContainer) {
            Write-Host "$Prefix$Name/"
            Show-Tree -Path $item.FullName -Prefix "$Prefix  "
        } else {
            $Size = [math]::Round($item.Length / 1MB, 1)
            Write-Host "$Prefix$Name  (${Size} MB)"
        }
    }
}

Show-Tree -Path $DeployDir -Prefix "  "

# ── Done ─────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== Deploy folder ready ===" -ForegroundColor Green
Write-Host "Location: $DeployDir"
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Edit deploy/preset.json (set jiraMcp.url, jiraMcp.pat, ai.modelsPath)"
Write-Host "  2. Copy GGUF files into deploy/models/"
Write-Host "  3. Copy the entire deploy/ folder to the air-gapped PC"
