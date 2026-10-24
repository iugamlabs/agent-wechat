//! A login belongs to the server, not to any individual websocket subscriber.
use crate::ia::types::SubscriptionEvent;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Progress {
    pub phase: String,
    pub event: Value,
    pub qr: Option<Value>,
    pub terminal: bool,
}

pub struct LoginJob {
    pub id: String,
    pub new_account: bool,
    pub deadline: Instant,
    pub updates: watch::Sender<Progress>,
}

#[derive(Default)]
pub struct LoginHub(Mutex<Option<Arc<LoginJob>>>);

impl LoginHub {
    pub fn attach(
        &self,
        id: Option<String>,
        new_account: bool,
        timeout: Duration,
    ) -> Result<(Arc<LoginJob>, bool), &'static str> {
        let mut slot = self.0.lock().unwrap();
        if let Some(job) = slot.as_ref() {
            if id.as_ref().is_some_and(|id| id == &job.id) {
                return Ok((job.clone(), false));
            }
            if !job.updates.borrow().terminal && job.deadline > Instant::now() {
                if id.is_none() && new_account == job.new_account {
                    return Ok((job.clone(), false));
                }
                return Err("login_busy");
            }
        }
        let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let (updates, _) = watch::channel(Progress {
            phase: "connecting".into(),
            event: json!({"type":"status", "phase":"connecting", "message":"Connecting login flow"}),
            qr: None,
            terminal: false,
        });
        let job = Arc::new(LoginJob {
            id,
            new_account,
            deadline: Instant::now() + timeout,
            updates,
        });
        *slot = Some(job.clone());
        Ok((job, true))
    }
}

pub fn hub() -> &'static LoginHub {
    static HUB: OnceLock<LoginHub> = OnceLock::new();
    HUB.get_or_init(LoginHub::default)
}

impl LoginJob {
    pub fn publish(&self, mut event: Value) {
        let old = self.updates.borrow().clone();
        if old.terminal {
            return;
        }
        if event["type"] == "login_success" && !crate::auth_snapshot::confirm_ready() {
            event = json!({"type":"error","code":"session_preparation_failed","retryable":true,"message":"Session readiness could not be verified"});
        }
        let phase = match event["type"].as_str().unwrap_or("") {
            "qr" => "waiting_qr",
            "phone_confirm" => "waiting_phone_confirm",
            "login_success" => "ready",
            "login_timeout" => "timed_out",
            "error" => "failed",
            _ => event["phase"].as_str().unwrap_or(&old.phase),
        }
        .to_string();
        let terminal = matches!(
            phase.as_str(),
            "ready" | "timed_out" | "failed" | "interrupted"
        );
        event["phase"] = json!(phase);
        event["updatedAt"] = json!(Utc::now().to_rfc3339());
        event["attemptId"] = json!(self.id);
        let qr = if phase == "waiting_qr" {
            if event["type"] == "qr" {
                Some(event.clone())
            } else {
                old.qr
            }
        } else {
            None
        };
        crate::auth_snapshot::progress(&phase, &self.id, event["code"].as_str());
        persist(&self.id, &phase, event["code"].as_str());
        tracing::info!(attempt_id=%self.id, phase=%phase, "Login progress");
        self.updates.send_replace(Progress {
            phase,
            event,
            qr,
            terminal,
        });
    }

    pub fn emit(&self, event: SubscriptionEvent) {
        let mut value = serde_json::to_value(event.data).unwrap_or_else(|_| json!({}));
        value["type"] = json!(event.event_type);
        if value["type"] == "status"
            && value["message"]
                .as_str()
                .is_some_and(|s| s.starts_with("Getting your WeChat messages"))
        {
            value["phase"] = json!("preparing_session");
        }
        if value["type"] == "qr" {
            if let Some(qr) = value["qrData"].as_str() {
                value["qrDataUrl"] = json!(crate::tools::qr::to_data_url(qr).ok());
            }
        }
        self.publish(value);
    }
}

fn persist(id: &str, phase: &str, code: Option<&str>) {
    if let Some(db) = crate::db::try_db() {
        if let Err(error) = db.execute("INSERT INTO login_jobs(id,phase,error_code,updated_at) VALUES (?1,?2,?3,?4)
            ON CONFLICT(id) DO UPDATE SET phase=excluded.phase,error_code=excluded.error_code,updated_at=excluded.updated_at",
            rusqlite::params![id, phase, code, Utc::now().to_rfc3339()]) {
            tracing::warn!(error=%error, "Could not persist login progress");
        }
    }
}

pub fn recover() {
    let db = crate::db::get_db();
    let _ = db.execute("UPDATE login_jobs SET phase='interrupted',error_code='login_interrupted' WHERE phase NOT IN ('ready','failed','timed_out','interrupted')", []);
}

pub async fn run(job: Arc<LoginJob>, session: crate::ia::types::Session) {
    job.publish(json!({"type":"status", "phase":"connecting", "message":"Connecting login flow"}));
    let limit = job.deadline.saturating_duration_since(Instant::now());
    let cancel = CancellationToken::new();
    let emitted = job.clone();
    let execution = async {
        let mut context = {
            let db = crate::db::get_db();
            crate::context::create_context(session, &db)
        };
        let result = crate::execution::run_execution_loop(
            &crate::plans::login::LoginPlan,
            &crate::plans::login::LoginParams {
                new_account: job.new_account,
            },
            &mut context,
            &move |event| emitted.emit(event),
            cancel.clone(),
        )
        .await
        .0;
        result
    };
    match tokio::time::timeout(limit, execution).await {
        Err(_) => {
            job.publish(json!({"type":"login_timeout", "code":"login_timeout", "retryable":true}))
        }
        Ok(result) if !job.updates.borrow().terminal => {
            job.publish(json!({"type":"error", "code":"session_preparation_failed", "retryable":true,
                "message":if result.success {"Session was not ready"} else {"Login could not finish"}}));
        }
        _ => {}
    }
    cancel.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_reuses_job_but_competing_attempt_is_rejected() {
        let hub = LoginHub::default();
        let (first, start) = hub
            .attach(Some("one".into()), false, Duration::from_secs(10))
            .unwrap();
        assert!(start);
        first.publish(json!({"type":"phone_confirm"}));
        let (reconnected, start) = hub
            .attach(Some("one".into()), false, Duration::from_secs(99))
            .unwrap();
        assert!(!start && Arc::ptr_eq(&first, &reconnected));
        assert_eq!(reconnected.deadline, first.deadline);
        assert!(matches!(
            hub.attach(Some("two".into()), false, Duration::from_secs(10)),
            Err("login_busy")
        ));
    }
    #[test]
    fn qr_is_not_replayed_after_phone_confirmation_and_failure_stays_terminal() {
        let hub = LoginHub::default();
        let (job, _) = hub.attach(None, false, Duration::from_secs(10)).unwrap();
        job.publish(json!({"type":"qr","qrData":"test"}));
        assert!(job.updates.borrow().qr.is_some());
        job.publish(json!({"type":"phone_confirm"}));
        assert!(job.updates.borrow().qr.is_none());
        job.publish(json!({"type":"error","code":"session_preparation_failed"}));
        job.publish(json!({"type":"login_success"}));
        assert_eq!(job.updates.borrow().phase, "failed");
    }
}
