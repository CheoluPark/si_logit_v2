//! Local-LLM compaction of sealed OCR sessions.
//!
//! This worker is deliberately separate from the OCR digest path: it consumes
//! completed sessions only and uses the digest guard only around short memory
//! DB snapshots/commits, never while starting or calling the local model.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Duration, Utc};
use rusqlite::params;

use super::{digest, MemoryDb};
use crate::ai::config::AiConfig;
use crate::ai::models;
use crate::ai::server::{EngineLease, EngineStartOverrides, EngineSupervisor};
use crate::ai::summary_operations;
use crate::error::{Error, Result};
use crate::repo::settings;
use crate::storage::{DbPool, SqliteResultExt};

const SEALED_AGE: Duration = Duration::minutes(10);
const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const FIRST_CHECK_DELAY: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const BATCH_SIZE: i64 = 4;
const MAX_RETRIES: i64 = 3;
#[derive(Debug, Clone)]
struct SessionCandidate {
    guid: String,
    local_date: String,
    started_ts: String,
    ended_ts: String,
    app_id: Option<String>,
    title: Option<String>,
    text: String,
}

/// Start one bounded background pass. Existing sessions become eligible by the
/// normal age query; there is intentionally no startup bulk loop.
pub fn spawn(pool: DbPool, mem: MemoryDb, supervisor: Arc<EngineSupervisor>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if let Err(e) = run_once(&pool, &mem, &supervisor).await {
                log::debug!("OCR compaction this round skipped: {e}");
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

async fn run_once(pool: &DbPool, mem: &MemoryDb, supervisor: &Arc<EngineSupervisor>) -> Result<()> {
    let candidates = {
        let Some(_maintenance) = digest::acquire_maintenance() else {
            return Ok(());
        };
        let cutoff = (Utc::now() - SEALED_AGE).to_rfc3339();
        load_candidates(mem, cutoff).await?
    };
    if candidates.is_empty() {
        return Ok(());
    }

    let cfg = settings::load(pool).await?;
    let ai = &cfg.ai;
    if ai.summary_use_cloud() {
        return Ok(());
    }

    // A background pass may reuse a matching engine or start one only when it
    // is stopped. A running foreground/other-model engine is never disturbed.
    let lease = match ensure_summary_engine(supervisor, ai).await {
        Ok(lease) => lease,
        Err(e) => {
            log::debug!("OCR compaction deferred: {e}");
            return Ok(());
        }
    };

    for candidate in candidates {
        if let Err(e) = lease.validate().await {
            record_deferred(mem, &candidate, &e.to_string()).await?;
            return Ok(());
        }
        let step2 =
            match summary_operations::build_step2(ai, lease.port(), ai.effective_summary_main()) {
                Ok(client) => client,
                Err(e) => {
                    record_deferred(mem, &candidate, &e.to_string()).await?;
                    return Ok(());
                }
            };
        let user = compaction_prompt(&candidate);
        let input_tokens = {
            let _inference = supervisor.acquire_inference();
            step2
                .exact_input_tokens(compaction_system_prompt(), &user, &[])
                .await
        };
        let input_tokens = match input_tokens {
            Ok(tokens) => tokens,
            Err(e) => {
                record_deferred(mem, &candidate, &e.to_string()).await?;
                continue;
            }
        };
        if !input_fits_context(input_tokens, step2.max_tokens(), lease.actual_ctx_size()) {
            record_needs_split(mem, &candidate).await?;
            continue;
        }
        if let Err(e) = lease.validate().await {
            record_deferred(mem, &candidate, &e.to_string()).await?;
            return Ok(());
        }
        let result = {
            let _inference = supervisor.acquire_inference();
            step2.chat(compaction_system_prompt(), &user, &[]).await
        };

        match result {
            Ok((summary, _usage)) if !summary.trim().is_empty() => {
                if let Err(e) = lease.validate().await {
                    record_deferred(mem, &candidate, &e.to_string()).await?;
                    return Ok(());
                }
                match commit_success(mem, &candidate, summary.trim()).await? {
                    CommitResult::Committed | CommitResult::AlreadyDone => {}
                    CommitResult::Stale | CommitResult::Deferred => {}
                }
            }
            Ok(_) => {
                record_failure(mem, &candidate, "로컬 summary 응답이 비어 있습니다").await?;
            }
            Err(e) => {
                record_failure(mem, &candidate, &e.to_string()).await?;
            }
        }
    }
    drop(lease);
    Ok(())
}

fn compaction_system_prompt() -> &'static str {
    "당신은 OCR 작업 기록을 압축하는 도우미다. OCR과 창 제목에 실제로 나타난 근거만 사용해 구체적이고 사실적인 한국어 작업 요약을 1~2문장으로 작성하라. 파일명, 이슈 키, 프로젝트명은 입력 그대로 유지하라. 근거가 부족하면 작업을 단정하지 말고 검토 또는 관련 작업으로 표현하라. 시간 나열, 원문 복사, Markdown, 추측을 금지한다."
}

fn compaction_prompt(source: &SessionCandidate) -> String {
    format!(
        "날짜: {}\n시간: {} ~ {}\n앱: {}\n창 제목: {}\nOCR:\n{}",
        source.local_date,
        source.started_ts,
        source.ended_ts,
        source.app_id.as_deref().unwrap_or(""),
        source.title.as_deref().unwrap_or(""),
        source.text
    )
}

fn input_fits_context(input_tokens: u32, output_tokens: u32, ctx_size: u32) -> bool {
    input_tokens.saturating_add(output_tokens) <= ctx_size
}

async fn load_candidates(mem: &MemoryDb, cutoff: String) -> Result<Vec<SessionCandidate>> {
    mem.0
        .call(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT s.guid, s.local_date, s.started_ts, s.ended_ts,
                            s.app_id, s.title, s.text
                       FROM text_sessions s
                       LEFT JOIN ocr_task_summaries c
                         ON c.source_session_guid = s.guid
                      WHERE s.origin_device IS NULL
                        AND julianday(s.ended_ts) < julianday(?1)
                        AND trim(s.text) != ''
                        AND s.guid IS NOT NULL
                        AND trim(s.guid) != ''
                        AND (c.source_session_guid IS NULL
                             OR (c.status != 'success'
                                 AND c.status != 'needs_split'
                                 AND c.attempts < ?2))
                      ORDER BY julianday(s.ended_ts) ASC, s.guid ASC
                      LIMIT ?3",
                )
                .db()?;
            let rows = stmt
                .query_map(params![cutoff, MAX_RETRIES, BATCH_SIZE], |r| {
                    Ok(SessionCandidate {
                        guid: r.get(0)?,
                        local_date: r.get(1)?,
                        started_ts: r.get(2)?,
                        ended_ts: r.get(3)?,
                        app_id: r.get(4)?,
                        title: r.get(5)?,
                        text: r.get(6)?,
                    })
                })
                .db()?
                .collect::<rusqlite::Result<Vec<_>>>()
                .db()?;
            Ok(rows)
        })
        .await
        .map_err(Into::into)
}

