# 개발 가이드 (DEVELOPMENT)

이 프로그램의 설정 Default 값 변경 위치와 Jira Work Item 연동 코드가 들어갈 위치를 정리한 문서입니다.

---

## 1. 설정(일반) Default 값 변경 위치

**파일**: `src-tauri/src/repo/settings.rs`

**위치**: `impl Default for Settings` (약 120행)

```rust
impl Default for Settings {
    fn default() -> Self {
        Self {
            capture_enabled: true,
            screenshot_enabled: false,
            capture_interval_seconds: 30,
            retention_days: 7,
            // ... 각 필드의 기본값
        }
    }
}
```

- `Settings` 구조체의 **모든 필드 기본값**이 여기 정의되어 있습니다 (캡처 간격, 보존 일수, 근무 시간, 개인정보 키워드 등).
- 참고: `load()` 함수(같은 파일)가 빈 `screenshot_path` / `ai.models_path`를 자동으로 기본 경로로 채웁니다.
- 참고: UI에서 값을 변경하면 `apply_patch()`(같은 파일)의 clamp/sanitize 로직이 적용됩니다.

## 2. AI 설정 Default 값 변경 위치

**파일**: `src-tauri/src/ai/config.rs`

**위치**: `impl Default for AiConfig` (약 312행)

```rust
impl Default for AiConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model: String::new(),
            api_key: String::new(),
            // ... AI 관련 기본값
        }
    }
}
```

- AI 설정(엔드포인트, 모델, API 키, 모델 경로, Jira MCP 등)의 기본값이 여기 정의되어 있습니다.
- **Jira MCP 설정 구조체**: `JiraMcpConfig` (약 58행) — `url`, `pat`, `transport` 필드.
  - `transport` 기본값은 `"remote"` (현재 `"remote"`만 지원).
- 참고: `sanitize()` 함수(같은 파일)가 AI 설정 값 검증/정리를 담당합니다.

## 3. Jira Work Item 가져오기 코드 위치

**파일**: `src-tauri/src/commands/worklog.rs`

| 함수 | 행 | 상태 | 역할 |
|------|-----|------|------|
| `fetch_work_items` | 약 53행 | **PLACEHOLDER** | Jira에서 현재 사용자의 활성 Work Item 목록 가져오기 |
| `register_work_log` | 약 111행 | **PLACEHOLDER** | Jira에 워크로그 등록 |

### 구현 시 참고사항

1. **Jira 설정 읽기**: 두 함수 모두 `Settings.ai.jira_mcp`에서 설정을 읽어야 합니다.

   ```rust
   let cfg = crate::repo::settings::load(&pool).await?;
   let jira = &cfg.ai.jira_mcp;   // url, pat, transport
   ```

   - `JiraMcpConfig` 구조체: `src-tauri/src/ai/config.rs` (58행)
   - 설정 저장: SQLite `settings_store` 단일행 JSON BLOB (`Settings.ai.jira_mcp`)

2. **PLACEHOLDER 주석**: 각 함수 위에 실제 MCP 호출 예시가 주석으로 적혀 있습니다.
   - 회사 MCP 서버의 실제 함수명/프로토콜을 확인 후 교체하세요.
   - `reqwest`는 이미 의존성에 포함되어 있습니다.

3. **프런트엔드 연결** (변경 불필요, 이미 배선됨):
   - `src/api/hindsight.ts` — `fetchWorkItems()` (1189행), `registerWorkLog()` (1191행)
   - `src/pages/WorkLog/WorkLogPage.tsx` — 호출부 (135행, 183행)

4. **배포 시 기본값 오버라이드**: `scripts/offline/preset.example.json`을 exe 옆 `preset.json`으로 두면
   첫 실행 시 설정에 자동 병합됩니다 (Jira URL/PAT, 모델 경로 등 사내 폐쇄망 맞춤 설정용).
   상세: `scripts/offline/README.md` 참고.