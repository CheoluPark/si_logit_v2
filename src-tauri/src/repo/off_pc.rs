//! Repository mutation for manually entered off-PC activity.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Timelike, Utc};
use rusqlite::{OptionalExtension, Transaction};
use serde::Serialize;

use crate::device;
use crate::error::{Error, Result};
use crate::storage::{utc_now_rfc3339, DbPool, SqliteResultExt};

const OFF_PC_CATEGORY_NAME: &str = "PC 외 업무";
const OFF_PC_CATEGORY_COLOR: &str = "#94a3b8";
const OFF_PC_CATEGORY_ICON: &str = "Briefcase";
const OFF_PC_SUPER_CATEGORY_ID: &str = "off_pc";
const OFF_PC_SUPER_CATEGORY_NAME: &str = "PC 외 업무";
const OFF_PC_SUPER_CATEGORY_COLOR: &str = "#94a3b8";
const OFF_PC_SUPER_CATEGORY_ICON: &str = "Briefcase";

/// Stable values accepted by [`record_off_pc`].
pub const MEETING: &str = "meeting";
pub const BUSINESS_TRIP: &str = "business_trip";
pub const EXAM: &str = "exam";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPcGap {
    pub from: String,
    pub to: String,
}

/// Return recordable gaps observed on the current device for one local date.
/// All activities are coverage here: report filters such as `excluded` and
/// category joins intentionally do not apply.
pub async fn recordable_off_pc_gaps(
    pool: &DbPool,
    local_date: NaiveDate,
    device_id: String,
) -> Result<Vec<OffPcGap>> {
    let self_id = device::self_id()?.to_string();
    if device_id != self_id {
        return Err(Error::InvalidInput(
            "off-PC gaps must target the current device",
        ));
    }

    let today = Local::now().date_naive();
    if local_date > today {
        return Ok(Vec::new());
    }
    let day_start = Local
        .with_ymd_and_hms(
            local_date.year(),
            local_date.month(),
            local_date.day(),
            0,
            0,
            0,
        )
        .earliest()
        .expect("local day start should exist");
    let day_end_date = local_date + Duration::days(1);
    let day_end = Local
        .with_ymd_and_hms(
            day_end_date.year(),
            day_end_date.month(),
            day_end_date.day(),
            0,
            0,
            0,
        )
        .latest()
        .expect("local day end should exist");
    let cutoff = if local_date == today {
        Local::now()
    } else {
        day_end
    };
    let window_start = day_start.with_timezone(&Utc);
    let cutoff = cutoff.with_timezone(&Utc);
    if cutoff <= window_start {
        return Ok(Vec::new());
    }

    let day_start_param = day_start.to_rfc3339();
    let day_end_param = day_end.to_rfc3339();
    let rows: Vec<(String, String)> = pool
        .0
        .call(move |conn| {
            // This is deliberately independent of report classification/filter joins:
            // excluded, hidden, categoryless, and synthetic rows all occupy coverage.
            let mut stmt = conn
                .prepare(
                    "SELECT started_at, ended_at FROM activities
                     WHERE device_id = ?1
                       AND (
                           (
                               julianday(started_at) < julianday(?2)
                               AND julianday(ended_at) > julianday(?3)
                           )
                           OR (
                               julianday(started_at) = julianday(ended_at)
                               AND julianday(started_at) >= julianday(?3)
                               AND julianday(started_at) < julianday(?2)
                           )
                       )",
                )
                .db()?;
            let it = stmt
                .query_map(
                    rusqlite::params![&device_id, &day_end_param, &day_start_param],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .db()?;
            let mut out = Vec::new();
            for row in it {
                out.push(row.db()?);
            }
            Ok(out)
        })
        .await?;

    let mut coverage: Vec<(DateTime<Utc>, DateTime<Utc>)> = rows
        .into_iter()
        .filter_map(|(started_at, ended_at)| {
            let started_at = DateTime::parse_from_rfc3339(&started_at)
                .ok()?
                .with_timezone(&Utc)
                .max(window_start);
            let ended_at = DateTime::parse_from_rfc3339(&ended_at)
                .ok()?
                .with_timezone(&Utc)
                .min(cutoff);
            (started_at <= ended_at).then_some((started_at, ended_at))
        })
        .collect();
    coverage.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let Some((first_start, first_end)) = coverage.first().copied() else {
        return Ok(Vec::new());
    };
    let mut merged = vec![(first_start, first_end)];
    for (start, end) in coverage.into_iter().skip(1) {
        let current = merged.last_mut().expect("merged has first interval");
        if start <= current.1 {
            current.1 = current.1.max(end);
        } else {
            merged.push((start, end));
        }
    }

    let mut gaps = Vec::new();
    let mut cursor = first_start;
    for (start, end) in merged {
        if let Some(gap) = canonical_gap(cursor, start) {
            gaps.push(gap);
        }
        cursor = cursor.max(end);
    }
    if let Some(gap) = canonical_gap(cursor, cutoff) {
        gaps.push(gap);
    }
    Ok(gaps)
}

fn canonical_gap(from: DateTime<Utc>, to: DateTime<Utc>) -> Option<OffPcGap> {
    let from = ceil_second(from);
    let to = floor_second(to);
    (from < to).then(|| OffPcGap {
        from: from.to_rfc3339(),
        to: to.to_rfc3339(),
    })
}

fn floor_second(value: DateTime<Utc>) -> DateTime<Utc> {
    value - Duration::nanoseconds(i64::from(value.nanosecond()))
}

fn ceil_second(value: DateTime<Utc>) -> DateTime<Utc> {
    let floored = floor_second(value);
    if floored == value {
        floored
    } else {
        floored + Duration::seconds(1)
    }
}

/// Record one exact, currently-local gap as a sealed synthetic activity.
pub async fn record_off_pc(
    pool: &DbPool,
    device_id: String,
    from: String,
    to: String,
    activity_type: String,
    detail: String,
) -> Result<i64> {
    let self_id = device::self_id()?.to_string();
    if device_id != self_id {
        return Err(Error::InvalidInput(
            "off-PC activity must target the current device",
        ));
    }

    let label = match activity_type.as_str() {
        MEETING => "Meeting",
        BUSINESS_TRIP => "Business trip",
        EXAM => "Satellite module functional test",
        _ => return Err(Error::InvalidInput("unsupported off-PC activity type")),
    };
    let detail = detail.trim().to_string();
    if detail.is_empty() {
        return Err(Error::InvalidInput(
            "off-PC activity detail must not be empty",
        ));
    }

    let from_dt = DateTime::parse_from_rfc3339(&from)
        .map_err(|_| Error::InvalidInput("off-PC start must be RFC3339"))?
        .with_timezone(&Utc);
    let to_dt = DateTime::parse_from_rfc3339(&to)
        .map_err(|_| Error::InvalidInput("off-PC end must be RFC3339"))?
        .with_timezone(&Utc);
    let span = to_dt - from_dt;
    let duration_secs = span.num_seconds();
    if duration_secs <= 0 || span != chrono::Duration::seconds(duration_secs) {
        return Err(Error::InvalidInput("off-PC interval must be non-empty"));
    }
    if to_dt > Utc::now() {
        return Err(Error::InvalidInput(
            "off-PC interval must not be in the future",
        ));
    }

    let started_at = from_dt.to_rfc3339();
    let ended_at = to_dt.to_rfc3339();
    let local_start = from_dt.with_timezone(&Local);
    let local_end_minus_ns = (to_dt - Duration::nanoseconds(1)).with_timezone(&Local);
    if local_start.date_naive() != local_end_minus_ns.date_naive() {
        return Err(Error::InvalidInput(
            "off-PC interval must stay within one local calendar date",
        ));
    }
    let local_date = local_start.format("%Y-%m-%d").to_string();
    let local_hour = local_start.hour() as i64;
    let process_name = format!("off_pc::{activity_type}");
    let label = label.to_string();
    let updated_at = utc_now_rfc3339();

    let result = pool
        .0
        .call(move |conn| {
            let tx = conn.transaction().db()?;

            let overlaps: bool = tx
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM activities
                         WHERE device_id = ?1
                           AND julianday(started_at) < julianday(?2)
                           AND julianday(ended_at) > julianday(?3)
                     )",
                    rusqlite::params![&device_id, &ended_at, &started_at],
                    |r| r.get(0),
                )
                .db()?;
            if overlaps {
                return Ok(Err(Error::InvalidInput(
                    "off-PC interval overlaps captured activity",
                )));
            }

            ensure_off_pc_supercategory(&tx, &updated_at).db()?;
            let category_id = ensure_off_pc_category(&tx, &updated_at).db()?;
            ensure_manual_group(&tx, &process_name, &label, &category_id, &updated_at).db()?;

            tx.execute(
                "INSERT INTO activities(
                     started_at, ended_at, duration_secs, local_date, local_hour,
                     process_name, window_title, category_id, device_id,
                     updated_at, origin, excluded, url_host
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, 'other', ?, ?, 'local', 0, NULL)",
                rusqlite::params![
                    &started_at,
                    &ended_at,
                    duration_secs,
                    &local_date,
                    local_hour,
                    &process_name,
                    &detail,
                    &device_id,
                    &updated_at,
                ],
            )
            .db()?;
            let activity_id = tx.last_insert_rowid();
            tx.commit().db()?;
            Ok(Ok(activity_id))
        })
        .await?;
    result
}

