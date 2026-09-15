# 오프라인 배포 가이드

사내 폐쇄망(인터넷 불가) 환경에서 SI Logit을 빌드하고 배포하는 방법입니다.

---

## A. 개발/테스트 번들 (Jira 연결 테스트용)

### 외부망 PC에서 실행

```powershell
cd <저장소 루트>
.\scripts\offline\assemble-dev-bundle.ps1
```

`offline-bundle/` 폴더가 생성됩니다. USB 등으로 폐쇄망 PC에 복사합니다.

### 폐쇄망 PC에서 설정

#### 1. 사전 요구사항 설치

`offline-bundle/prereqs/` 폴더에 다음 설치기를 넣고 실행합니다:

| 설치기 | 설명 |
|--------|------|
| **Node.js LTS** | `node-v20.x.x-x64.msi` (https://nodejs.org) |
| **VS Build Tools** | "Desktop development with C++" 워크로드 포함 (오프라인 설치기 필요) |
| **WebView2 Runtime** | Microsoft Edge WebView2 런타임 (오프라인 설치기) |

#### 2. Rust 도구체인 PATH 설정

다음 경로를 시스템 PATH에 추가합니다:

```
<USB 드라이버 또는 복사 위치>\offline-bundle\rust\toolchain\stable-x86_64-pc-windows-msvc\bin
```

> **순서 중요**: toolchain bin이 `rust\bin`보다 앞에 와야 합니다.
> `rust\bin`의 cargo.exe는 rustup shim이라 rustup 홈이 없는 폐쇄망 PC에서 실패합니다.
> toolchain bin의 cargo.exe는 독립 실행됩니다. `rust\bin`은 PATH에 넣지 않아도 됩니다.
> 실제 stable 버전 디렉토리 이름은 `rust/toolchain/` 아래를 확인하세요.

PowerShell에서 일시적으로 설정:

```powershell
$RustRoot = "E:\offline-bundle\rust"   # 실제 경로로 변경
$env:PATH = "$RustRoot\toolchain\stable-x86_64-pc-windows-msvc\bin;$env:PATH"
```

#### 3. 빌드 실행

```powershell
cd offline-bundle\src

# 개발 모드 (핫 리로드)
npm run tauri dev

# 프로덕션 빌드
npm run build
cargo build --release --manifest-path src-tauri/Cargo.toml
```

> `src-tauri/.cargo/config.toml`이 미리 설정되어 있어 crate는 인터넷 없이 `vendor/`에서 가져옵니다.

---

## B. exe 배포

### 1. 빌드

외부망 PC에서:

```powershell
npm run tauri build
```

### 2. 배포 폴더 생성

```powershell
.\scripts\offline\prepare-deploy.ps1
```

`deploy/` 폴더가 생성됩니다:

```
deploy/
  SI Logit.exe       ← 실행 파일
  models/            ← GGUF 모델 파일을 여기에 넣으세요
    README.txt
  preset.json        ← 설정 파일 (처음 실행 시 자동 적용)
```

### 3. GGUF 모델 추가

`deploy/models/` 폴더에 GGUF 파일을 넣습니다. 예:

- 텍스트 모델: `qwen2.5-3b-instruct-q4_k_m.gguf`
- 비전 모델(선택): mmproj GGUF 파일

### 4. preset.json 편집

`deploy/preset.json`을 열어서 다음 필드를 수정합니다:

| 필드 | 설명 | 예시 |
|------|------|------|
| `ai.jiraMcp.url` | Jira 서버 주소 | `https://jira.company.com` |
| `ai.jiraMcp.pat` | Jira Personal Access Token | `NjE2...` |
| `ai.modelsPath` | GGUF 모델이 있는 절대 경로 | `C:\Hindsight\models` |

### 5. 폐쇄망에 배포

`deploy/` 폴더 전체를 폐쇄망 PC에 복사합니다.

**첫 실행 시** `preset.json`이 자동으로 `preset.applied.json`으로 변경되면서 설정이 적용됩니다.

---

## C. preset.json 필드 설명

| 필드 | 타입 | 기본값 | 설명 |
|------|------|--------|------|
| `captureIntervalSeconds` | number | `30` | 화면 캡처 간격 (초) |
| `workHoursEnabled` | boolean | `true` | 근무 시간 모드 활성화 여부 |
| `workRanges` | TimeRange[] | `[{start:"09:00",end:"18:00"}]` | 근무 시간대 목록 (HH:MM) |
| `privacyUrlKeywords` | string[] | `[]` | 캡처 제외 URL 키워드 목록 |
| `ai.jiraMcp.url` | string | `""` | Jira 서버 base URL |
| `ai.jiraMcp.pat` | string | `""` | Jira Personal Access Token (평문) |
| `ai.jiraMcp.transport` | string | `"remote"` | 연결 방식 (현재 `"remote"`만 지원) |
| `ai.modelsPath` | string | (기본값: `<data_root>/ai/models/`) | GGUF 모델 파일 디렉토리 |

> 전체 Settings 구조는 `src-tauri/src/repo/settings.rs`의 `Settings` 구조체와
> `src-tauri/src/ai/config.rs`의 `AiConfig` 구조체를 참고하세요.

---

## D. 주의사항

1. **모델 경로는 절대 경로를 권장합니다.** 상대 경로는 앱 실행 디렉토리에 따라 달라질 수 있습니다.

2. **PAT는 평문으로 저장됩니다.** `preset.json`과 `preset.applied.json` 모두 평문이므로,
   폐쇄망 PC 외부로 옮길 때 주의하세요. 설정이 적용된 후에는 `preset.applied.json`을
   안전한 곳으로 옮기거나 삭제할 수 있습니다.

3. **업데이터(tauri-plugin-updater)는 폐쇄망에서 무의미합니다.**
   설정의 `autoUpdateEnabled`를 `false`로 두는 것을 권장합니다 (기본값: false).

4. **Rust 도구체인 버전은 빌드 시 사용한 버전과 일치해야 합니다.**
   `rust/toolchain/` 아래 디렉토리 이름을 확인하세요.

---

## E. 트러블슈팅

| 증상 | 해결 방법 |
|------|-----------|
| `cargo` 명령을 찾을 수 없습니다 | Rust toolchain PATH가 올바르게 설정되었는지 확인하세요 |
| `npm: command not found` | Node.js가 설치되었는지, PATH에 포함되었는지 확인하세요 |
| 빌드 시 crate 다운로드 시도 | `.cargo/config.toml`이 올바르게 설정되었는지 확인하세요 |
| WebView2 관련 오류 | WebView2 Runtime이 설치되었는지 확인하세요 |
| exe 실행 시 설정이 적용되지 않음 | `deploy/preset.json` 파일이 exe와 같은 디렉토리에 있는지 확인하세요 |