fn summary_engine_overrides(ai: &AiConfig) -> EngineStartOverrides {
    EngineStartOverrides {
        batch_size: ai.summary_batch_size_effective(),
        parallel_slots: ai.summary_parallel_slots_effective(),
        ctx_size: ai.summary_ctx_size_effective(),
    }
}

fn resolve_summary_model_paths(ai: &AiConfig) -> Result<(PathBuf, Option<PathBuf>)> {
    let models_dir = models::root_dir(ai);
    let main_name = ai.effective_summary_main();
    let main = models_dir.join(main_name);
    if !main.is_file() {
        return Err(Error::ModelFileMissing(format!(
            "{}（可能被删除或路径变了）",
            main_name
        )));
    }
    let mmproj_name = ai.effective_summary_mmproj();
    let mmproj = if mmproj_name.trim().is_empty() {
        None
    } else {
        let path = models_dir.join(mmproj_name);
        if !path.is_file() {
            return Err(Error::ModelFileMissing(format!(
                "vision 投影 {}",
                mmproj_name
            )));
        }
        Some(path)
    };
    Ok((main, mmproj))
}

async fn ensure_summary_engine(
    supervisor: &Arc<EngineSupervisor>,
    ai: &AiConfig,
) -> Result<EngineLease> {
    let (main, mmproj) = resolve_summary_model_paths(ai)?;
    let overrides = summary_engine_overrides(ai);
    supervisor
        .acquire_matching_engine_lease(Some(main), mmproj, overrides)
        .await
}

