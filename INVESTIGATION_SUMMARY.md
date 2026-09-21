# Build/Packaging System Investigation — Compressed Map

## 1. Frontend Build
- **package.json**: scripts={dev,build,tauri,release,tauri:build:mac,...}, deps=46, devDeps=24, manager=npm, lockfile=package-lock.json
- **vite.config.ts**: build output=default, server port=41420, hmr, ignores src-tauri/, optimizeDeps entries=[index.html]

## 2. Tauri Configuration
- **tauri.conf.json**: build.beforeDevCmd="npm run dev", devUrl="http://localhost:41420", beforeBuildCmd="npm run build", frontendDist="../dist", identifier="com.tomotsugu.hindsight"
- **bundle.targets**=["nsis","app","deb","rpm","dmg"] (NSIS for Windows installer)
- **bundle.icon**=[32x32.png,128x128.png,128x128@2x.png,icon.icns,icon.ico]
- **updater.endpoints**="https://github.com/Tomotsugu-dev/Hindsight/releases/latest/download/latest.json" (meaningless in offline mode)

## 3. Cargo.toml (src-tauri)
- **tauri**=2 + features=[tray-icon,protocol-asset,image-png]
- **reqwest**=0.12 + features=[json,rustls-tls,stream,system-proxy,socks] (network client)
- **zip**=2 + feature=[deflate] (archive extraction for models)
- **tar**=0.4, **flate2**=1 (archive handling)
- **ort**="=2.0.0-rc.10" + features=[load-dynamic,ndarray,directml] (ONNX Runtime dynamic loading)
- **rusqlite**="0.32" + feature=[bundled] (SQLite compiled in, no system dep)
- **tokio**="1" + feature=[full]
- **Windows-specific**: winapi, uiautomation, webview2-com

## 4. Settings Storage (settings.load())
- **Location**: SQLite `settings_store` table, single row, JSON BLOB in `data` column
- **load() flow**:
  1. Read JSON from `SELECT data FROM settings_store WHERE id = 1`
  2. If parse fails: backup corrupt file to `settings_store.corrupt.json` (redacted), use defaults (never write over user data)
  3. If `screenshot_path` empty → fill with `<data_root>/screenshots` (via `db_path_dir()`)
  4. If `ai.models_path` empty → fill with `<data_root>/ai/models/` (via `default_root_dir()`)
  5. Migrate old `external_enabled` → `summary_main` sentinel for backward compat
  6. Clean stale prompt overrides from old pipeline
  7. Only write back if `dirty && !parse_failed` (never clobber user data on corrupt read)

## 5. Model Default Path
- **models.rs default_root_dir()**: `<data_root>/ai/models/` (via `db_path_dir().join("ai").join("models")`)
- **Fallback**: relative `ai/models/`
- **root_dir(cfg)**: Uses `cfg.models_path` if set, otherwise `default_root_dir()`
- **GGUF download source**: `https://huggingface.co/{repo}/resolve/main/{file}` (via `download_from_hf()` using `reqwest::Client` with 1h timeout)
- **Offline requirement**: Models must be pre-placed in `<data_root>/ai/models/`; HF URLs unreachable

## 6. First-Run Initialization
- **No dedicated wizard**: `settings.load()` auto-fills empty paths on every start
- **bootstrap.rs init sequence**:
  1. `init_self_identity()` — loads device, migrates legacy DB
  2. `init_database()` — opens SQLite, runs migrations
  3. `init_capture_service()` — configures capture per settings
  4. `install_tray_and_window()` — tray/icon setup
  5. `spawn_backfill_tasks()` — icon backfill, cross-OS alias pairing, builtin category backfill (runs once at startup)

## 7. Existing Structure
- **scripts/**: demo/, macos-mem-watch.sh, poc/, release.mjs, shots/
  - `release.mjs`: pushes tag → triggers GitHub Actions CI (macOS + Windows parallel)
- **.gitignore**: covers src-tauri/target/, .vite/, onnxruntime dlls, release artifacts; exceptions for scripts/demo.py, scripts/poc/, scripts/shots/, vscode settings
- **README.md**: exists with project description, quick-start, key features, release download links
- **src-tauri/icons/**: multiple icon files (ICO, ICNS, PNG sizes 284x284 down to 24x24)
- **Update mechanism**: tauri.conf.json updater.endpoints + tauri-plugin-updater dependency; offline mode renders it meaningless

## KEY OFFLINE DEPLOYMENT CONSIDERATIONS
1. No npm/pnpm/yarn in offline env → pre-bundle all deps + node_modules
2. Rust toolchain not available → need rustup or pre-built binary
3. GGUF models must be pre-placed in `<data_root>/ai/models/` (HF URLs unreachable)
4. updater endpoint disabled in offline mode (or remove from tauri.conf.json)
5. ONNX Runtime DLLs: gitignored, fetched via scripts/ or ship bundled
6. data_root defaults to `%APPDATA%/Hindsight` or can be overridden via `bootstrap.json` + `HINDSIGHT_DATA_DIR` env var

## Phase 3 cleanup remediation evidence
- Traced `SyncParse`, `SyncUtf8`, and `SyncIncomplete`: definitions/tests only in `src-tauri/src/error.rs`; no callers remain.
- Removed those error variants and their focused tests.
- Removed stale outbox/sync TODO documentation from `src-tauri/src/repo/super_categories.rs`.
- Intentionally did not modify `src-tauri/tauri.conf.json` or `createUpdaterArtifacts`.
