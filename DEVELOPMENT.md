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
- **Jira MCP 설정 구조체**: `JiraMcpConfig` (약 58행) — `url`, `pat` 필드.
- 참고: `sanitize()` 함수(같은 파일)가 AI 설정 값 검증/정리를 담당합니다.

## 3. Jira Work Item 가져오기 코드 위치

**파일**: `src-tauri/src/commands/worklog.rs`

| 함수 | 상태 | 역할 |
|------|------|------|
| `fetch_work_items` | **구현 완료** (MCP streamable HTTP) | Jira에서 현재 사용자의 활성 Work Item 목록 가져오기 (`searchJiraIssues`) |

### 구현 시 참고사항

1. **Jira 설정 읽기**: `Settings.ai.jira_mcp`에서 설정을 읽습니다.

   ```rust
   let cfg = crate::repo::settings::load(&pool).await?;
   let jira = &cfg.ai.jira_mcp;   // url, pat
   ```

   - `JiraMcpConfig` 구조체: `src-tauri/src/ai/config.rs` (58행)
   - 설정 저장: SQLite `settings_store` 단일행 JSON BLOB (`Settings.ai.jira_mcp`)

2. **MCP 호출 흐름** (`fetch_work_items` 구현 완료):
   - `initialize` → `notifications/initialized` → `tools/call searchJiraIssues`
   - JQL: `issuetype = "Work Item" AND assignee = currentUser() AND statusCategory != Done`
   - 응답: `result.content[0].text` JSON → `data` 배열 → `WorkItem` 변환
   - 커스텀 필드: `customfield_13548`(작업 배경), `customfield_13549`(필요 정보),
     `customfield_13550`(작업 목표), `customfield_13551`(산출물)
   - JSON / SSE(`text/event-stream`) 응답 모두 처리, `Mcp-Session-Id` 헤더 처리

3. **프런트엔드 연결** (변경 불필요, 이미 배선됨):
   - `src/api/hindsight.ts` — `fetchWorkItems()`
   - `src/pages/WorkLog/WorkLogPage.tsx` — 호출부 (135행, 183행)

4. **배포 시 기본값 오버라이드**: exe 옆에 `preset.json`(부분 Settings JSON, camelCase)을 두면
   첫 실행 시 설정에 자동 병합됩니다 (Jira URL/PAT, 모델 경로 등 사내 폐쇄망 맞춤 설정용).
   적용 후 `preset.json` → `preset.applied.json`으로 변경됩니다.
   구현: `src-tauri/src/repo/settings.rs`의 `apply_preset_if_present`.