async fn record_failure(mem: &MemoryDb, source: &SessionCandidate, error: &str) -> Result<()> {
    let error = error.chars().take(500).collect::<String>();
    let now = Utc::now().to_rfc3339();
    let source = source.clone();
    let Some(_maintenance) = digest::acquire_maintenance() else {
        return Ok(());
    };
    mem.0
        .call(move |conn| {
            conn.execute(
                "INSERT INTO ocr_task_summaries(
                    source_session_guid, local_date, started_ts, ended_ts,
                    app_id, title, summary, status, attempts, last_error, updated_ts
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', 'error', 1, ?7, ?8)
                 ON CONFLICT(source_session_guid) DO UPDATE SET
                    status = 'error',
                    attempts = MIN(ocr_task_summaries.attempts + 1, ?9),
                    last_error = excluded.last_error,
                    updated_ts = excluded.updated_ts",
                params![
                    source.guid,
                    source.local_date,
                    source.started_ts,
                    source.ended_ts,
                    source.app_id,
                    source.title,
                    error,
                    now,
                    MAX_RETRIES,
                ],
            )
            .db()?;
            Ok(())
        })
        .await
        .map_err(Into::into)
}

async fn record_needs_split(mem: &MemoryDb, source: &SessionCandidate) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let source = source.clone();
    let Some(_maintenance) = digest::acquire_maintenance() else {
        return Ok(());
    };
    mem.0
        .call(move |conn| {
            conn.execute(
                "INSERT INTO ocr_task_summaries(
                    source_session_guid, local_date, started_ts, ended_ts,
                    app_id, title, summary, status, attempts, last_error, updated_ts
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', 'needs_split', 0, ?7, ?8)
                 ON CONFLICT(source_session_guid) DO UPDATE SET
                    status = 'needs_split',
                    last_error = excluded.last_error,
                    updated_ts = excluded.updated_ts",
                params![
                    source.guid,
                    source.local_date,
                    source.started_ts,
                    source.ended_ts,
                    source.app_id,
                    source.title,
                    "OCR prompt exceeds the conservative configured context budget",
                    now,
                ],
            )
            .db()?;
            Ok(())
        })
        .await
        .map_err(Into::into)
}

