use serde::{Deserialize, Serialize};
use tauri::State;

use crate::repo::settings;
use crate::storage::DbPool;

// JQL: 현재 사용자에게 할당된 미완료 Work Item 조회
const JQL: &str =
    r#"issuetype = "Work Item" AND assignee = currentUser() AND statusCategory != Done"#;

// Jira custom field IDs
const CF_BACKGROUND: &str = "customfield_13548"; // 작업 배경
const CF_INFO: &str = "customfield_13549"; // 작업 수행 필요 정보
const CF_OBJECTIVE: &str = "customfield_13550"; // 작업 목표
const CF_OUTPUT: &str = "customfield_13551"; // 산출물

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub key: String,
    pub summary: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub assignee: String,
    #[serde(default)]
    pub issue_type: String,
    /// 작업 배경 (customfield_13548)
    #[serde(default)]
    pub background: String,
    /// 작업 수행 필요 정보 (customfield_13549)
    #[serde(default)]
    pub info: String,
    /// 작업 목표 (customfield_13550)
    #[serde(default)]
    pub objective: String,
    /// 산출물 (customfield_13551)
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkLogDraft {
    pub work_item_key: String,
    pub summary: String,
    pub started_at: String,
    pub ended_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkLogResult {
    pub success: bool,
    pub message: String,
}

/// Jira MCP URL 미설정 시 반환하는 테스트용 Work Item (매칭 검증용)
fn mock_work_items() -> Vec<WorkItem> {
    vec![
        WorkItem {
            key: "MOCK-101".into(),
            summary: "SI Logit 프로젝트 개발 및 OpenCode 작업".into(),
            status: "In Progress".into(),
            assignee: "me".into(),
            issue_type: "Work Item".into(),
            background: "사내 폐쇄망 배포 전환에 따른 개발 작업".into(),
            info: "OpenCode에서 작업 수행".into(),
            objective: "SI Logit 기능 개발 완료".into(),
            output: "개발 완료된 코드".into(),
        },
        WorkItem {
            key: "MOCK-102".into(),
            summary: "README 및 DEVELOPMENT 문서 작성".into(),
            status: "In Progress".into(),
            assignee: "me".into(),
            issue_type: "Work Item".into(),
            background: "프로젝트 문서화 필요".into(),
            info: "Visual Studio Code에서 문서 편집".into(),
            objective: "개발 가이드 문서 완성".into(),
            output: "README.md, DEVELOPMENT.md".into(),
        },
        WorkItem {
            key: "MOCK-103".into(),
            summary: "GitHub 저장소 push 및 si_logit_v2 관리".into(),
            status: "In Progress".into(),
            assignee: "me".into(),
            issue_type: "Work Item".into(),
            background: "코드 원격 저장소 반영 필요".into(),
            info: "GitHub 저장소 확인".into(),
            objective: "최신 코드 push 완료".into(),
            output: "si_logit_v2 저장소 최신화".into(),
        },
    ]
}

/// MCP streamable HTTP 클라이언트: initialize → tools/call → 파싱
#[tauri::command]
pub async fn fetch_work_items(pool: State<'_, DbPool>) -> Result<Vec<WorkItem>, String> {
    let cfg = settings::load(&pool).await.map_err(String::from)?;
    let jira_cfg = &cfg.ai.jira_mcp;

    if jira_cfg.url.trim().is_empty() {
        // Jira 미연결: 매칭 검증용 mock 데이터 반환
        return Ok(mock_work_items());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP 클라이언트 생성 실패: {e}"))?;

    let base_url = jira_cfg.url.trim_end_matches('/');
    let pat = &jira_cfg.pat;
    let auth = format!("Bearer {pat}");

    let headers = |req: reqwest::RequestBuilder| {
        req.header("Authorization", &auth)
            .header("Accept", "application/json, text/event-stream")
    };

    // ── Step 1: initialize ──────────────────────────────────────────
    let init_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "si-logit", "version": "0.8.24" }
        }
    });

    let init_resp = headers(client.post(base_url).json(&init_body))
        .send()
        .await
        .map_err(|e| format!("MCP initialize 요청 실패: {e}"))?;

    // Mcp-Session-Id 캡처
    let session_id = init_resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // initialize 응답 본문 파싱 (에러 무시 — 서버가 세션 ID 없이 응답할 수 있음)
    let _init_json: serde_json::Value = init_resp
        .json()
        .await
        .unwrap_or(serde_json::Value::Null);

    // ── Step 2: notifications/initialized ───────────────────────────
    let notif_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });

    let mut notif_req = headers(client.post(base_url).json(&notif_body));
    if let Some(ref sid) = session_id {
        notif_req = notif_req.header("Mcp-Session-Id", sid.as_str());
    }
    let _ = notif_req.send().await; // notification은 응답 무시

    // ── Step 3: tools/call searchJiraIssues ─────────────────────────
    let call_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "searchJiraIssues",
            "arguments": { "jql": JQL, "maxResults": 200 }
        }
    });

    let mut call_req = headers(client.post(base_url).json(&call_body));
    if let Some(ref sid) = session_id {
        call_req = call_req.header("Mcp-Session-Id", sid.as_str());
    }

    let call_resp = call_req
        .send()
        .await
        .map_err(|e| format!("MCP tools/call 요청 실패: {e}"))?;

    let status = call_resp.status();
    if !status.is_success() {
        let body_text = call_resp.text().await.unwrap_or_default();
        return Err(format!("MCP 서버 오류 (HTTP {status}): {body_text}"));
    }

    let content_type = call_resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    let resp_text = call_resp
        .text()
        .await
        .map_err(|e| format!("MCP 응답 읽기 실패: {e}"))?;

    // ── 응답 파싱: application/json 또는 text/event-stream ──────────
    let rpc_result = if content_type.contains("text/event-stream") {
        parse_sse_response(&resp_text, 2)?
    } else {
        serde_json::from_str::<serde_json::Value>(&resp_text)
            .map_err(|e| format!("MCP 응답 JSON 파싱 실패: {e}"))?
    };

    // JSON-RPC error 체크
    if let Some(err_obj) = rpc_result.get("error") {
        let msg = err_obj
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("MCP 오류: {msg}"));
    }

    // result.content[0].text 추출
    let text_json = rpc_result
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "MCP 응답에서 result.content[0].text를 찾을 수 없습니다.".to_string())?;

    let data_obj: serde_json::Value = serde_json::from_str(text_json)
        .map_err(|e| format!("MCP content text JSON 파싱 실패: {e}"))?;

    let data_arr = data_obj
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "MCP 응답에서 data 배열을 찾을 수 없습니다.".to_string())?;

    // ── WorkItem으로 변환 ──────────────────────────────────────────
    let items: Vec<WorkItem> = data_arr
        .iter()
        .map(|item| {
            let key = item
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let summary = item
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status = item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let assignee = item
                .get("assignee")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let issue_type = item
                .get("issueType")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("type").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();

            let cf = |field: &str| -> String {
                item.get("customFields")
                    .and_then(|cf| cf.get(field))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };

            WorkItem {
                key,
                summary,
                status,
                assignee,
                issue_type,
                background: cf(CF_BACKGROUND),
                info: cf(CF_INFO),
                objective: cf(CF_OBJECTIVE),
                output: cf(CF_OUTPUT),
            }
        })
        .collect();

    Ok(items)
}

