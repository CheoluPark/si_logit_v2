use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::ai::config::AiConfig;
use crate::ai::models;
use crate::ai::server::{EngineStartOverrides, EngineState, EngineSupervisor, DEFAULT_CTX_SIZE};
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
    let _init_json: serde_json::Value = init_resp.json().await.unwrap_or(serde_json::Value::Null);

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

// ────────────────── generate_work_description ──────────────────

/// epoch ms → "HH:MM" 로컬 시각 문자열 (프롬프트 표시용).
fn epoch_ms_to_hm(ms: i64) -> String {
    let secs = ms / 1000;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    let dt: DateTime<Utc> = DateTime::from_timestamp(secs, nsecs).unwrap_or_default();
    dt.format("%H:%M").to_string()
}

/// epoch ms → RFC3339 문자열 (DB 쿼리 비교용).
fn epoch_ms_to_rfc3339(ms: i64) -> String {
    let secs = ms / 1000;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    let dt: DateTime<Utc> = DateTime::from_timestamp(secs, nsecs).unwrap_or_default();
    dt.to_rfc3339()
}

/// Compacted summaries are preferred; raw local sessions remain the transitional
/// fallback until they have a successful non-empty summary.
async fn query_ocr_evidence(mem: &MemoryDb, start_ms: i64, end_ms: i64) -> Result<String, String> {
    let start_rfc = epoch_ms_to_rfc3339(start_ms);
    let end_rfc = epoch_ms_to_rfc3339(end_ms);
    mem.0
        .call(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT started_ts, ended_ts, app_id, title, summary, 1
                       FROM ocr_task_summaries
                      WHERE julianday(started_ts) < julianday(?2)
                        AND julianday(ended_ts) > julianday(?1)
                        AND status = 'success'
                        AND trim(summary) != ''
                     UNION ALL
                     SELECT s.started_ts, s.ended_ts, s.app_id, s.title, s.text, 0
                       FROM text_sessions s
                      WHERE julianday(s.started_ts) < julianday(?2)
                        AND julianday(s.ended_ts) > julianday(?1)
                        AND s.origin_device IS NULL
                        AND trim(s.text) != ''
                        AND NOT EXISTS (
                            SELECT 1
                              FROM ocr_task_summaries c
                             WHERE c.source_session_guid = s.guid
                               AND c.status = 'success'
                               AND trim(c.summary) != ''
                        )
                     ORDER BY started_ts ASC",
                )
                .db()?;
            let evidence: Vec<String> = stmt
                .query_map(rusqlite::params![start_rfc, end_rfc], |r| {
                    let started: String = r.get(0)?;
                    let ended: String = r.get(1)?;
                    let app_id: Option<String> = r.get(2)?;
                    let title: Option<String> = r.get(3)?;
                    let text: String = r.get(4)?;
                    let compacted: i64 = r.get(5)?;
                    let label = if compacted == 1 {
                        "[COMPACTED OCR SUMMARY]"
                    } else {
                        "[RAW OCR]"
                    };
                    Ok(format!(
                        "{label}\n시간: {started} ~ {ended}\n앱: {}\n창 제목: {}\n근거:\n{}",
                        app_id.as_deref().unwrap_or(""),
                        title.as_deref().unwrap_or(""),
                        text.trim()
                    ))
                })
                .db()?
                .filter_map(|r| r.ok())
                .collect();
            Ok(evidence.join("\n---\n"))
        })
        .await
        .map_err(|e| e.to_string())
}