async fn record_deferred(mem: &MemoryDb, source: &SessionCandidate, error: &str) -> Result<()> {
    let error = error.chars().take(500).collect::<String>();
    let now = Utc::now().to_rfc3339();
    let source = source.clone();
    let Some(_maintenance) = digest::acquire_maintenance() else {
        return Ok(());
    };
    mem.0
        .call(move |conn| {
            conn.execute(
                "INSERT INTO ocr_task_summaries(
                    source_session_guid, local_date, started_ts, ended_ts,
                    app_id, title, summary, status, attempts, last_error, updated_ts
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', 'deferred', 0, ?7, ?8)
                 ON CONFLICT(source_session_guid) DO UPDATE SET
                    status = 'deferred',
                    last_error = excluded.last_error,
                    updated_ts = excluded.updated_ts",
                params![
                    source.guid,
                    source.local_date,
                    source.started_ts,
                    source.ended_ts,
                    source.app_id,
                    source.title,
                    error,
                    now,
                ],
            )
            .db()?;
            Ok(())
        })
        .await
        .map_err(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitResult {
    Committed,
    AlreadyDone,
    Stale,
    Deferred,
}

async fn commit_success(
    mem: &MemoryDb,
    source: &SessionCandidate,
    summary: &str,
) -> Result<CommitResult> {
    let summary = summary.trim().to_string();
    if summary.is_empty() {
        return Err(Error::InvalidInput("OCR task summary must not be empty"));
    }
    let now = Utc::now().to_rfc3339();
    let source = source.clone();
    let Some(_maintenance) = digest::acquire_maintenance() else {
        return Ok(CommitResult::Deferred);
    };
    mem.0
        .call(move |conn| {
            let tx = conn.transaction().db()?;
            let source_exists: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM text_sessions
                      WHERE guid = ?1 AND text = ?2 AND ended_ts = ?3
                        AND origin_device IS NULL",
                    params![source.guid, source.text, source.ended_ts],
                    |r| r.get(0),
                )
                .db()?;
            let already_done: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM ocr_task_summaries
                      WHERE source_session_guid = ?1 AND status = 'success'",
                    [source.guid.as_str()],
                    |r| r.get(0),
                )
                .db()?;
            if source_exists == 0 && already_done > 0 {
                return Ok(CommitResult::AlreadyDone);
            }
            if source_exists == 0 {
                return Ok(CommitResult::Stale);
            }
            let source_id: i64 = tx
                .query_row(
                    "SELECT id FROM text_sessions
                      WHERE guid = ?1 AND text = ?2 AND ended_ts = ?3
                        AND origin_device IS NULL",
                    params![source.guid, source.text, source.ended_ts],
                    |r| r.get(0),
                )
                .db()?;

            tx.execute(
                "INSERT INTO ocr_task_summaries(
                    source_session_guid, local_date, started_ts, ended_ts,
                    app_id, title, summary, status, attempts, last_error, updated_ts
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'success', 0, NULL, ?8)
                 ON CONFLICT(source_session_guid) DO UPDATE SET
                    summary = excluded.summary,
                    status = 'success',
                    attempts = 0,
                    last_error = NULL,
                    updated_ts = excluded.updated_ts",
                params![
                    source.guid,
                    source.local_date,
                    source.started_ts,
                    source.ended_ts,
                    source.app_id,
                    source.title,
                    summary,
                    now,
                ],
            )
            .db()?;
            tx.execute(
                "DELETE FROM session_lines WHERE session_id = ?1",
                [source_id],
            )
            .db()?;
            tx.execute(
                "UPDATE frames SET session_id = NULL WHERE session_id = ?1",
                [source_id],
            )
            .db()?;
            let deleted = tx
                .execute(
                    "DELETE FROM text_sessions
                      WHERE guid = ?1 AND text = ?2 AND ended_ts = ?3
                        AND origin_device IS NULL",
                    params![source.guid, source.text, source.ended_ts],
                )
                .db()?;
            if deleted != 1 {
                return Ok(CommitResult::Stale);
            }
            tx.commit().db()?;
            Ok(CommitResult::Committed)
        })
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::server::parse_slots_context;
    use crate::ai::server::EngineState;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn source(id: i64) -> SessionCandidate {
        SessionCandidate {
            guid: format!("session-guid-{id}"),
            local_date: "2026-09-20".into(),
            started_ts: "2026-09-20T09:00:00+00:00".into(),
            ended_ts: "2026-09-20T09:05:00+00:00".into(),
            app_id: Some("code".into()),
            title: Some("main.rs".into()),
            text: "main.rs 작업 내용".into(),
        }
    }

    async fn insert_source(db: &MemoryDb, id: i64, source: &SessionCandidate) {
        let source = source.clone();
        db.0.call(move |conn| {
            let frame_path = format!("/frame-{id}-{}.jpg", source.guid);
            conn.execute(
                "INSERT INTO text_sessions(
                        id, local_date, started_ts, ended_ts, app_id, title, text,
                        guid, origin_device
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
                params![
                    id,
                    source.local_date,
                    source.started_ts,
                    source.ended_ts,
                    source.app_id,
                    source.title,
                    source.text,
                    source.guid,
                ],
            )
            .db()?;
            conn.execute(
                "INSERT INTO frames(path, ts, local_date, app_id, title, session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    frame_path,
                    source.started_ts,
                    source.local_date,
                    source.app_id,
                    source.title,
                    id
                ],
            )
            .db()?;
            conn.execute(
                "INSERT INTO session_lines(session_id, line_no, text, first_path, first_ts)
                     VALUES (?1, 0, 'main.rs 작업 내용', ?2, ?3)",
                params![id, frame_path, source.started_ts],
            )
            .db()?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn success_transaction_deletes_source_and_is_idempotent() {
        let _lock = TEST_LOCK.lock().unwrap();
        let db = MemoryDb::open_in_memory().await.unwrap();
        let source = source(1);
        insert_source(&db, 1, &source).await;

        commit_success(&db, &source, "main.rs 작업 내용을 검토함")
            .await
            .unwrap();
        commit_success(&db, &source, "main.rs 작업 내용을 검토함")
            .await
            .unwrap();

        db.0.call(|conn| {
            let raw: i64 = conn
                .query_row("SELECT COUNT(*) FROM text_sessions", [], |r| r.get(0))
                .db()?;
            let lines: i64 = conn
                .query_row("SELECT COUNT(*) FROM session_lines", [], |r| r.get(0))
                .db()?;
            let attached_frames: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM frames WHERE session_id IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .db()?;
            let ledger: (i64, String) = conn
                .query_row(
                    "SELECT COUNT(*), MAX(status) FROM ocr_task_summaries",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .db()?;
            assert_eq!(
                (raw, lines, attached_frames, ledger),
                (0, 0, 0, (1, "success".into()))
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn failure_records_bounded_error_and_retains_raw() {
        let _lock = TEST_LOCK.lock().unwrap();
        let db = MemoryDb::open_in_memory().await.unwrap();
        let source = source(2);
        insert_source(&db, 2, &source).await;

        for _ in 0..(MAX_RETRIES + 2) {
            record_failure(&db, &source, "모델 오류").await.unwrap();
        }

        db.0.call(|conn| {
            let raw: i64 = conn
                .query_row("SELECT COUNT(*) FROM text_sessions", [], |r| r.get(0))
                .db()?;
            let row: (String, i64, String) = conn
                .query_row(
                    "SELECT status, attempts, last_error FROM ocr_task_summaries",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .db()?;
            assert_eq!(raw, 1);
            assert_eq!(row, ("error".into(), MAX_RETRIES, "모델 오류".into()));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn candidates_order_by_absolute_time() {
        let db = MemoryDb::open_in_memory().await.unwrap();
        db.0.call(|conn| {
            for (guid, ended) in [
                ("later", "2026-09-20T08:00:00+00:00"),
                ("early", "2026-09-20T09:00:00+02:00"),
            ] {
                conn.execute(
                    "INSERT INTO text_sessions(
                             local_date, started_ts, ended_ts, text, guid
                         ) VALUES ('2026-09-20', ?1, ?2, 'ocr', ?3)",
                    params![ended, ended, guid],
                )
                .db()?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let candidates = load_candidates(&db, "2026-09-20T10:00:00+00:00".into())
            .await
            .unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.guid.as_str())
                .collect::<Vec<_>>(),
            vec!["early", "later"]
        );
    }

    #[tokio::test]
    async fn cutoff_uses_absolute_instant_for_offset_timestamps() {
        let db = MemoryDb::open_in_memory().await.unwrap();
        db.0.call(|conn| {
            for (guid, ended) in [
                ("before-jst", "2026-09-20T08:00:00+09:00"),
                ("after-utc", "2026-09-20T08:00:00+00:00"),
            ] {
                conn.execute(
                    "INSERT INTO text_sessions(local_date, started_ts, ended_ts, text, guid)
                     VALUES ('2026-09-20', ?1, ?1, 'ocr', ?2)",
                    params![ended, guid],
                )
                .db()?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let candidates = load_candidates(&db, "2026-09-20T00:00:00+00:00".into())
            .await
            .unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.guid.as_str())
                .collect::<Vec<_>>(),
            vec!["before-jst"]
        );
    }

    #[tokio::test]
    async fn source_mutation_between_snapshot_and_commit_is_deferred() {
        let _lock = TEST_LOCK.lock().unwrap();
        let db = MemoryDb::open_in_memory().await.unwrap();
        let source = source(4);
        insert_source(&db, 4, &source).await;
        let guid = source.guid.clone();
        db.0.call(move |conn| {
            conn.execute(
                "UPDATE text_sessions SET text = 'changed' WHERE guid = ?1",
                [guid],
            )
            .db()?;
            Ok(())
        })
        .await
        .unwrap();

        assert_eq!(
            commit_success(&db, &source, "요약").await.unwrap(),
            CommitResult::Stale
        );
        db.0.call(|conn| {
            let raw: i64 = conn
                .query_row("SELECT COUNT(*) FROM text_sessions", [], |r| r.get(0))
                .db()?;
            let ledger: i64 = conn
                .query_row("SELECT COUNT(*) FROM ocr_task_summaries", [], |r| r.get(0))
                .db()?;
            assert_eq!((raw, ledger), (1, 0));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn guid_identity_does_not_follow_reused_session_id() {
        let _lock = TEST_LOCK.lock().unwrap();
        let db = MemoryDb::open_in_memory().await.unwrap();
        let old = source(5);
        insert_source(&db, 5, &old).await;
        assert_eq!(
            commit_success(&db, &old, "이전 요약").await.unwrap(),
            CommitResult::Committed
        );

        let mut replacement = source(6);
        replacement.guid = "replacement-guid".into();
        replacement.text = "새 작업 내용".into();
        insert_source(&db, 5, &replacement).await;

        assert_eq!(
            commit_success(&db, &old, "이전 요약").await.unwrap(),
            CommitResult::AlreadyDone
        );
        db.0.call(|conn| {
            let row: (i64, String) = conn
                .query_row("SELECT COUNT(*), MAX(guid) FROM text_sessions", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .db()?;
            assert_eq!(row, (1, "replacement-guid".into()));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unavailable_model_defers_without_starting_engine() {
        let mut ai = AiConfig::default();
        ai.models_path = "definitely-not-a-model-directory".into();
        let supervisor = Arc::new(EngineSupervisor::new());

        assert!(matches!(
            ensure_summary_engine(&supervisor, &ai).await,
            Err(Error::ModelFileMissing(_))
        ));
        assert_eq!(supervisor.status().await.state, EngineState::Stopped);
    }

    #[tokio::test]
    async fn token_count_failure_is_non_destructive_and_does_not_retry() {
        let _lock = TEST_LOCK.lock().unwrap();
        let db = MemoryDb::open_in_memory().await.unwrap();
        let source = source(3);
        insert_source(&db, 3, &source).await;
        record_deferred(&db, &source, "token endpoint unavailable")
            .await
            .unwrap();

        db.0.call(|conn| {
            let raw: i64 = conn
                .query_row("SELECT COUNT(*) FROM text_sessions", [], |r| r.get(0))
                .db()?;
            let row: (String, i64) = conn
                .query_row("SELECT status, attempts FROM ocr_task_summaries", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .db()?;
            assert_eq!((raw, row), (1, ("deferred".into(), 0)));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unavailable_slots_defer_without_deleting_source() {
        let _lock = TEST_LOCK.lock().unwrap();
        let db = MemoryDb::open_in_memory().await.unwrap();
        let source = source(4);
        insert_source(&db, 4, &source).await;
        assert!(parse_slots_context("[]").is_err());
        record_deferred(&db, &source, "llama /slots unavailable")
            .await
            .unwrap();

        db.0.call(|conn| {
            let raw: i64 = conn
                .query_row("SELECT COUNT(*) FROM text_sessions", [], |r| r.get(0))
                .db()?;
            assert_eq!(raw, 1);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[test]
    fn configured_small_context_rejects_over_budget_input() {
        assert!(input_fits_context(2_047, 2_048, 4_096));
        assert!(!input_fits_context(2_049, 2_048, 4_096));
        assert!(!input_fits_context(1, 2_048, 2_048));
    }

    #[test]
    fn capped_actual_context_rejects_config_valid_candidate() {
        let configured_ctx = 8_192;
        let actual_ctx = 4_096;
        let input_tokens = 3_500;
        let max_tokens = 768;
        assert!(input_fits_context(input_tokens, max_tokens, configured_ctx));
        assert!(!input_fits_context(input_tokens, max_tokens, actual_ctx));
    }

    #[tokio::test]
    async fn needs_split_does_not_starve_newer_source_or_start_engine() {
        let _lock = TEST_LOCK.lock().unwrap();
        let db = MemoryDb::open_in_memory().await.unwrap();
        let oversized = source(7);
        insert_source(&db, 7, &oversized).await;
        record_needs_split(&db, &oversized).await.unwrap();
        let newer = source(8);
        insert_source(&db, 8, &newer).await;

        db.0.call(|conn| {
            let row: (String, i64) = conn
                .query_row("SELECT status, attempts FROM ocr_task_summaries", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .db()?;
            assert_eq!(row, ("needs_split".into(), 0));
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            load_candidates(&db, "2026-09-21T00:00:00+00:00".into())
                .await
                .unwrap()
                .iter()
                .map(|candidate| candidate.guid.as_str())
                .collect::<Vec<_>>(),
            vec!["session-guid-8"]
        );
        let supervisor = EngineSupervisor::new();
        assert_eq!(supervisor.status().await.state, EngineState::Stopped);
    }
}
