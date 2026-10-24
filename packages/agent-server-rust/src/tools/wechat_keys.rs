use super::exec::{exec_command, ExecOptions};
use super::wechat_db::{get_db_path, list_account_dbs};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;

/// Extract all WeChat DB credentials (async, non-blocking).
/// Calls the Python extract-keys script.
pub async fn extract_keys_async(wechat_pid: i64) -> HashMap<String, String> {
    extract_keys(wechat_pid, false).await
}

pub async fn prepare_core_data_async(wechat_pid: i64) -> HashMap<String, String> {
    extract_keys(wechat_pid, true).await
}

async fn extract_keys(wechat_pid: i64, core_only: bool) -> HashMap<String, String> {
    // All callers (login, chats and contacts) share one extraction per process.
    static EXTRACTION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = EXTRACTION.lock().await;
    let (Some(session), Some(account)) = (
        crate::sessions::manager::get_session("default"),
        super::wechat_db::find_account_dir(wechat_pid),
    ) else {
        return HashMap::new();
    };
    {
        let db = crate::db::get_db();
        if if core_only {
            core_data_ready(&db, &session.id, &account)
        } else {
            !needs_key_extraction(&db, &session.id, &account)
        } {
            return get_stored_keys(&db, &session.id, &account);
        }
    }
    let out_path = format!(
        "/tmp/wechat_keys_{}_{}.json",
        wechat_pid,
        uuid::Uuid::new_v4()
    );
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(out_path.clone());

    let pid = wechat_pid.to_string();
    let mut args = vec![
        "HOME=/home/wechat",
        "python3",
        "/opt/tools/extract-keys.py",
        "--pid",
        &pid,
        "--output",
        &out_path,
    ];
    if core_only {
        args.push("--core-only");
    }
    let result = exec_command(
        "env",
        &args,
        &ExecOptions {
            timeout_ms: 120_000,
            ..Default::default()
        },
    )
    .await;

    if result.exit_code != 0 {
        tracing::warn!(
            exit_code = result.exit_code,
            timed_out = result.stderr == "Command timed out",
            core_only,
            "WeChat data preparation did not finish cleanly"
        );
    }
    let result = (|| -> Option<HashMap<String, String>> {
        let content = std::fs::read_to_string(&out_path).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
        let keys = parsed.get("keys")?.as_object()?;
        let map: HashMap<String, String> = keys
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect();

        let db_keys: Vec<_> = map.keys().filter(|k| !k.starts_with('_')).collect();
        let has_image_aes = map.contains_key("_image_aes");
        tracing::info!(
            "[wechat-keys] Extracted {} DB keys, image key: {}",
            db_keys.len(),
            if has_image_aes { "yes" } else { "no" }
        );
        Some(map)
    })();

    // Clean up temp file
    let _ = std::fs::remove_file(&out_path);

    let result = result.unwrap_or_default();
    if super::wechat_db::find_account_dir(wechat_pid).as_ref() == Some(&account)
        && super::wechat_db::find_wechat_pid() == Some(wechat_pid)
    {
        let db = crate::db::get_db();
        store_keys(&db, &session.id, &account, &result);
    } else {
        tracing::warn!("WeChat identity changed during data preparation");
        return HashMap::new();
    }
    result
}

/// A UI login is usable only when core local stores can actually be opened.
/// Optional media/image credentials do not block the basic session.
pub fn core_data_ready(conn: &Connection, session: &str, account: &str) -> bool {
    let keys = get_stored_keys(conn, session, account);
    let mut required = vec!["session.db".to_string(), "contact.db".to_string()];
    required.extend(
        list_account_dbs(account)
            .into_iter()
            .filter(|name| name.starts_with("message_")),
    );
    let paths = required
        .into_iter()
        .map(|name| {
            let path = get_db_path(account, &name);
            (name, path)
        })
        .collect::<Vec<_>>();
    readable_stores(&keys, &paths)
}

fn readable_stores(keys: &HashMap<String, String>, paths: &[(String, String)]) -> bool {
    paths
        .iter()
        .all(|(name, path)| keys.get(name).is_some_and(|key| verify_key(path, key)))
}

#[cfg(test)]
mod readiness_tests {
    use super::*;
    #[test]
    fn readiness_requires_valid_core_credentials_but_not_conversations_or_media() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&directory).unwrap();
        let key = "11".repeat(32);
        let mut keys = HashMap::new();
        let paths=["session.db","contact.db"].map(|name| {
            let path=directory.join(name);
            let conn=Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{key}'\"; PRAGMA cipher_compatibility=4; CREATE TABLE empty_store(id INTEGER);")).unwrap();
            keys.insert(name.to_string(),key.clone());
            (name.to_string(),path.to_string_lossy().to_string())
        });
        assert!(readable_stores(&keys, &paths));
        keys.insert("contact.db".into(), "22".repeat(32));
        assert!(!readable_stores(&keys, &paths));
        keys.remove("contact.db");
        assert!(!readable_stores(&keys, &paths));
        std::fs::remove_dir_all(directory).unwrap();
    }
}

