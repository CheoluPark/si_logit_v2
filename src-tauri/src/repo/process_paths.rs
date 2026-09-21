//! `process_paths` 表：从 process_name 到 exe 绝对路径的映射。
//! 用于本机图标提取（GDI / plist 需要 exe 路径）+ 调试。
//!
//! capture 路径每次见到一个进程时刷新当前 exe 路径。

use chrono::Local;

use crate::error::Result;
use crate::storage::{utc_now_rfc3339, DbPool, SqliteResultExt};

/// 登记 / 更新某 process_name 对应的 exe 路径。
pub async fn upsert(pool: &DbPool, process_name: &str, exe_path: &str) -> Result<()> {
    let p = process_name.to_string();
    let e = exe_path.to_string();
    let seen = Local::now().to_rfc3339();
    let updated = utc_now_rfc3339();
    pool.0
        .call(move |conn| {
            conn.execute(
                "INSERT INTO process_paths(process_name, exe_path, seen_at, updated_at)
                 VALUES(?, ?, ?, ?)
                 ON CONFLICT(process_name) DO UPDATE SET
                   exe_path = excluded.exe_path,
                   seen_at = excluded.seen_at,
                   updated_at = excluded.updated_at",
                rusqlite::params![p, e, seen, updated],
            )
            .db()?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// 查某 process_name 当前的 exe 路径；表里没有返回 None。
pub async fn get_path(pool: &DbPool, process_name: &str) -> Result<Option<String>> {
    let p = process_name.to_string();
    let path = pool
        .0
        .call(move |conn| {
            let r = conn
                .query_row(
                    "SELECT exe_path FROM process_paths WHERE process_name = ?",
                    [&p],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            Ok(r)
        })
        .await?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util::fresh_test_pool;

    #[tokio::test]
    async fn upsert_then_get_roundtrip_and_update() {
        let pool = fresh_test_pool().await;
        assert_eq!(get_path(&pool, "Code").await.unwrap(), None);

        upsert(&pool, "Code", "/apps/Code.app").await.unwrap();
        assert_eq!(
            get_path(&pool, "Code").await.unwrap().as_deref(),
            Some("/apps/Code.app")
        );

        // 路径变更被更新;重复写同路径幂等不报错
        upsert(&pool, "Code", "/newpath/Code.app").await.unwrap();
        upsert(&pool, "Code", "/newpath/Code.app").await.unwrap();
        assert_eq!(
            get_path(&pool, "Code").await.unwrap().as_deref(),
            Some("/newpath/Code.app")
        );
    }
}
