use axum::{extract::Query, Json};
use serde::Deserialize;

use crate::db::get_db;
use crate::ia::types::Contact;
use crate::sessions::manager::get_session;
use crate::tools::wechat_contacts;
use crate::tools::wechat_db::{find_wechat_pid, list_account_dbs};
use crate::tools::wechat_keys::{extract_keys_async, get_stored_keys, store_keys};

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(
        default,
        rename = "localType",
        deserialize_with = "deserialize_local_type"
    )]
    local_type: Option<i64>,
}

fn default_limit() -> i64 {
    200
}

pub async fn list_contacts(Query(params): Query<ListParams>) -> Json<Vec<Contact>> {
    let session = match get_session("default") {
        Some(s) => s,
        None => return Json(Vec::new()),
    };
    let logged_in_user = match &session.logged_in_user {
        Some(u) => u.clone(),
        None => return Json(Vec::new()),
    };

    let mut keys = {
        let db = get_db();
        get_stored_keys(&db, &session.id, &logged_in_user)
    };

    // Lazy key extraction: if contact.db exists on disk without stored key, re-extract
    if !keys.contains_key("contact.db") {
        let on_disk = list_account_dbs(&logged_in_user);
        if on_disk.iter().any(|name| name == "contact.db") {
            if let Some(pid) = find_wechat_pid() {
                let extracted = extract_keys_async(pid).await;
                if !extracted.is_empty() {
                    let db = get_db();
                    store_keys(&db, &session.id, &logged_in_user, &extracted);
                    keys = get_stored_keys(&db, &session.id, &logged_in_user);
                }
            }
        }
    }

    if !keys.contains_key("contact.db") {
        return Json(Vec::new());
    }

    Json(wechat_contacts::list_contacts(
        &logged_in_user,
        &keys,
        params.limit,
        params.offset,
        params.local_type,
    ))
}

#[derive(Deserialize)]
pub struct FindParams {
    name: String,
    #[serde(
        default,
        rename = "localType",
        deserialize_with = "deserialize_local_type"
    )]
    local_type: Option<i64>,
}

pub async fn find_contacts(Query(params): Query<FindParams>) -> Json<Vec<Contact>> {
    let session = match get_session("default") {
        Some(s) => s,
        None => return Json(Vec::new()),
    };
    let logged_in_user = match &session.logged_in_user {
        Some(u) => u.clone(),
        None => return Json(Vec::new()),
    };

    let mut keys = {
        let db = get_db();
        get_stored_keys(&db, &session.id, &logged_in_user)
    };

    // Lazy key extraction: if contact.db exists on disk without stored key, re-extract
    if !keys.contains_key("contact.db") {
        let on_disk = list_account_dbs(&logged_in_user);
        if on_disk.iter().any(|name| name == "contact.db") {
            if let Some(pid) = find_wechat_pid() {
                let extracted = extract_keys_async(pid).await;
                if !extracted.is_empty() {
                    let db = get_db();
                    store_keys(&db, &session.id, &logged_in_user, &extracted);
                    keys = get_stored_keys(&db, &session.id, &logged_in_user);
                }
            }
        }
    }

    Json(wechat_contacts::find_contacts(
        &logged_in_user,
        &keys,
        &params.name,
        params.local_type,
    ))
}

/// Reject unsupported filters during query extraction, before accessing session or database.
fn deserialize_local_type<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Query parameters arrive as strings.
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "1" => Ok(Some(1)),
        "3" => Ok(Some(3)),
        "5" => Ok(Some(5)),
        _ => Err(serde::de::Error::custom("localType must be 1, 3 or 5")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn unsupported_filter_is_bad_request_even_without_a_session() {
        let app = Router::new()
            .route("/contacts", get(list_contacts))
            .route("/contacts/find", get(find_contacts));
        for path in ["/contacts", "/contacts/find?name=Alice"] {
            for value in ["0", "2", "4", "-1", "unknown", "", "1.0"] {
                let separator = if path.contains('?') { '&' } else { '?' };
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(format!("{path}{separator}localType={value}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            }
        }
    }

    #[tokio::test]
    async fn supported_filters_extract_without_changing_defaults() {
        let app = Router::new()
            .route(
                "/contacts",
                get(|Query(params): Query<ListParams>| async move { Json(params.local_type) }),
            )
            .route(
                "/contacts/find",
                get(|Query(params): Query<FindParams>| async move { Json(params.local_type) }),
            );
        for path in ["/contacts", "/contacts/find?name=Alice"] {
            for value in [None, Some("1"), Some("3"), Some("5")] {
                let separator = if path.contains('?') { '&' } else { '?' };
                let uri = value
                    .map(|v| format!("{path}{separator}localType={v}"))
                    .unwrap_or(path.to_string());
                let response = app
                    .clone()
                    .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), 128)
                    .await
                    .unwrap();
                let extracted: Option<i64> = serde_json::from_slice(&body).unwrap();
                assert_eq!(extracted, value.map(|v| v.parse::<i64>().unwrap()));
            }
        }
    }
}