/// SSE(text/event-stream) 응답에서 JSON-RPC 메시지 추출.
/// `data: {...}` 라인들을 모아 요청 id와 일치하는 메시지를 찾음.
fn parse_sse_response(body: &str, request_id: u64) -> Result<serde_json::Value, String> {
    let mut candidates: Vec<serde_json::Value> = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if let Some(payload) = line.strip_prefix("data:") {
            let payload = payload.trim();
            if payload.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) {
                candidates.push(val);
            }
        }
    }

    // id가 일치하는 메시지 우선
    for msg in &candidates {
        if msg.get("id").and_then(|v| v.as_u64()) == Some(request_id) {
            return Ok(msg.clone());
        }
    }

    // 없으면 첫 번째 유효한 JSON-RPC 메시지 반환
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| "SSE 스트림에서 유효한 JSON-RPC 메시지를 찾을 수 없습니다.".to_string())
}

// =========================================================================
// PLACEHOLDER: Replace this function body with actual MCP call
//
// This function should call your company's MCP server to register
// a work log entry via Jira for the given work item.
//
// The MCP server URL and PAT (Personal Access Token) should be stored
// in the app's settings and read from there.
//
// Example MCP call (replace with your actual implementation):
//   let client = reqwest::Client::new();
//   let resp = client
//       .post(&format!("{mcp_server_url}/tools/call"))
//       .header("Authorization", format!("Bearer {pat}"))
//       .json(&serde_json::json!({
//           "tool": "jira_add_worklog",
//           "arguments": {
//               "issue_key": draft.work_item_key,
//               "time_spent": compute_duration(&draft),
//               "comment": draft.summary,
//               "started": draft.started_at,
//           }
//       }))
//       .send()
//       .await
//       .map_err(|e| format!("MCP request failed: {e}"))?;
//
//   let body: serde_json::Value = resp.json().await
//       .map_err(|e| format!("MCP response parse failed: {e}"))?;
//   // ... check body for success
// =========================================================================
#[tauri::command]
pub async fn register_work_log(draft: WorkLogDraft) -> Result<WorkLogResult, String> {
    log::info!(
        "register_work_log: key={}, summary={}, started={}, ended={}",
        draft.work_item_key,
        draft.summary,
        draft.started_at,
        draft.ended_at
    );

    Ok(WorkLogResult {
        success: true,
        message: format!("Work log registered for {} (placeholder)", draft.work_item_key),
    })
}
