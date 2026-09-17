use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::ai::server::EngineSupervisor;
use crate::commands::screen_memory::MemoryState;
use crate::memory::MemoryDb;
use crate::repo::settings;
use crate::storage::{DbPool, SqliteResultExt};

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

/// MCP streamable HTTP 클라이언트: initialize → tools/call → 파싱
#[tauri::command]
pub async fn fetch_work_items(pool: State<'_, DbPool>) -> Result<Vec<WorkItem>, String> {
    let cfg = settings::load(&pool).await.map_err(String::from)?;
    let jira_cfg = &cfg.ai.jira_mcp;

    if jira_cfg.url.trim().is_empty() {
        return Err("Jira MCP 서버 URL이 설정되지 않았습니다. AI 설정 → Jira MCP에서 URL과 PAT를 입력하세요.".into());
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

// ────────────────── generate_work_description ──────────────────

/// epoch ms → "HH:MM" 로컬 시각 문자열 (프롬프트 표시용).
fn epoch_ms_to_hm(ms: i64) -> String {
    let secs = ms / 1000;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    let dt: DateTime<Utc> = DateTime::from_timestamp(secs, nsecs)
        .unwrap_or_default();
    dt.format("%H:%M").to_string()
}

/// epoch ms → RFC3339 문자열 (DB 쿼리 비교용).
fn epoch_ms_to_rfc3339(ms: i64) -> String {
    let secs = ms / 1000;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    let dt: DateTime<Utc> = DateTime::from_timestamp(secs, nsecs)
        .unwrap_or_default();
    dt.to_rfc3339()
}

/// MemoryDb의 text_sessions에서 해당 시간 범위에 겹치는 OCR 텍스트를 조회.
async fn query_ocr_text(
    mem: &MemoryDb,
    date: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<String, String> {
    let start_rfc = epoch_ms_to_rfc3339(start_ms);
    let end_rfc = epoch_ms_to_rfc3339(end_ms);
    let date = date.to_string();
    mem.0
        .call(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT text FROM text_sessions
                     WHERE local_date = ?1
                       AND started_ts < ?3
                       AND ended_ts > ?2
                     ORDER BY started_ts ASC",
                )
                .db()?;
            let texts: Vec<String> = stmt
                .query_map(rusqlite::params![date, start_rfc, end_rfc], |r| r.get(0))
                .db()?
                .filter_map(|r| r.ok())
                .filter(|t: &String| !t.trim().is_empty())
                .collect();
            Ok(texts.join("\n---\n"))
        })
        .await
        .map_err(|e| e.to_string())
}

/// activities 테이블에서 해당 시간 범위의 process_name + window_title을 조회 (fallback).
async fn query_activities_fallback(
    pool: &DbPool,
    date: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<String, String> {
    let start_rfc = epoch_ms_to_rfc3339(start_ms);
    let end_rfc = epoch_ms_to_rfc3339(end_ms);
    let date = date.to_string();
    pool.0
        .call(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT process_name, window_title FROM activities
                     WHERE local_date = ?1
                       AND started_at < ?3
                       AND ended_at > ?2
                     ORDER BY started_at ASC",
                )
                .db()?;
            let rows: Vec<(String, String)> = stmt
                .query_map(rusqlite::params![date, start_rfc, end_rfc], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1).unwrap_or_default()))
                })
                .db()?
                .filter_map(|r| r.ok())
                .collect();
            // 중복 제거: 같은 (process_name, window_title) 쌍은 한 번만
            let mut seen = std::collections::HashSet::new();
            let unique: Vec<String> = rows
                .into_iter()
                .filter_map(|(proc, title)| {
                    let key = format!("{proc}|{title}");
                    if seen.insert(key) {
                        let mut s = proc;
                        if !title.is_empty() {
                            s.push_str(" — ");
                            s.push_str(&title);
                        }
                        Some(s)
                    } else {
                        None
                    }
                })
                .collect();
            Ok(unique.join("\n"))
        })
        .await
        .map_err(|e| e.to_string())
}