/// activities 테이블에서 해당 시간 범위의 process_name + window_title을 조회.
async fn query_activities(
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
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1).unwrap_or_default(),
                    ))
                })
                .db()?
                .filter_map(|r| r.ok())
                .collect();
            // 중복 제거: 같은 (process_name, window_title) 쌍은 한 번만
            let mut seen = std::collections::HashSet::new();
            let unique: Vec<String> = rows
                .into_iter()
                .filter_map(|(proc, title)| {
                    let proc = proc.trim();
                    let title = title.trim();
                    if proc.is_empty() && title.is_empty() {
                        return None;
                    }

                    let key = format!("{proc}|{title}");
                    if seen.insert(key) {
                        let mut s = proc.to_string();
                        if !title.is_empty() && !proc.is_empty() {
                            s.push_str(" — ");
                            s.push_str(&title);
                        } else if !title.is_empty() {
                            s = title.to_string();
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

fn description_system_prompt() -> &'static str {
    "당신은 Work Log 설명을 작성하는 도우미다. 입력의 Work Item 컨텍스트, OCR 텍스트, 활동 기록(process_name + window_title)을 모두 근거로 삼아 서로 보완되는 내용을 최대한 종합하라. 다음 규칙을 지켜라:\n\
        - 구체적인 한국어 작업 문장을 1~3개만 작성한다.\n\
        - 실제 파일명, 설계·구현·검증 대상, 이슈 키·프로젝트명은 입력에 있는 그대로 사용한다.\n\
        - OCR이나 창 제목에 근거가 없으면 '수정했다', '설계했다', '완료했다'처럼 단정하지 말고 관련 작업 또는 검토로 제한한다.\n\
        - 앱별 시간 나열, 총 시간 언급, 입력 데이터 원문 복사는 금지한다. Markdown 없이 평문만 출력한다."
}

fn description_system_prompt_with_template(template: &str) -> String {
    let template = template.trim();
    if template.is_empty() {
        return description_system_prompt().to_string();
    }
    format!(
        "{}\n\n[사용자 지정 Work Log 지침]\n{}\n[사용자 지정 Work Log 지침 끝]\n\n[고정 최종 규칙]\n사용자 지정 지침은 출력 형식만 정할 수 있다. 사실성·근거·증거 보존 규칙과 충돌하면 이 고정 규칙이 우선한다.",
        description_system_prompt(),
        template
    )
}

fn take_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn truncate_with_marker(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let marker = "\n[… 입력 일부 생략 …]\n";
    if max_chars <= marker.chars().count() {
        return take_chars(marker, max_chars);
    }
    let side = (max_chars - marker.chars().count()) / 2;
    let head: String = text.chars().take(side).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(max_chars - marker.chars().count() - side)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head}{marker}{tail}")
}

fn context_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, tokens: &mut Vec<String>| {
        if current.chars().count() >= 3 && !tokens.iter().any(|t| t == current) {
            tokens.push(std::mem::take(current));
        } else {
            current.clear();
        }
    };

    for ch in text.chars() {
        if ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/') {
            current.push(ch);
        } else {
            flush(&mut current, &mut tokens);
        }
    }
    flush(&mut current, &mut tokens);
    tokens
}