fn ensure_off_pc_supercategory(tx: &Transaction<'_>, updated_at: &str) -> rusqlite::Result<()> {
    let existing: Option<(String, String, String, Option<String>)> = tx
        .query_row(
            "SELECT name, color, icon, deleted_at FROM super_categories WHERE id = ?",
            rusqlite::params![OFF_PC_SUPER_CATEGORY_ID],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    match existing {
        None => {
            let sort_order: i64 = tx.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1
                 FROM super_categories WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO super_categories(
                     id, name, color, icon, sort_order, updated_at, deleted_at
                 ) VALUES (?, ?, ?, ?, ?, ?, NULL)",
                rusqlite::params![
                    OFF_PC_SUPER_CATEGORY_ID,
                    OFF_PC_SUPER_CATEGORY_NAME,
                    OFF_PC_SUPER_CATEGORY_COLOR,
                    OFF_PC_SUPER_CATEGORY_ICON,
                    sort_order,
                    updated_at,
                ],
            )?;
        }
        Some((name, color, icon, deleted_at))
            if deleted_at.is_some()
                || name != OFF_PC_SUPER_CATEGORY_NAME
                || color != OFF_PC_SUPER_CATEGORY_COLOR
                || icon != OFF_PC_SUPER_CATEGORY_ICON =>
        {
            tx.execute(
                "UPDATE super_categories
                 SET name = ?, color = ?, icon = ?, updated_at = ?, deleted_at = NULL
                 WHERE id = ?",
                rusqlite::params![
                    OFF_PC_SUPER_CATEGORY_NAME,
                    OFF_PC_SUPER_CATEGORY_COLOR,
                    OFF_PC_SUPER_CATEGORY_ICON,
                    updated_at,
                    OFF_PC_SUPER_CATEGORY_ID,
                ],
            )?;
        }
        Some(_) => {}
    }
    Ok(())
}