/// LLM 프롬프트를 빌드한다 (단위 테스트 가능).
fn build_description_prompt(
    date: &str,
    start_ms: i64,
    end_ms: i64,
    summary: &str,
    context_text: &str,
) -> (String, String) {
    let system = "다음은 스크린샷 OCR 텍스트와 창 제목이다. \
        사용자가 수행한 작업을 구체적인 문장으로 1~3개 요약하라. \
        예: 'ACU 기능 시험 보고서 작성'. \
        활동 나열 금지, 총 수행시간 언급 금지, 마크다운 금지, 평문만.".to_string();

    let time_range = format!("{} ~ {}", epoch_ms_to_hm(start_ms), epoch_ms_to_hm(end_ms));
    let user_text = format!(
        "날짜: {date}\n시간범위: {time_range}\n작업 요약: {summary}\n\n\
         스크린샷 OCR 텍스트 및 창 제목:\n{context_text}"
    );
    (system, user_text)
}

/// Work Log 페이지에서 "스크린샷 분석 기반 구체적 작업 묘사"를 생성한다.
///
/// 1. MemoryDb text_sessions에서 해당 시간대 OCR 텍스트를 조회
/// 2. 없으면 main DbPool activities에서 process_name + window_title을 fallback
/// 3. LLM에 프롬프트를 넣어 구체적 작업 묘사를 생성
#[tauri::command]
pub async fn generate_work_description(
    pool: State<'_, DbPool>,
    mem: State<'_, MemoryState>,
    supervisor: State<'_, Arc<EngineSupervisor>>,
    date: String,
    start_ms: i64,
    end_ms: i64,
    summary: String,
) -> Result<String, String> {
    // 1. OCR 텍스트 조회 (MemoryDb)
    let context_text = if let Some(ref mem_db) = mem.0 {
        let ocr = query_ocr_text(mem_db, &date, start_ms, end_ms).await?;
        if !ocr.is_empty() {
            ocr
        } else {
            // fallback: activities
            query_activities_fallback(&pool, &date, start_ms, end_ms).await?
        }
    } else {
        // MemoryDb 불가: activities fallback only
        query_activities_fallback(&pool, &date, start_ms, end_ms).await?
    };

    if context_text.trim().is_empty() {
        return Err(" 해당 시간대에 스크린샷 OCR 텍스트 또는 활동 기록이 없습니다.".into());
    }

    // 2. 프롬프트 빌드
    let (system, user_text) = build_description_prompt(&date, start_ms, end_ms, &summary, &context_text);

    // 3. LLM 호출 (기존 infra 재사용)
    let cfg = settings::load(&pool).await.map_err(String::from)?;
    let ai = &cfg.ai;
    let step2 = if ai.summary_use_cloud() {
        crate::ai::summary_operations::build_step2(ai, 0, "")
            .map_err(|e| e.to_string())?
    } else {
        let st = supervisor.status().await;
        let port = st.port.ok_or_else(|| {
            "LLM 엔진이 실행 중이지 않습니다. 엔진을 먼저 시작하세요.".to_string()
        })?;
        crate::ai::summary_operations::build_step2(ai, port, ai.effective_summary_main())
            .map_err(|e| e.to_string())?
    };

    let (content, _usage) = step2
        .chat(&system, &user_text, &[])
        .await
        .map_err(|e| e.to_string())?;

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_description_prompt_includes_all_fields() {
        let (sys, usr) = build_description_prompt(
            "2026-09-17",
            1_726_545_600_000,  // epoch ms
            1_726_549_200_000,
            "ACU 기능 시험",
            "Visual Studio Code — main.rs\nChrome — Jira 보고서",
        );
        assert!(sys.contains("OCR"));
        assert!(sys.contains("평문만"));
        assert!(usr.contains("2026-09-17"));
        assert!(usr.contains("ACU 기능 시험"));
        assert!(usr.contains("Visual Studio Code"));
        assert!(usr.contains("Chrome"));
    }

    #[test]
    fn epoch_ms_to_hm_returns_hhmm() {
        // 2024-01-01T09:00:00Z = 1704099600000
        let hm = epoch_ms_to_hm(1_704_099_600_000);
        assert_eq!(hm, "09:00");
    }

    #[test]
    fn epoch_ms_to_rfc3339_roundtrips() {
        let ms = 1_726_544_400_000i64;
        let rfc = epoch_ms_to_rfc3339(ms);
        assert!(rfc.contains("2024"));
        assert!(rfc.contains("T"));
    }
}
