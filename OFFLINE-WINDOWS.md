# Closed-network Windows delivery

Run the build on a Windows build machine after the normal app has populated the selected model,
OCR, and ONNX Runtime caches (or provide equivalent files under `DataRoot`):

```powershell
npm run build:offline:windows --
```

The defaults follow the app's data-root order: `HINDSIGHT_DATA_DIR`, the documented
`APPDATA\Hindsight\bootstrap.json` `data_path` value, then the current-user `APPDATA\Hindsight`
location. Output defaults to `HINDSIGHT_OFFLINE_OUTPUT`, then a directory below `TEMP`. Override
only when needed, for example:

```powershell
npm run build:offline:windows -- -DataRoot $env:MY_HINDSIGHT_DATA -OutputDir $env:MY_OFFLINE_OUTPUT
```

Neither default contains a named user or build-machine path. Malformed or incomplete
`bootstrap.json` is ignored safely and resolution continues to the APPDATA fallback. The output
contains the NSIS installer and its adjacent `offline-assets` directory. The script stages the default Qwen3.5 4B
main/mmproj pair, PP-OCR assets, ONNX Runtime DirectML files, and CPU/CUDA 12.4/CUDA 13.1
`llama-server` directories. Missing llama release archives are downloaded and expanded only on
the build machine; target machines need no network, Rust, Node, or build tools.

The staging set needs approximately 5 GB after expansion; keep at least 8 GB free for temporary
archives and the installer build. Network access is required only on the build machine (and only
when its caches or missing llama archives need downloading). Use `-ValidateOnly` to exercise data-
root resolution without staging, downloads, or a Tauri build:

```powershell
npm run build:offline:windows -- -ValidateOnly
```

The script copies only extracted `.exe`/`.dll` llama runtime files into the delivery folder; source
ZIP archives remain temporary build-machine files and are never staged. For syntax-only validation:

```powershell
$tokens = $errors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
  (Resolve-Path src-tauri/scripts/build-offline-windows.ps1),
  [ref]$tokens, [ref]$errors
)
if ($errors.Count) { throw $errors }
```

At install time, NSIS calls `hindsight.exe --seed-resources` from the installer's own directory.
The app resolves the destination only through `bootstrap::data_root()` (`HINDSIGHT_DATA_DIR`,
bootstrap configuration, then the current-user default) and copies missing files recursively
under `ai`; existing user models, binaries, and OCR files are never overwritten. If the adjacent
folder is absent, installation continues and the NSIS detail log says so.

The installer also guarantees the per-user Start Menu shortcut
`$SMPROGRAMS\SI Logit\SI Logit.lnk` under the current user's Start Menu Programs location. It
replaces that shortcut on install and removes the shortcut and its product folder during uninstall;
no named-user path is embedded.

## Redistribution notice

The delivery contains Qwen3.5 4B GGUF/mmproj model files, llama.cpp binaries, ONNX Runtime with
DirectML, and PP-OCR assets. Preserve the upstream license and notice files with the release and
review the current Qwen model card, GGUF publisher terms, llama.cpp license, Microsoft ONNX
Runtime/DirectML licenses, and PaddleOCR/model licenses before redistribution. These notices are
not a substitute for the upstream terms; update this document when the selected model or pinned
runtime changes.
