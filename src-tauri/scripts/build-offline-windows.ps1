[CmdletBinding()]
param(
    [string]$DataRoot,
    [string]$OutputDir,
    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'

$ProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Tag = 'b9025'
$ModelRoot = 'ai\models'
$OcrRoot = 'ai\ocr'
$RuntimeRoot = 'ai\runtime'
$BinaryRoot = 'ai\bin'

function Resolve-BootstrapDataRoot {
    $appData = [Environment]::GetEnvironmentVariable('APPDATA')
    if ([string]::IsNullOrWhiteSpace($appData)) { return $null }

    $configPath = Join-Path (Join-Path $appData 'Hindsight') 'bootstrap.json'
    if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) { return $null }

    try {
        $config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
        $dataPathProperty = $config.PSObject.Properties['data_path']
        if ($null -eq $dataPathProperty -or $dataPathProperty.Value -isnot [string]) { return $null }
        $dataPath = $dataPathProperty.Value.Trim()
        if ([string]::IsNullOrWhiteSpace($dataPath)) { return $null }
        return $dataPath
    }
    catch {
        Write-Warning "Ignoring malformed Hindsight bootstrap.json at ${configPath}: $($_.Exception.Message)"
        return $null
    }
}

function Resolve-DataRoot([string]$Explicit) {
    if (-not [string]::IsNullOrWhiteSpace($Explicit)) { return $Explicit }

    $fromEnvironment = [Environment]::GetEnvironmentVariable('HINDSIGHT_DATA_DIR')
    if (-not [string]::IsNullOrWhiteSpace($fromEnvironment)) { return $fromEnvironment }

    $fromBootstrap = Resolve-BootstrapDataRoot
    if (-not [string]::IsNullOrWhiteSpace($fromBootstrap)) { return $fromBootstrap }

    $appData = [Environment]::GetEnvironmentVariable('APPDATA')
    if (-not [string]::IsNullOrWhiteSpace($appData)) { return (Join-Path $appData 'Hindsight') }
    return $null
}

function Copy-RuntimeBinaries([string]$Source, [string]$Destination) {
    if (-not (Test-Path -LiteralPath $Source -PathType Container)) { throw "Cached asset directory is missing: $Source" }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Get-ChildItem -LiteralPath $Source -Recurse -File |
        Where-Object { $_.Extension -in @('.exe', '.dll') } |
        ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $Destination $_.Name) -Force
        }
}