fn ocr_chunks(ocr_text: &str) -> Vec<&str> {
    ocr_text
        .split("\n---\n")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

fn evenly_spaced_indices(len: usize, max_count: usize) -> Vec<usize> {
    if len == 0 || max_count == 0 {
        return Vec::new();
    }
    if len <= max_count {
        return (0..len).collect();
    }
    (0..max_count)
        .map(|n| n * (len - 1) / (max_count - 1).max(1))
        .fold(Vec::new(), |mut indices, index| {
            if !indices.contains(&index) {
                indices.push(index);
            }
            indices
        })
}

/// OCR 조각을 관련도 우선으로 고르되, 시간대 앞·중간·끝도 남긴다.
fn build_budgeted_ocr_section(ocr_text: &str, budget: usize, relevance_source: &str) -> String {
    let chunks = ocr_chunks(ocr_text);
    if chunks.is_empty() || budget == 0 {
        return String::new();
    }

    let header = "[OCR 텍스트 시작]\n";
    let footer = "\n[OCR 텍스트 끝]";
    let full = chunks.join("\n---\n");
    if header.chars().count() + full.chars().count() + footer.chars().count() <= budget {
        return format!("{header}{full}{footer}");
    }

    let tokens = context_tokens(relevance_source);
    let mut relevance: Vec<(usize, usize)> = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            (
                index,
                tokens
                    .iter()
                    .filter(|token| chunk.contains(token.as_str()))
                    .count(),
            )
        })
        .filter(|(_, score)| *score > 0)
        .collect();
    relevance.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let omitted_marker = format!(
        "[OCR 일부만 포함: 전체 {}개 조각 중 대표 조각 선택]\n",
        chunks.len()
    );
    let content_budget = budget.saturating_sub(
        header.chars().count() + omitted_marker.chars().count() + footer.chars().count(),
    );
    if content_budget == 0 {
        return truncate_with_marker(&format!("{header}{omitted_marker}{footer}"), budget);
    }

    let mut order = Vec::new();
    let relevance_budget = content_budget * 2 / 3;
    let mut relevance_used = 0;
    for (index, _) in relevance {
        if relevance_used >= relevance_budget {
            break;
        }
        order.push(index);
        relevance_used += chunks[index].chars().count().min(content_budget / 8).max(1);
    }
    for index in evenly_spaced_indices(chunks.len(), 8) {
        if !order.contains(&index) {
            order.push(index);
        }
    }

    let per_chunk = (content_budget / 8).max(1);
    let mut selected = Vec::new();
    let mut used = 0;
    for index in order {
        let separator = if selected.is_empty() {
            0
        } else {
            "\n---\n".chars().count()
        };
        let remaining = content_budget.saturating_sub(used + separator);
        if remaining == 0 {
            break;
        }
        let chunk = take_chars(chunks[index], per_chunk.min(remaining));
        if chunk.is_empty() {
            continue;
        }
        used += separator + chunk.chars().count();
        selected.push((index, chunk));
    }
    selected.sort_by_key(|(index, _)| *index);
    let body = selected
        .into_iter()
        .map(|(_, chunk)| chunk)
        .collect::<Vec<_>>()
        .join("\n---\n");
    format!("{header}{omitted_marker}{body}{footer}")
}

#[cfg(test)]
fn worklog_content_budget(ctx_size: u32, max_tokens: u32) -> usize {
    worklog_content_budget_with_template(ctx_size, max_tokens, "")
}

fn worklog_content_budget_with_template(ctx_size: u32, max_tokens: u32, template: &str) -> usize {
    let input_budget = (ctx_size as usize).saturating_sub(max_tokens as usize);
    input_budget
        .saturating_sub(
            description_system_prompt_with_template(template)
                .chars()
                .count(),
        )
        .saturating_sub(768)
        * 3
        / 4
}

/// Work Item/activity를 먼저 보존하고, 남은 예산으로 OCR을 대표 샘플링한다.
#[cfg(test)]
fn build_budgeted_inputs(
    ctx_size: u32,
    max_tokens: u32,
    summary: &str,
    activity_text: &str,
    ocr_text: &str,
) -> (String, String) {
    build_budgeted_inputs_with_template(ctx_size, max_tokens, summary, activity_text, ocr_text, "")
}

