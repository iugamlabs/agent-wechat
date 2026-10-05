use super::wechat_db::{get_db_path, query_wechat_db};
use crate::ia::types::Contact;
use std::collections::HashMap;

/// Known system/internal usernames to exclude from contact listings.
const SYSTEM_USERNAMES: &[&str] = &[
    "qmessage",
    "floatbottle",
    "medianote",
    "notifymessage",
    "weixin",
    "fmessage",
    "filehelper",
    "newsapp",
    "tmessage",
    "mphelper",
    "qqmail",
    "weixingongzhong",
    "qqsafe",
    "exmail_tool",
    "lbsapp",
    "pc_qq",
];

fn classify_contact(username: &str, local_type: i64) -> &'static str {
    if username.ends_with("@chatroom") {
        "chatroom"
    } else if username.starts_with("gh_") {
        "official"
    } else if username.contains("@openim") || local_type == 5 {
        "openim"
    } else {
        "individual"
    }
}

fn is_system_account(username: &str) -> bool {
    SYSTEM_USERNAMES.contains(&username)
}

/// Keep list and find within the supported non-chatroom contact scope.
fn contact_query(limit: i64, offset: i64, query: Option<&str>, local_type: Option<i64>) -> String {
    let type_filter = match local_type {
        Some(value) => format!("local_type IN (1, 3, 5) AND local_type = {value}"),
        None => "local_type IN (1, 3, 5)".to_string(),
    };
    let name_filter = query
        .map(|query| {
            let escaped = query.replace('\'', "''");
            format!(
                "AND (nick_name LIKE '%{escaped}%' OR remark LIKE '%{escaped}%'
                 OR alias LIKE '%{escaped}%' OR username LIKE '%{escaped}%')"
            )
        })
        .unwrap_or_default();
    format!(
        "SELECT username, nick_name, remark, alias, small_head_url, local_type
             FROM contact WHERE {type_filter} AND username NOT LIKE '%@chatroom'
             {name_filter}
             ORDER BY remark != '' DESC, nick_name COLLATE NOCASE ASC
             LIMIT {limit} OFFSET {offset};"
    )
}

fn contacts_from_rows(rows: &[serde_json::Value]) -> Vec<Contact> {
    rows.iter()
        .filter_map(|row| {
            let username = row.get("username")?.as_str()?;
            if is_system_account(username) {
                return None;
            }

            let local_type = row.get("local_type").and_then(|v| v.as_i64()).unwrap_or(3);

            Some(Contact {
                username: username.to_string(),
                nick_name: row
                    .get("nick_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                remark: row
                    .get("remark")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                alias: row
                    .get("alias")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                small_head_url: row
                    .get("small_head_url")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                contact_type: classify_contact(username, local_type).to_string(),
                local_type,
            })
        })
        .collect()
}

/// List stored contacts of types 1, 3 and 5, excluding chatrooms and system accounts.
pub fn list_contacts(
    account_dir: &str,
    keys: &HashMap<String, String>,
    limit: i64,
    offset: i64,
    local_type: Option<i64>,
) -> Vec<Contact> {
    query_contacts(
        account_dir,
        keys,
        &contact_query(limit, offset, None, local_type),
    )
}

/// Search contacts by partial nick_name, remark, alias or username.
pub fn find_contacts(
    account_dir: &str,
    keys: &HashMap<String, String>,
    query: &str,
    local_type: Option<i64>,
) -> Vec<Contact> {
    query_contacts(
        account_dir,
        keys,
        &contact_query(50, 0, Some(query), local_type),
    )
}

fn query_contacts(account_dir: &str, keys: &HashMap<String, String>, query: &str) -> Vec<Contact> {
    let Some(key) = keys.get("contact.db") else {
        return Vec::new();
    };
    let rows = query_wechat_db(&get_db_path(account_dir, "contact.db"), key, query);
    contacts_from_rows(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(query: Option<&str>, local_type: Option<i64>) -> Vec<Contact> {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE contact (username TEXT, nick_name TEXT, remark TEXT,
            alias TEXT, small_head_url TEXT, local_type INTEGER);
            INSERT INTO contact VALUES
            ('friend', 'Alice', '', '', '', 1), ('stranger', 'Alice', '', '', '', 3),
            ('enterprise', 'Alice', '', '', '', 5), ('gh_official', 'Alice', '', '', '', 1),
            ('peer@openim', 'Alice', '', '', '', 3), ('weixin', 'Alice', '', '', '', 1),
            ('system', 'Alice', '', '', '', 0), ('group@chatroom', 'Alice', '', '', '', 2),
            ('odd@chatroom', 'Alice', '', '', '', 1);",
        )
        .unwrap();
        let mut stmt = db
            .prepare(&contact_query(200, 0, query, local_type))
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({
                    "username": r.get::<_, String>(0)?, "nick_name": r.get::<_, String>(1)?,
                    "remark": r.get::<_, String>(2)?, "alias": r.get::<_, String>(3)?,
                    "small_head_url": r.get::<_, String>(4)?, "local_type": r.get::<_, i64>(5)?
                }))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        contacts_from_rows(&rows)
    }

    #[test]
    fn retains_raw_type_and_existing_classification() {
        let contacts = fixture(None, None);
        assert_eq!(contacts.len(), 5);
        for (username, kind, raw) in [
            ("friend", "individual", 1),
            ("stranger", "individual", 3),
            ("enterprise", "openim", 5),
            ("gh_official", "official", 1),
            ("peer@openim", "openim", 3),
        ] {
            let c = contacts.iter().find(|c| c.username == username).unwrap();
            let value = serde_json::to_value(c).unwrap();
            assert_eq!(value["contactType"], kind);
            assert_eq!(value["localType"], raw);
        }
    }

    #[test]
    fn list_and_find_filter_without_expanding_scope() {
        for query in [None, Some("Alice")] {
            for (kind, count) in [(1, 2), (3, 2), (5, 1)] {
                let contacts = fixture(query, Some(kind));
                assert_eq!(contacts.len(), count);
                assert!(contacts.iter().all(|c| c.local_type == kind));
            }
        }
        assert!(fixture(Some("does-not-exist"), None).is_empty());
        assert!(fixture(Some("' OR 1=1 --"), None).is_empty());
        assert!(fixture(None, Some(2)).is_empty());
    }
}
