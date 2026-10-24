//! Request handlers read this cache; only observers and login jobs do GUI work.
use crate::ia::types::{AppState, MainWindowView};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

fn cache() -> &'static Mutex<Value> {
    static CACHE: OnceLock<Mutex<Value>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(
            json!({"status":"unknown", "phase":"connecting", "dataReady":false,
        "protocolVersion":2, "updatedAt":Utc::now().to_rfc3339()}),
        )
    })
}

pub fn read() -> Value {
    let mut value = cache().lock().unwrap().clone();
    let fresh = value
        .get("observedAt")
        .and_then(Value::as_str)
        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
        .is_some_and(|stamp| Utc::now().signed_duration_since(stamp).num_seconds() < 30);
    value["stale"] = json!(!fresh);
    value
}

pub fn progress(phase: &str, attempt_id: &str, code: Option<&str>) {
    let mut value = cache().lock().unwrap();
    if value["phase"] != phase || value["attemptId"] != attempt_id {
        value["updatedAt"] = json!(Utc::now().to_rfc3339());
    }
    value["phase"] = json!(phase);
    value["attemptId"] = json!(attempt_id);
    value["errorCode"] = json!(code);
    if phase != "ready" {
        value["dataReady"] = json!(false);
    }
}

pub fn observe(state: &AppState) {
    let session = crate::sessions::manager::get_session("default");
    let identity = session.as_ref().and_then(|s| s.logged_in_user.clone());
    let ready = state.main_window.is_logged_in
        && session.as_ref().is_some_and(|session| {
            identity.as_ref().is_some_and(|identity| {
                if crate::tools::wechat_db::find_wechat_pid()
                    .and_then(crate::tools::wechat_db::find_account_dir)
                    .as_ref()
                    != Some(identity)
                {
                    return false;
                }
                let db = crate::db::get_db();
                crate::tools::wechat_keys::core_data_ready(&db, &session.id, identity)
            })
        });
    let phase = if ready {
        "ready"
    } else {
        match state.main_window.view {
            MainWindowView::LoginQr => "waiting_qr",
            MainWindowView::LoginPhoneConfirm => "waiting_phone_confirm",
            MainWindowView::LoginLoading | MainWindowView::Chat | MainWindowView::ChatOpen => {
                "preparing_session"
            }
            _ => "connecting",
        }
    };
    let mut value = cache().lock().unwrap();
    value["status"] = json!(if state.main_window.is_logged_in {
        "logged_in"
    } else {
        "logged_out"
    });
    value["loggedInUser"] = json!(identity);
    value["dataReady"] = json!(ready);
    value["observedAt"] = json!(Utc::now().to_rfc3339());
    // Preserve the actionable terminal failure until a new task is requested.
    if ready
        || !matches!(
            value["phase"].as_str(),
            Some("failed" | "timed_out" | "interrupted")
        )
    {
        if value["phase"] != phase {
            value["updatedAt"] = json!(Utc::now().to_rfc3339());
        }
        value["phase"] = json!(phase);
    }
}

pub fn confirm_ready() -> bool {
    let Some(session) = crate::sessions::manager::get_session("default") else {
        return false;
    };
    let Some(identity) = session.logged_in_user.as_ref() else {
        return false;
    };
    if crate::tools::wechat_db::find_wechat_pid()
        .and_then(crate::tools::wechat_db::find_account_dir)
        .as_ref()
        != Some(identity)
    {
        return false;
    }
    let ready = {
        let db = crate::db::get_db();
        crate::tools::wechat_keys::core_data_ready(&db, &session.id, identity)
    };
    if ready {
        let mut value = cache().lock().unwrap();
        value["status"] = json!("logged_in");
        value["loggedInUser"] = json!(identity);
        value["dataReady"] = json!(true);
        value["observedAt"] = json!(Utc::now().to_rfc3339());
    }
    ready
}

pub fn stopped() {
    let mut value = cache().lock().unwrap();
    *value = json!({"status":"app_not_running", "phase":"connecting", "dataReady":false,
        "protocolVersion":2, "updatedAt":Utc::now().to_rfc3339(), "observedAt":Utc::now().to_rfc3339()});
}