fn build_budgeted_inputs_with_template(
    ctx_size: u32,
    max_tokens: u32,
    summary: &str,
    activity_text: &str,
    ocr_text: &str,
    template: &str,
) -> (String, String) {
    let budget = worklog_content_budget_with_template(ctx_size, max_tokens, template);
    let priority_budget = budget * 3 / 4;
    let activity_overhead = "[활동 기록(process_name + window_title) 시작]\n"
        .chars()
        .count()
        + "\n[활동 기록 끝]".chars().count();
    let summary_len = summary.trim().chars().count();
    let activity_len = activity_text.trim().chars().count();
    let (summary_budget, activity_payload_budget) =
        if summary_len + activity_len + activity_overhead <= priority_budget {
            (summary_len, activity_len)
        } else {
            let summary_budget = summary_len.min(priority_budget / 2);
            (
                summary_budget,
                priority_budget.saturating_sub(summary_budget + activity_overhead),
            )
        };
    let budgeted_summary = truncate_with_marker(summary, summary_budget);
    let remaining = budget.saturating_sub(budgeted_summary.chars().count());

    let activity_label = "[활동 기록(process_name + window_title) 시작]\n";
    let activity_footer = "\n[활동 기록 끝]";
    let activity_payload = truncate_with_marker(activity_text, activity_payload_budget);
    let activity_section = if activity_payload.is_empty() {
        String::new()
    } else {
        format!("{activity_label}{activity_payload}{activity_footer}")
    };
    let separator_budget = if activity_section.is_empty() { 0 } else { 2 };
    let ocr_budget = remaining
        .saturating_sub(activity_section.chars().count())
        .saturating_sub(separator_budget);
    let relevance_source = format!("{summary}\n{activity_text}");
    let ocr_section = build_budgeted_ocr_section(ocr_text, ocr_budget, &relevance_source);

    let context = match (activity_section.is_empty(), ocr_section.is_empty()) {
        (true, true) => String::new(),
        (false, true) => activity_section,
        (true, false) => ocr_section,
        (false, false) => format!("{activity_section}\n\n{ocr_section}"),
    };
    (budgeted_summary, context)
}

/// LLM 프롬프트를 빌드한다 (단위 테스트 가능).
#[cfg(test)]
fn build_description_prompt(
    date: &str,
    start_ms: i64,
    end_ms: i64,
    summary: &str,
    context_text: &str,
) -> (String, String) {
    build_description_prompt_with_template(date, start_ms, end_ms, summary, context_text, "")
}

fn build_description_prompt_with_template(
    date: &str,
    start_ms: i64,
    end_ms: i64,
    summary: &str,
    context_text: &str,
    template: &str,
) -> (String, String) {
    let system = description_system_prompt_with_template(template);

    let time_range = format!("{} ~ {}", epoch_ms_to_hm(start_ms), epoch_ms_to_hm(end_ms));
    let user_text = format!(
        "날짜: {date}\n시간범위: {time_range}\n\
         Work Item 컨텍스트 (프론트엔드에서 summary/background/info/objective/output을 합친 값):\n{summary}\n\n\
         아래 근거를 종합해 설명을 작성하라.\n{context_text}"
    );
    (system, user_text)
}

fn summary_engine_overrides(ai: &AiConfig) -> EngineStartOverrides {
    EngineStartOverrides {
        batch_size: ai.summary_batch_size_effective(),
        parallel_slots: ai.summary_parallel_slots_effective(),
        ctx_size: ai.summary_ctx_size_effective(),
    }
}

fn resolve_summary_model_paths(ai: &AiConfig) -> Result<(PathBuf, Option<PathBuf>), String> {
    let main_name = ai.effective_summary_main();
    let mmproj_name = ai.effective_summary_mmproj();
    let models_dir = models::root_dir(ai);
    let main_path = models_dir.join(main_name);
    if !main_path.exists() {
        return Err(format!(
            "Summary 모델 파일을 찾을 수 없습니다: {}（가능한 경로: {}）",
            main_name,
            main_path.display()
        ));
    }

    let mmproj_path = if mmproj_name.trim().is_empty() {
        None
    } else {
        let path = models_dir.join(mmproj_name);
        if !path.exists() {
            return Err(format!(
                "Summary vision projection 파일을 찾을 수 없습니다: {}（가능한 경로: {}）",
                mmproj_name,
                path.display()
            ));
        }
        Some(path)
    };

    Ok((main_path, mmproj_path))
}