/// Get stored keys for a session + account from the agent DB.
pub fn get_stored_keys(
    conn: &Connection,
    session_id: &str,
    account_dir: &str,
) -> HashMap<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT db_name, hex_key FROM wechat_keys
             WHERE session_id = ?1 AND account_dir = ?2",
        )
        .unwrap();

    let rows = stmt
        .query_map(params![session_id, account_dir], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap();

    let mut map = HashMap::new();
    for row in rows.flatten() {
        map.insert(row.0, row.1);
    }
    map
}

/// Store extracted keys in the agent DB.
pub fn store_keys(
    conn: &Connection,
    session_id: &str,
    account_dir: &str,
    keys: &HashMap<String, String>,
) {
    let now = chrono::Utc::now().to_rfc3339();
    for (db_name, hex_key) in keys {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO wechat_keys (id, session_id, account_dir, db_name, hex_key, verified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id, account_dir, db_name) DO UPDATE SET
               hex_key = excluded.hex_key,
               verified_at = excluded.verified_at",
            params![id, session_id, account_dir, db_name, hex_key, now],
        )
        .ok();
    }
}

/// Store a single key.
pub fn store_single_key(
    conn: &Connection,
    session_id: &str,
    account_dir: &str,
    db_name: &str,
    hex_key: &str,
) {
    let mut keys = HashMap::new();
    keys.insert(db_name.to_string(), hex_key.to_string());
    store_keys(conn, session_id, account_dir, &keys);
}

/// Verify a single key against a database file.
/// Opens with immutable=1 to avoid acquiring any locks that could interfere
/// with WeChat's own writes/checkpoints.
pub fn verify_key(db_path: &str, hex_key: &str) -> bool {
    let uri = format!("file:{}?immutable=1", db_path);
    let conn = match Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };

    if conn
        .execute_batch(&format!(
            "PRAGMA key = \"x'{hex_key}'\"; PRAGMA cipher_compatibility = 4;"
        ))
        .is_err()
    {
        return false;
    }

    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .is_ok()
}

/// Check if credential setup is needed.
///
/// 1. Check that all required DB keys exist in stored_keys
/// 2. Scan disk for additional required DBs (message_N, media_N) without keys
/// 3. Spot-check one key for validity
pub fn needs_key_extraction(conn: &Connection, session_id: &str, account_dir: &str) -> bool {
    let stored_keys = get_stored_keys(conn, session_id, account_dir);

    if stored_keys.is_empty() {
        tracing::info!("[wechat-keys] No stored keys, extraction needed");
        return true;
    }

    let required_exact: &[&str] = &[
        "session.db",
        "contact.db",
        "emoticon.db",
        "head_image.db",
        "hardlink.db",
    ];
    let required_prefixes: &[&str] = &["message_", "media_"];

    // Check required exact keys are present (regardless of disk scan)
    let missing_required: Vec<&str> = required_exact
        .iter()
        .filter(|name| !stored_keys.contains_key(**name))
        .copied()
        .collect();

    if !missing_required.is_empty() {
        tracing::info!(
            "[wechat-keys] Missing required keys: {}",
            missing_required.join(", ")
        );
        return true;
    }

    // Scan disk for sharded DBs (message_N.db, media_N.db) missing keys
    let existing_dbs = list_account_dbs(account_dir);
    tracing::debug!(
        "[wechat-keys] Disk scan: {} files for account {}",
        existing_dbs.len(),
        account_dir
    );

    let missing_on_disk: Vec<_> = existing_dbs
        .iter()
        .filter(|name| {
            required_prefixes.iter().any(|p| name.starts_with(p))
                && !stored_keys.contains_key(name.as_str())
        })
        .collect();

    if !missing_on_disk.is_empty() {
        tracing::info!(
            "[wechat-keys] Missing keys for on-disk DBs: {}",
            missing_on_disk
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return true;
    }

    // Spot-check one key
    let check_db = "session.db";
    if let Some(check_key) = stored_keys.get(check_db) {
        let check_path = get_db_path(account_dir, check_db);
        if !verify_key(&check_path, check_key) {
            tracing::info!("[wechat-keys] Spot-check failed for {check_db}, re-extraction needed");
            return true;
        }
    }

    let db_key_count = stored_keys.keys().filter(|k| !k.starts_with('_')).count();
    tracing::info!(
        "[wechat-keys] {db_key_count} DB keys stored, spot-check passed, image_aes={}",
        stored_keys.contains_key("_image_aes")
    );
    false
}

/// Get stored image keys.
pub fn get_image_keys(
    conn: &Connection,
    session_id: &str,
    account_dir: &str,
) -> Option<(String, Option<u8>)> {
    let keys = get_stored_keys(conn, session_id, account_dir);
    let aes_hex = keys.get("_image_aes")?;
    let xor_byte = keys
        .get("_image_xor")
        .and_then(|h| u8::from_str_radix(h, 16).ok());
    Some((aes_hex.clone(), xor_byte))
}