fn ensure_off_pc_category(tx: &Transaction<'_>, updated_at: &str) -> rusqlite::Result<String> {
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM categories
             WHERE name = ?1 AND super_category_id = ?2 AND deleted_at IS NULL
             ORDER BY id LIMIT 1",
            rusqlite::params![OFF_PC_CATEGORY_NAME, OFF_PC_SUPER_CATEGORY_ID],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(id);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let sort_order: i64 = tx.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM categories WHERE deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO categories(
             id, name, color, icon, builtin, sort_order, super_category_id, updated_at
         ) VALUES (?, ?, ?, ?, 0, ?, ?, ?)",
        rusqlite::params![
            &id,
            OFF_PC_CATEGORY_NAME,
            OFF_PC_CATEGORY_COLOR,
            OFF_PC_CATEGORY_ICON,
            sort_order,
            OFF_PC_SUPER_CATEGORY_ID,
            updated_at,
        ],
    )?;
    Ok(id)
}

fn ensure_manual_group(
    tx: &Transaction<'_>,
    process_name: &str,
    display_name: &str,
    category_id: &str,
    updated_at: &str,
) -> rusqlite::Result<()> {
    let group: Option<(String, Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT display_name, category_id, deleted_at FROM app_groups WHERE id = ?",
            rusqlite::params![process_name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let _group_changed = match group {
        None => {
            tx.execute(
                "INSERT INTO app_groups(id, display_name, category_id, updated_at, deleted_at)
                 VALUES (?, ?, ?, ?, NULL)",
                rusqlite::params![process_name, display_name, category_id, updated_at],
            )?;
            true
        }
        Some((current_name, current_category, deleted_at))
            if current_name != display_name
                || current_category.as_deref() != Some(category_id)
                || deleted_at.is_some() =>
        {
            tx.execute(
                "UPDATE app_groups
                 SET display_name = ?, category_id = ?, updated_at = ?, deleted_at = NULL
                 WHERE id = ?",
                rusqlite::params![display_name, category_id, updated_at, process_name],
            )?;
            true
        }
        Some(_) => false,
    };

    let member: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT group_id, deleted_at FROM app_group_members WHERE process_name = ?",
            rusqlite::params![process_name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let _member_changed = match member {
        None => {
            tx.execute(
                "INSERT INTO app_group_members(process_name, group_id, updated_at, deleted_at)
                 VALUES (?, ?, ?, NULL)",
                rusqlite::params![process_name, process_name, updated_at],
            )?;
            true
        }
        Some((current_group, deleted_at))
            if current_group != process_name || deleted_at.is_some() =>
        {
            tx.execute(
                "UPDATE app_group_members
                 SET group_id = ?, updated_at = ?, deleted_at = NULL
                 WHERE process_name = ?",
                rusqlite::params![process_name, updated_at, process_name],
            )?;
            true
        }
        Some(_) => false,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util::{fresh_test_pool, TEST_SELF_ID};
    use crate::storage::SqliteResultExt;
    use chrono::{Datelike, TimeZone};

    fn interval_at(start_hour: u32) -> (String, String) {
        let day = Local::now().date_naive() - Duration::days(2);
        let from = Local
            .with_ymd_and_hms(day.year(), day.month(), day.day(), start_hour, 0, 0)
            .single()
            .unwrap();
        let to = from + Duration::hours(1);
        (from.to_rfc3339(), to.to_rfc3339())
    }

    fn interval() -> (String, String) {
        interval_at(10)
    }

    async fn scalar(pool: &DbPool, sql: &str) -> i64 {
        let sql = sql.to_string();
        pool.0
            .call(move |conn| conn.query_row(&sql, [], |r| r.get(0)).db())
            .await
            .unwrap()
    }

    fn local_time(day: NaiveDate, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(day.year(), day.month(), day.day(), hour, minute, 0)
            .single()
            .unwrap()
    }

    async fn insert_raw_activity(
        pool: &DbPool,
        device_id: &str,
        started_at: DateTime<Local>,
        ended_at: DateTime<Local>,
        excluded: bool,
    ) {
        let device_id = device_id.to_string();
        let started = started_at.to_rfc3339();
        let ended = ended_at.to_rfc3339();
        let local_date = started_at.format("%Y-%m-%d").to_string();
        let local_hour = started_at.hour() as i64;
        pool.0
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO activities(
                         started_at, ended_at, duration_secs, local_date, local_hour,
                         process_name, window_title, category_id, device_id,
                         updated_at, origin, excluded
                     ) VALUES (?, ?, ?, ?, ?, 'Captured', '', 'other', ?, ?, 'local', ?)",
                    rusqlite::params![
                        started,
                        ended,
                        (ended_at - started_at).num_seconds(),
                        local_date,
                        local_hour,
                        device_id,
                        ended,
                        excluded,
                    ],
                )
                .db()?;
                Ok(())
            })
            .await
            .unwrap();
    }

    fn gap_overlaps(gap: &OffPcGap, start: DateTime<Utc>, end: DateTime<Utc>) -> bool {
        let gap_start = DateTime::parse_from_rfc3339(&gap.from)
            .unwrap()
            .with_timezone(&Utc);
        let gap_end = DateTime::parse_from_rfc3339(&gap.to)
            .unwrap()
            .with_timezone(&Utc);
        gap_start < end && gap_end > start
    }

    #[tokio::test]
    async fn gaps_use_excluded_and_categoryless_activity_as_coverage() {
        let pool = fresh_test_pool().await;
        let day = Local::now().date_naive() - Duration::days(2);
        let started = local_time(day, 10, 0);
        let ended = local_time(day, 11, 0);
        insert_raw_activity(&pool, TEST_SELF_ID, started, ended, true).await;

        let gaps = recordable_off_pc_gaps(&pool, day, TEST_SELF_ID.into())
            .await
            .unwrap();
        assert!(!gaps.iter().any(|gap| {
            gap_overlaps(gap, started.with_timezone(&Utc), ended.with_timezone(&Utc))
        }));
    }

    #[tokio::test]
    async fn gaps_clip_to_today_cutoff_and_cover_cross_date_activity() {
        let pool = fresh_test_pool().await;
        let now = Local::now();
        let started = now - Duration::minutes(20);
        let ended = now - Duration::minutes(10);
        insert_raw_activity(&pool, TEST_SELF_ID, started, ended, false).await;
        let before = Utc::now();
        let gaps = recordable_off_pc_gaps(&pool, now.date_naive(), TEST_SELF_ID.into())
            .await
            .unwrap();
        assert!(!gaps.is_empty());
        let after = Utc::now();
        for gap in &gaps {
            let gap_end = DateTime::parse_from_rfc3339(&gap.to)
                .unwrap()
                .with_timezone(&Utc);
            assert!(gap_end <= after);
        }
        assert!(gaps.iter().any(|gap| {
            let gap_end = DateTime::parse_from_rfc3339(&gap.to)
                .unwrap()
                .with_timezone(&Utc);
            gap_end >= before - Duration::seconds(1)
        }));

        let previous_day = now.date_naive() - Duration::days(1);
        let crossing_start = local_time(previous_day, 23, 50);
        let crossing_end = local_time(now.date_naive(), 0, 10);
        let boundary_pool = fresh_test_pool().await;
        insert_raw_activity(
            &boundary_pool,
            TEST_SELF_ID,
            crossing_start,
            crossing_end,
            false,
        )
        .await;
        let boundary_gaps =
            recordable_off_pc_gaps(&boundary_pool, now.date_naive(), TEST_SELF_ID.into())
                .await
                .unwrap();
        assert!(!boundary_gaps.iter().any(|gap| {
            gap_overlaps(
                gap,
                local_time(now.date_naive(), 0, 0).with_timezone(&Utc),
                crossing_end.with_timezone(&Utc),
            )
        }));
    }

    #[tokio::test]
    async fn gaps_reject_remote_and_return_empty_for_future_dates() {
        let pool = fresh_test_pool().await;
        let today = Local::now().date_naive();
        assert!(recordable_off_pc_gaps(&pool, today, "remote-device".into())
            .await
            .is_err());
        assert!(
            recordable_off_pc_gaps(&pool, today + Duration::days(1), TEST_SELF_ID.into())
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn canonical_gap_after_fractional_zero_length_activity_is_recordable() {
        let pool = fresh_test_pool().await;
        let now = Local::now();
        let zero = now - Duration::minutes(5);
        insert_raw_activity(&pool, TEST_SELF_ID, zero, zero, false).await;
        let gaps = recordable_off_pc_gaps(&pool, now.date_naive(), TEST_SELF_ID.into())
            .await
            .unwrap();
        let gap = gaps
            .iter()
            .find(|gap| {
                DateTime::parse_from_rfc3339(&gap.from)
                    .unwrap()
                    .with_timezone(&Utc)
                    >= zero.with_timezone(&Utc)
            })
            .cloned()
            .expect("zero-length activity should establish an observation start");

        let id = record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            gap.from,
            gap.to,
            EXAM.into(),
            "recorded from certified gap".into(),
        )
        .await
        .unwrap();
        assert!(id > 0);
    }

    #[tokio::test]
    async fn records_exact_duration() {
        let pool = fresh_test_pool().await;
        let (from, to) = interval();
        let id = record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            from.clone(),
            to,
            MEETING.into(),
            "planning meeting".into(),
        )
        .await
        .unwrap();

        let duration = scalar(
            &pool,
            &format!("SELECT duration_secs FROM activities WHERE id = {id}"),
        )
        .await;
        assert_eq!(duration, 3600);
        assert_eq!(scalar(&pool, "SELECT COUNT(*) FROM activities").await, 1);
    }

    #[tokio::test]
    async fn rejects_overlap_future_and_non_local_device() {
        let pool = fresh_test_pool().await;
        let (from, to) = interval();
        pool.0
            .call({
                let from = from.clone();
                let to = to.clone();
                move |conn| {
                    conn.execute(
                        "INSERT INTO activities(
                             started_at, ended_at, duration_secs, local_date, local_hour,
                             process_name, window_title, category_id, device_id,
                             updated_at, origin, excluded
                         ) VALUES (?, ?, 60, '2026-01-01', 0, 'Captured', '', 'other', ?, ?, 'local', 0)",
                        rusqlite::params![from, to, TEST_SELF_ID, to],
                    )
                    .db()?;
                    Ok(())
                }
            })
            .await
            .unwrap();
        assert!(record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            from.clone(),
            to.clone(),
            MEETING.into(),
            "overlap".into(),
        )
        .await
        .is_err());
        pool.0
            .call(|conn| {
                conn.execute("UPDATE activities SET excluded = 1", [])
                    .db()?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            record_off_pc(
                &pool,
                TEST_SELF_ID.into(),
                from.clone(),
                to.clone(),
                MEETING.into(),
                "excluded overlap".into(),
            )
            .await
            .is_err(),
            "excluded captured activity still occupies a raw gap"
        );
        let future_from = Utc::now() + chrono::Duration::minutes(1);
        let future_to = future_from + chrono::Duration::minutes(5);
        assert!(record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            future_from.to_rfc3339(),
            future_to.to_rfc3339(),
            MEETING.into(),
            "future".into(),
        )
        .await
        .is_err());
        assert!(record_off_pc(
            &pool,
            "other-device".into(),
            from,
            to,
            MEETING.into(),
            "wrong device".into(),
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn compatible_records_reuse_classified_manual_artifacts() {
        let pool = fresh_test_pool().await;
        let (from, to) = interval_at(10);
        record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            from,
            to,
            MEETING.into(),
            "first meeting".into(),
        )
        .await
        .unwrap();
        let category_id: String = pool
            .0
            .call(|conn| {
                conn.query_row(
                    "SELECT id FROM categories WHERE name = 'PC 외 업무' AND super_category_id = 'off_pc'",
                    [],
                    |r| r.get(0),
                )
                .db()
            })
            .await
            .unwrap();

        let (from2, to2) = interval_at(12);
        record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            from2,
            to2,
            MEETING.into(),
            "second meeting".into(),
        )
        .await
        .unwrap();

        assert_eq!(
            scalar(
                &pool,
                "SELECT COUNT(*) FROM categories WHERE name = 'PC 외 업무' AND super_category_id = 'off_pc'",
            )
            .await,
            1
        );
        assert_eq!(
            scalar(
                &pool,
                "SELECT COUNT(*) FROM super_categories WHERE id = 'off_pc' AND name = 'PC 외 업무' AND deleted_at IS NULL",
            )
            .await,
            1
        );
        assert_eq!(
            scalar(
                &pool,
                "SELECT COUNT(*) FROM app_groups WHERE id = 'off_pc::meeting'",
            )
            .await,
            1
        );
        assert_eq!(
            scalar(
                &pool,
                "SELECT COUNT(*) FROM app_group_members WHERE process_name = 'off_pc::meeting'",
            )
            .await,
            1
        );
        let group_category: String = pool
            .0
            .call(|conn| {
                conn.query_row(
                    "SELECT category_id FROM app_groups WHERE id = 'off_pc::meeting'",
                    [],
                    |r| r.get(0),
                )
                .db()
            })
            .await
            .unwrap();
        assert_eq!(group_category, category_id);

        let day = Local::now().date_naive() - Duration::days(2);
        let rows = crate::repo::reports::timeline_sessions(
            &pool,
            day,
            crate::repo::reports::DeviceFilter::Only(TEST_SELF_ID.into()),
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.category_id == category_id));
    }

    #[tokio::test]
    async fn rejects_cross_date_interval_but_accepts_midnight_end() {
        let pool = fresh_test_pool().await;
        let day = Local::now().date_naive() - Duration::days(2);
        let before_midnight = Local
            .with_ymd_and_hms(day.year(), day.month(), day.day(), 23, 0, 0)
            .single()
            .unwrap();
        let after_midnight = before_midnight + Duration::hours(2);
        assert!(record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            before_midnight.to_rfc3339(),
            after_midnight.to_rfc3339(),
            MEETING.into(),
            "crosses date".into(),
        )
        .await
        .is_err());

        let midnight = before_midnight + Duration::hours(1);
        let id = record_off_pc(
            &pool,
            TEST_SELF_ID.into(),
            (midnight - Duration::hours(1)).to_rfc3339(),
            midnight.to_rfc3339(),
            BUSINESS_TRIP.into(),
            "ends at midnight".into(),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(
                &pool,
                &format!("SELECT duration_secs FROM activities WHERE id = {id}"),
            )
            .await,
            3600
        );
    }
}