/// 로컬 summary 모델과 시작 옵션이 일치할 때만 엔진을 재사용한다.
/// 클라우드 summary는 로컬 엔진을 시작하지 않고 `None`을 반환한다.
async fn ensure_summary_engine(
    supervisor: &Arc<EngineSupervisor>,
    ai: &AiConfig,
) -> Result<Option<u16>, String> {
    if ai.summary_use_cloud() {
        return Ok(None);
    }

    let (main_path, mmproj_path) = resolve_summary_model_paths(ai)?;
    let overrides = summary_engine_overrides(ai);
    let status = supervisor.status().await;
    if status.state == EngineState::Running {
        if let Some(port) = status.port {
            if supervisor.loaded_main().as_deref() == Some(main_path.as_path())
                && supervisor.loaded_overrides() == overrides
            {
                supervisor.touch();
                return Ok(Some(port));
            }

            log::info!(
                "worklog: loaded model/params do not match summary requirements, restarting engine"
            );
            supervisor.stop().await.map_err(|e| e.to_string())?;
        }
    }

    supervisor
        .start_with_overrides(Some(main_path), mmproj_path, overrides)
        .await
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Work Log 페이지에서 "스크린샷 분석 기반 구체적 작업 묘사"를 생성한다.
///
/// 1. MemoryDb text_sessions와 main DbPool activities에서 해당 시간대 근거를 모두 조회
/// 2. OCR과 process_name + window_title을 구획된 context로 결합
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
    // 1. Compacted OCR summaries are preferred; unprocessed local raw OCR fills gaps.
    let ocr_text = if let Some(ref mem_db) = mem.0 {
        query_ocr_evidence(mem_db, start_ms, end_ms).await?
    } else {
        String::new()
    };
    let activity_text = query_activities(&pool, &date, start_ms, end_ms).await?;

    if ocr_text.trim().is_empty() && activity_text.trim().is_empty() {
        return Err(" 해당 시간대에 스크린샷 OCR 텍스트 또는 활동 기록이 없습니다.".into());
    }

    // 2. 모델 context/output 예산에 맞춰 Work Item·활동을 우선 보존하고 OCR을 샘플링한다.
    let cfg = settings::load(&pool).await.map_err(String::from)?;
    let ai = &cfg.ai;
    let ctx_size = ai.summary_ctx_size_effective().unwrap_or(DEFAULT_CTX_SIZE);
    let (budgeted_summary, context_text) = build_budgeted_inputs_with_template(
        ctx_size,
        ai.summary_max_tokens(),
        &summary,
        &activity_text,
        &ocr_text,
        &ai.jira_worklog_prompt,
    );

    // 3. 프롬프트 빌드
    let (system, user_text) = build_description_prompt_with_template(
        &date,
        start_ms,
        end_ms,
        &budgeted_summary,
        &context_text,
        &ai.jira_worklog_prompt,
    );

    // 4. LLM 호출 (기존 infra 재사용)
    let step2 = if ai.summary_use_cloud() {
        crate::ai::summary_operations::build_step2(ai, 0, "").map_err(|e| e.to_string())?
    } else {
        let port = ensure_summary_engine(&supervisor, ai)
            .await?
            .ok_or_else(|| "로컬 summary 엔진을 시작하지 못했습니다.".to_string())?;
        crate::ai::summary_operations::build_step2(ai, port, ai.effective_summary_main())
            .map_err(|e| e.to_string())?
    };

    let _inference_guard = (!ai.summary_use_cloud()).then(|| supervisor.acquire_inference());
    let (content, _usage) = step2
        .chat(&system, &user_text, &[])
        .await
        .map_err(|e| e.to_string())?;

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::config::SUMMARY_CLOUD_SENTINEL;

    fn ms(timestamp: &str) -> i64 {
        DateTime::parse_from_rfc3339(timestamp)
            .unwrap()
            .timestamp_millis()
    }

    #[tokio::test]
    async fn compacted_summary_is_preferred_over_raw_and_other_statuses() {
        let db = MemoryDb::open_in_memory().await.unwrap();
        db.0.call(|conn| {
            conn.execute(
                "INSERT INTO text_sessions(
                        local_date, started_ts, ended_ts, app_id, title, text, guid
                     ) VALUES ('2026-09-20', '2026-09-20T10:00:00Z',
                               '2026-09-20T10:30:00Z', 'Code', 'main.rs',
                               'raw should not be used', 'compact-guid')",
                [],
            )
            .db()?;
            conn.execute(
                "INSERT INTO ocr_task_summaries(
                        source_session_guid, local_date, started_ts, ended_ts,
                        app_id, title, summary, status, updated_ts
                     ) VALUES ('compact-guid', '2026-09-20',
                               '2026-09-20T10:00:00Z', '2026-09-20T10:30:00Z',
                               'Code', 'main.rs', 'successful compacted work', 'success',
                               '2026-09-20T10:31:00Z')",
                [],
            )
            .db()?;
            conn.execute(
                "INSERT INTO ocr_task_summaries(
                        source_session_guid, local_date, started_ts, ended_ts,
                        app_id, title, summary, status, updated_ts
                     ) VALUES ('ignored-guid', '2026-09-20',
                               '2026-09-20T10:00:00Z', '2026-09-20T10:30:00Z',
                               'Code', 'main.rs', 'must not appear', 'deferred',
                               '2026-09-20T10:31:00Z')",
                [],
            )
            .db()?;
            Ok(())
        })
        .await
        .unwrap();

        let evidence =
            query_ocr_evidence(&db, ms("2026-09-20T09:00:00Z"), ms("2026-09-20T11:00:00Z"))
                .await
                .unwrap();
        assert!(evidence.contains("[COMPACTED OCR SUMMARY]"));
        assert!(evidence.contains("successful compacted work"));
        assert!(evidence.contains("앱: Code"));
        assert!(evidence.contains("창 제목: main.rs"));
        assert!(!evidence.contains("raw should not be used"));
        assert!(!evidence.contains("must not appear"));
    }

    #[tokio::test]
    async fn raw_ocr_is_used_when_session_is_not_compacted() {
        let db = MemoryDb::open_in_memory().await.unwrap();
        db.0.call(|conn| {
            conn.execute(
                "INSERT INTO text_sessions(
                        local_date, started_ts, ended_ts, app_id, title, text, guid
                     ) VALUES ('2026-09-20', '2026-09-20T10:00:00Z',
                               '2026-09-20T10:30:00Z', 'Terminal', 'cargo test',
                               'raw fallback evidence', 'raw-guid')",
                [],
            )
            .db()?;
            Ok(())
        })
        .await
        .unwrap();

        let evidence =
            query_ocr_evidence(&db, ms("2026-09-20T09:00:00Z"), ms("2026-09-20T11:00:00Z"))
                .await
                .unwrap();
        assert!(evidence.contains("[RAW OCR]"));
        assert!(evidence.contains("raw fallback evidence"));
    }

    #[tokio::test]
    async fn prior_day_compacted_summary_overlapping_selected_day_is_included() {
        let db = MemoryDb::open_in_memory().await.unwrap();
        db.0.call(|conn| {
            conn.execute(
                "INSERT INTO ocr_task_summaries(
                    source_session_guid, local_date, started_ts, ended_ts,
                    app_id, title, summary, status, updated_ts
                 ) VALUES ('overnight-summary', '2026-09-19',
                           '2026-09-19T23:30:00Z', '2026-09-20T00:30:00Z',
                           'Code', 'night.rs', 'prior-day compacted evidence', 'success',
                           '2026-09-20T00:31:00Z')",
                [],
            )
            .db()?;
            Ok(())
        })
        .await
        .unwrap();

        let evidence =
            query_ocr_evidence(&db, ms("2026-09-20T00:00:00Z"), ms("2026-09-20T01:00:00Z"))
                .await
                .unwrap();
        assert!(evidence.contains("prior-day compacted evidence"));
    }

    #[tokio::test]
    async fn prior_day_raw_fallback_overlapping_selected_day_is_included() {
        let db = MemoryDb::open_in_memory().await.unwrap();
        db.0.call(|conn| {
            conn.execute(
                "INSERT INTO text_sessions(
                    local_date, started_ts, ended_ts, text, guid
                 ) VALUES ('2026-09-19', '2026-09-19T23:30:00Z',
                           '2026-09-20T00:30:00Z', 'prior-day raw evidence',
                           'overnight-raw')",
                [],
            )
            .db()?;
            Ok(())
        })
        .await
        .unwrap();

        let evidence =
            query_ocr_evidence(&db, ms("2026-09-20T00:00:00Z"), ms("2026-09-20T01:00:00Z"))
                .await
                .unwrap();
        assert!(evidence.contains("prior-day raw evidence"));
    }

    #[tokio::test]
    async fn ocr_overlap_uses_strict_time_boundaries() {
        let db = MemoryDb::open_in_memory().await.unwrap();
        db.0.call(|conn| {
            for (guid, start, end, text) in [
                (
                    "before",
                    "2026-09-20T09:00:00Z",
                    "2026-09-20T10:00:00Z",
                    "touches start only",
                ),
                (
                    "overlap",
                    "2026-09-20T09:59:00Z",
                    "2026-09-20T10:01:00Z",
                    "overlaps boundary",
                ),
                (
                    "after",
                    "2026-09-20T11:00:00Z",
                    "2026-09-20T12:00:00Z",
                    "touches end only",
                ),
            ] {
                conn.execute(
                    "INSERT INTO text_sessions(
                            local_date, started_ts, ended_ts, text, guid
                         ) VALUES ('2026-09-20', ?1, ?2, ?3, ?4)",
                    rusqlite::params![start, end, text, guid],
                )
                .db()?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let evidence =
            query_ocr_evidence(&db, ms("2026-09-20T10:00:00Z"), ms("2026-09-20T11:00:00Z"))
                .await
                .unwrap();
        assert!(evidence.contains("overlaps boundary"));
        assert!(!evidence.contains("touches start only"));
        assert!(!evidence.contains("touches end only"));
    }

    #[test]
    fn compacted_labels_keep_prompt_budget_protection() {
        let evidence =
            "[COMPACTED OCR SUMMARY]\n시간: 10:00 ~ 10:30\n앱: Code\n창 제목: main.rs\n근거:\n요약";
        let (budgeted_summary, context) =
            build_budgeted_inputs(4096, 1024, "Work Item context", "Code — main.rs", evidence);
        let budget = worklog_content_budget(4096, 1024);

        assert!(context.contains("[COMPACTED OCR SUMMARY]"));
        assert!(budgeted_summary.chars().count() + context.chars().count() <= budget);
    }

    #[test]
    fn configured_worklog_template_is_appended_without_replacing_fixed_rules() {
        let (system, _) = build_description_prompt_with_template(
            "2026-09-20",
            ms("2026-09-20T10:00:00Z"),
            ms("2026-09-20T11:00:00Z"),
            "Work Item",
            "evidence",
            "Focus on the requested acceptance criteria.",
        );

        assert!(system.contains("[사용자 지정 Work Log 지침]"));
        assert!(system.contains("Focus on the requested acceptance criteria."));
        assert!(system.contains("근거가 없으면 '수정했다', '설계했다', '완료했다'"));
        let template_end = system.find("[사용자 지정 Work Log 지침 끝]").unwrap();
        let fixed_rule = system.find("[고정 최종 규칙]").unwrap();
        assert!(fixed_rule > template_end);
        assert!(system[fixed_rule..].contains("사실성·근거·증거 보존 규칙과 충돌하면"));
    }

    #[test]
    fn configured_worklog_template_is_reserved_before_evidence_budget() {
        let template = "사용자 지침 ".repeat(100);
        let base_budget = worklog_content_budget(4096, 1024);
        let configured_budget = worklog_content_budget_with_template(4096, 1024, &template);
        assert!(configured_budget < base_budget);

        let (budgeted_summary, context) = build_budgeted_inputs_with_template(
            4096,
            1024,
            "Work Item context",
            "Code — main.rs",
            &"OCR evidence ".repeat(1_000),
            &template,
        );
        assert!(budgeted_summary.chars().count() + context.chars().count() <= configured_budget);
    }

    #[test]
    fn build_description_prompt_includes_all_fields() {
        let (_, context) = build_budgeted_inputs(
            8192,
            4096,
            "ACU 기능 시험",
            "Visual Studio Code — main.rs\nChrome — Jira 보고서",
            "main.rs의 함수 구현 검토",
        );
        let (sys, usr) = build_description_prompt(
            "2026-09-17",
            1_726_545_600_000, // epoch ms
            1_726_549_200_000,
            "ACU 기능 시험",
            &context,
        );
        assert!(sys.contains("OCR"));
        assert!(sys.contains("평문만"));
        assert!(sys.contains("구체적인 한국어"));
        assert!(sys.contains("단정하지 말고"));
        assert!(usr.contains("2026-09-17"));
        assert!(usr.contains("Work Item 컨텍스트"));
        assert!(usr.contains("ACU 기능 시험"));
        assert!(usr.contains("[OCR 텍스트 시작]"));
        assert!(usr.contains("[활동 기록(process_name + window_title) 시작]"));
        assert!(usr.contains("Visual Studio Code"));
        assert!(usr.contains("Chrome"));
    }

    #[test]
    fn budgeted_context_combines_ocr_and_activity_sections() {
        let (_, context) = build_budgeted_inputs(
            8192,
            4096,
            "Work Item",
            "VS Code — main.rs",
            "  main.rs 구현 검토  ",
        );

        assert!(context.contains("[OCR 텍스트 시작]"));
        assert!(context.contains("main.rs 구현 검토"));
        assert!(context.contains("[활동 기록(process_name + window_title) 시작]"));
        assert!(context.contains("VS Code — main.rs"));
    }

    #[test]
    fn budgeted_inputs_sample_large_ocr_across_time_and_relevance() {
        let ocr = (0..6000)
            .map(|index| {
                if index == 3047 {
                    format!("ocr-{index} TEST-LOCAL-1 main.rs 관련 화면 내용")
                } else {
                    format!("ocr-{index} 시간대 대표 화면 내용")
                }
            })
            .collect::<Vec<_>>()
            .join("\n---\n");
        let summary = "TEST-LOCAL-1 main.rs 구현 검토";
        let activity = "VS Code — main.rs\nChrome — TEST-LOCAL-1";
        let (budgeted_summary, context) =
            build_budgeted_inputs(8192, 4096, summary, activity, &ocr);
        let budget = worklog_content_budget(8192, 4096);

        assert!(ocr.chars().count() > 126_934);
        assert!(budgeted_summary.chars().count() + context.chars().count() <= budget);
        assert!(context.contains("TEST-LOCAL-1 main.rs"));
        assert!(context.contains("ocr-0"));
        assert!(context.contains("ocr-5999"));
        assert!(context.contains("OCR 일부만 포함"));
    }

    #[test]
    fn default_summary_context_cap_reserves_output_tokens() {
        let ai = AiConfig::default();
        let ctx_size = ai.summary_ctx_size_effective().unwrap_or(DEFAULT_CTX_SIZE);

        assert_eq!(ctx_size, 8192);
        assert_eq!(ai.summary_max_tokens(), 4096);
        assert!(worklog_content_budget(ctx_size, ai.summary_max_tokens()) < 4096);
    }

    #[test]
    fn summary_engine_overrides_use_effective_values() {
        let mut ai = AiConfig::default();
        ai.batch_size = Some(32);
        ai.parallel_slots = Some(2);
        ai.ctx_size = Some(4096);

        assert_eq!(
            summary_engine_overrides(&ai),
            EngineStartOverrides {
                batch_size: Some(32),
                parallel_slots: Some(2),
                ctx_size: Some(4096),
            }
        );
    }

    #[tokio::test]
    async fn cloud_summary_skips_local_engine_startup() {
        let mut ai = AiConfig::default();
        ai.external_enabled = true;
        ai.summary_main = SUMMARY_CLOUD_SENTINEL.to_string();

        let supervisor = Arc::new(EngineSupervisor::new());
        assert_eq!(ensure_summary_engine(&supervisor, &ai).await.unwrap(), None);
        assert_eq!(supervisor.status().await.state, EngineState::Stopped);
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
