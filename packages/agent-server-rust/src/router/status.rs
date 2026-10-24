use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query,
    },
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::context::create_context;
use crate::db::get_db;
use crate::execution::run_execution_loop;
use crate::ia::types::*;
use crate::ia::{find_state_by_id, identify_states};
use crate::plans::logout::{LogoutParams, LogoutPlan};
use crate::sessions::manager::get_session;
use crate::tools::a11y::get_a11y_desktop;
use crate::tools::exec::ExecOptions;
use crate::tools::qr::{decode_qr_from_base64, to_data_url};
use crate::tools::screenshot::capture_screenshot;
use base64::Engine;

pub async fn get_status() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "container": "running",
        "loginState": { "status": "logged_out" },
        "version": "0.1.0"
    }))
}

/// Cheap cached observation; requests never start a desktop scan.
pub async fn auth_status() -> Json<serde_json::Value> {
    Json(crate::auth_snapshot::read())
}

/// Log out of WeChat via FSM execution loop.
pub async fn logout() -> Json<serde_json::Value> {
    let session = match get_session("default") {
        Some(s) => s,
        None => {
            return Json(serde_json::json!({
                "success": false,
                "error": "No session available"
            }))
        }
    };

    // Quick auth check first
    let exec_options = ExecOptions {
        session: Some(session.clone()),
        timeout_ms: 30_000,
    };

    let a11y = match get_a11y_desktop(&exec_options).await {
        Ok(tree) => tree,
        Err(e) => {
            return Json(serde_json::json!({
                "success": false,
                "error": format!("Failed to get a11y tree: {e}")
            }))
        }
    };

    let screenshot = capture_screenshot(&exec_options).await.unwrap_or_default();
    let identified = identify_states(&a11y, &screenshot);

    // Load persisted state and check if logged in
    let mut context = {
        let db = get_db();
        create_context(session.clone(), &db)
    };

    if let Some(ref mw) = identified.main_window {
        if let Some(state_impl) = find_state_by_id(&mw.state_id) {
            let screenshot_bytes = base64::engine::general_purpose::STANDARD
                .decode(&screenshot)
                .unwrap_or_default();
            context.state = state_impl.reduce(&ReduceArgs {
                prev: &context.state,
                a11y: &a11y,
                screenshot: &screenshot_bytes,
            });
        }
    }

    if !context.state.main_window.is_logged_in {
        return Json(serde_json::json!({
            "success": false,
            "error": "Not logged in"
        }));
    }

    // Run logout FSM
    let cancel = CancellationToken::new();
    let plan = LogoutPlan;
    let params = LogoutParams;
    let emit = |_event: SubscriptionEvent| {};
    let (result, _) = run_execution_loop(&plan, &params, &mut context, &emit, cancel).await;

    if result.success {
        // Clear logged_in_user from session
        let db = get_db();
        crate::db::queries::update_session_logged_in_user(&db, &session.id, None);
    }

    Json(serde_json::json!({
        "success": result.success,
        "error": result.error
    }))
}

pub async fn login() -> Json<serde_json::Value> {
    let screenshot = capture_screenshot(&ExecOptions::default()).await;

    match screenshot {
        Ok(b64) => {
            if let Some(qr_result) = decode_qr_from_base64(&b64) {
                let data_url = to_data_url(&qr_result.data).ok();
                return Json(serde_json::json!({
                    "success": false,
                    "state": { "status": "qr_pending" },
                    "qrDataUrl": data_url
                }));
            }

            Json(serde_json::json!({
                "success": false,
                "state": { "status": "qr_pending" }
            }))
        }
        Err(_) => Json(serde_json::json!({
            "success": false,
            "state": { "status": "logged_out" }
        })),
    }
}

#[derive(Deserialize)]
pub struct LoginWsParams {
    #[serde(rename = "timeoutMs", default = "default_timeout")]
    timeout_ms: u64,
    #[serde(rename = "newAccount", default)]
    new_account: bool,
    #[serde(rename = "attemptId", default)]
    attempt_id: Option<String>,
}

fn default_timeout() -> u64 {
    300_000
}

pub async fn login_ws(
    ws: WebSocketUpgrade,
    Query(params): Query<LoginWsParams>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_login_ws(socket, params))
}

async fn handle_login_ws(mut socket: WebSocket, params: LoginWsParams) {
    use serde_json::json;
    let Some(session) = get_session("default") else {
        let _ = socket
            .send(Message::Text(
                json!({"type":"error","code":"instance_starting",
            "message":"Session is not ready","retryable":true})
                .to_string()
                .into(),
            ))
            .await;
        return;
    };
    if params
        .attempt_id
        .as_ref()
        .is_some_and(|id| id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        let _ = socket
            .send(Message::Text(
                json!({"type":"error","code":"invalid_attempt",
            "message":"Invalid login attempt","retryable":false})
                .to_string()
                .into(),
            ))
            .await;
        return;
    }
    let (job, start) = match crate::login_jobs::hub().attach(
        params.attempt_id,
        params.new_account,
        std::time::Duration::from_millis(params.timeout_ms.clamp(1_000, 300_000)),
    ) {
        Ok(value) => value,
        Err(code) => {
            let _ = socket
                .send(Message::Text(
                    json!({"type":"error","code":code,
                "message":"Another login is already active","retryable":true})
                    .to_string()
                    .into(),
                ))
                .await;
            return;
        }
    };
    let mut updates = job.updates.subscribe();
    if start {
        tokio::spawn(crate::login_jobs::run(job, session));
    }
    loop {
        let progress = updates.borrow_and_update().clone();
        if socket
            .send(Message::Text(progress.event.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
        if progress.event["type"] != "qr" {
            if let Some(qr) = progress.qr {
                if socket
                    .send(Message::Text(qr.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
        if progress.terminal {
            break;
        }
        tokio::select! {
            changed=updates.changed()=>{ if changed.is_err() { break; } },
            received=socket.recv()=>{
                if !matches!(received,Some(Ok(Message::Ping(_) | Message::Pong(_)))) { break; }
            }
        }
    }
    // Dropping a subscriber does not stop the bounded server-owned task.
}
