use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub assignee: String,
    pub issue_type: String,
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

// =========================================================================
// PLACEHOLDER: Replace this function body with actual MCP call
//
// This function should call your company's MCP server to fetch active
// Jira work items assigned to the current user.
//
// The MCP server URL and PAT (Personal Access Token) should be stored
// in the app's settings and read from there.
//
// Example MCP call (replace with your actual implementation):
//   let client = reqwest::Client::new();
//   let resp = client
//       .get(&format!("{mcp_server_url}/tools/call"))
//       .header("Authorization", format!("Bearer {pat}"))
//       .json(&serde_json::json!({
//           "tool": "jira_get_my_issues",
//           "arguments": { "status_filter": "In Progress,Open,To Do" }
//       }))
//       .send()
//       .await
//       .map_err(|e| format!("MCP request failed: {e}"))?;
//
//   let body: serde_json::Value = resp.json().await
//       .map_err(|e| format!("MCP response parse failed: {e}"))?;
//   // ... parse body into Vec<WorkItem>
// =========================================================================
#[tauri::command]
pub async fn fetch_work_items() -> Result<Vec<WorkItem>, String> {
    Ok(vec![
        WorkItem {
            key: "PROJ-123".into(),
            summary: "Implement user authentication module".into(),
            status: "In Progress".into(),
            assignee: "Current User".into(),
            issue_type: "Story".into(),
        },
        WorkItem {
            key: "PROJ-456".into(),
            summary: "Fix database connection pooling issue".into(),
            status: "Open".into(),
            assignee: "Current User".into(),
            issue_type: "Bug".into(),
        },
        WorkItem {
            key: "PROJ-789".into(),
            summary: "Write API documentation for v2 endpoints".into(),
            status: "To Do".into(),
            assignee: "Current User".into(),
            issue_type: "Task".into(),
        },
    ])
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