function Test-RuntimeBinaryFilter {
    $tempRoot = Join-Path ([Environment]::GetEnvironmentVariable('TEMP')) "hindsight-offline-validation-$([guid]::NewGuid().ToString('N'))"
    $source = Join-Path $tempRoot 'source'
    $destination = Join-Path $tempRoot 'destination'
    try {
        New-Item -ItemType Directory -Force -Path $source | Out-Null
        Set-Content -LiteralPath (Join-Path $source 'llama-server.exe') -Value 'binary'
        Set-Content -LiteralPath (Join-Path $source 'ggml-cuda.dll') -Value 'binary'
        Set-Content -LiteralPath (Join-Path $source 'llama-release.zip') -Value 'archive'
        Copy-RuntimeBinaries $source $destination
        if (-not (Test-Path -LiteralPath (Join-Path $destination 'llama-server.exe') -PathType Leaf)) {
            throw 'runtime binary filter dropped llama-server.exe'
        }
        if (-not (Test-Path -LiteralPath (Join-Path $destination 'ggml-cuda.dll') -PathType Leaf)) {
            throw 'runtime binary filter dropped DLL'
        }
        if (Test-Path -LiteralPath (Join-Path $destination 'llama-release.zip')) {
            throw 'runtime binary filter staged a ZIP archive'
        }
    }
    finally {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$DataRoot = Resolve-DataRoot $DataRoot
if ($ValidateOnly) {
    if ([string]::IsNullOrWhiteSpace($DataRoot)) {
        Write-Host 'PowerShell syntax is valid; no data-root override or APPDATA fallback was found.'
    }
    else {
        Write-Host "PowerShell syntax is valid; resolved data root: $DataRoot"
    }
    Test-RuntimeBinaryFilter
    Write-Host 'Runtime binary filter validation passed; ZIP archives are excluded.'
    Write-Host 'No staging, downloads, or Tauri build requested.'
    exit 0
}

if ([string]::IsNullOrWhiteSpace($DataRoot)) {
    throw 'DataRoot is not set. Use -DataRoot or HINDSIGHT_DATA_DIR, or configure bootstrap.json.'
}
if (-not (Test-Path -LiteralPath $DataRoot -PathType Container)) {
    throw "Data root does not exist: $DataRoot"
}

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = [Environment]::GetEnvironmentVariable('HINDSIGHT_OFFLINE_OUTPUT')
    if ([string]::IsNullOrWhiteSpace($OutputDir)) {
        $temp = [Environment]::GetEnvironmentVariable('TEMP')
        if ([string]::IsNullOrWhiteSpace($temp)) { throw 'Set -OutputDir or HINDSIGHT_OFFLINE_OUTPUT.' }
        $OutputDir = Join-Path $temp 'hindsight-offline-build'
    }
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
$AssetsDir = Join-Path $OutputDir 'offline-assets'
if (Test-Path -LiteralPath $AssetsDir) { Remove-Item -LiteralPath $AssetsDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $AssetsDir | Out-Null

function Copy-DirectoryContents([string]$Source, [string]$Destination) {
    if (-not (Test-Path -LiteralPath $Source -PathType Container)) { throw "Cached asset directory is missing: $Source" }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Get-ChildItem -LiteralPath $Source -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
    }
}

function Copy-CachedFile([string[]]$Candidates, [string]$Destination) {
    $source = $Candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if ($null -eq $source) { throw "Cached asset is missing: $($Candidates -join ', ')" }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Destination) | Out-Null
    Copy-Item -LiteralPath $source -Destination $Destination -Force
}

function Download-AndExpand([string]$Url, [string]$Archive, [string]$Destination) {
    Write-Host "[offline] downloading llama archive $Url"
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Archive
    Expand-Archive -LiteralPath $Archive -DestinationPath $Destination -Force
}

function Stage-LlamaVariant([string]$Variant, [string]$MainArchive, [string]$CudaArchive) {
    $cached = Join-Path (Join-Path $DataRoot $BinaryRoot) $Variant
    $staged = Join-Path (Join-Path $AssetsDir $BinaryRoot) $Variant
    $server = Join-Path $cached 'llama-server.exe'
    if (Test-Path -LiteralPath $server -PathType Leaf) {
        Write-Host "[offline] reusing cached llama-server $Variant"
        Copy-RuntimeBinaries $cached $staged
        return
    }

    $work = Join-Path ([Environment]::GetEnvironmentVariable('TEMP')) "hindsight-llama-$Variant-$Tag"
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    try {
        $mainUrl = "https://github.com/ggml-org/llama.cpp/releases/download/$Tag/$MainArchive"
        Download-AndExpand $mainUrl (Join-Path $work $MainArchive) (Join-Path $work 'main')
        if (-not [string]::IsNullOrWhiteSpace($CudaArchive)) {
            $cudaUrl = "https://github.com/ggml-org/llama.cpp/releases/download/$Tag/$CudaArchive"
            Download-AndExpand $cudaUrl (Join-Path $work $CudaArchive) (Join-Path $work 'cuda')
        }
        $foundServer = Get-ChildItem -LiteralPath $work -Recurse -Filter 'llama-server.exe' -File | Select-Object -First 1
        if ($null -eq $foundServer) { throw "llama-server.exe was not found after expanding $MainArchive" }
        Copy-RuntimeBinaries $work $staged
    }
    finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# These names match AiConfig::default() and the UI's rec-aware mmproj filename.
$modelDestination = Join-Path $AssetsDir $ModelRoot
Copy-CachedFile @(
    (Join-Path (Join-Path $DataRoot $ModelRoot) 'Qwen3.5-4B-Q4_K_M.gguf')
) (Join-Path $modelDestination 'Qwen3.5-4B-Q4_K_M.gguf')
Copy-CachedFile @(
    (Join-Path (Join-Path $DataRoot $ModelRoot) 'Qwen3.5-4B-Q4_K_M__mmproj-F16.gguf'),
    (Join-Path (Join-Path $DataRoot $ModelRoot) 'mmproj-F16.gguf')
) (Join-Path $modelDestination 'Qwen3.5-4B-Q4_K_M__mmproj-F16.gguf')

Copy-DirectoryContents (Join-Path $DataRoot $OcrRoot) (Join-Path $AssetsDir $OcrRoot)
Copy-DirectoryContents (Join-Path $DataRoot $RuntimeRoot) (Join-Path $AssetsDir $RuntimeRoot)

Stage-LlamaVariant 'win-cpu-x64' "llama-$Tag-bin-win-cpu-x64.zip" $null
Stage-LlamaVariant 'win-cuda-12.4-x64' "llama-$Tag-bin-win-cuda-12.4-x64.zip" "cudart-llama-bin-win-cuda-12.4-x64.zip"
Stage-LlamaVariant 'win-cuda-13.1-x64' "llama-$Tag-bin-win-cuda-13.1-x64.zip" "cudart-llama-bin-win-cuda-13.1-x64.zip"

Write-Host '[offline] building the NSIS installer'
Push-Location $ProjectRoot
try {
    & npm run tauri -- build --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw "Tauri NSIS build failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}

$bundleDir = Join-Path $ProjectRoot 'src-tauri\target\release\bundle\nsis'
$installer = Get-ChildItem -LiteralPath $bundleDir -Filter '*.exe' -File |
    Sort-Object LastWriteTimeUtc | Select-Object -Last 1
if ($null -eq $installer) { throw "No NSIS installer found in $bundleDir" }
Copy-Item -LiteralPath $installer.FullName -Destination (Join-Path $OutputDir $installer.Name) -Force
Write-Host "[offline] delivery folder: $OutputDir"
Write-Host "[offline] installer: $(Join-Path $OutputDir $installer.Name)"
Write-Host "[offline] assets: $AssetsDir"
