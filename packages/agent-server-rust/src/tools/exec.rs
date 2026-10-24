use crate::ia::types::Session;
use std::collections::HashMap;
use tokio::process::Command;

#[derive(Debug)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    async fn helper_is_cleaned_up(cancel: bool) {
        let path = std::env::temp_dir().join(format!("wechat-child-{}", uuid::Uuid::new_v4()));
        let args = [
            "-c",
            "sleep 60 & echo $! > \"$1\"; wait",
            "test",
            path.to_str().unwrap(),
        ];
        let options = ExecOptions {
            timeout_ms: if cancel { 10_000 } else { 200 },
            ..Default::default()
        };
        if cancel {
            assert!(tokio::time::timeout(
                std::time::Duration::from_millis(200),
                exec_command("sh", &args, &options)
            )
            .await
            .is_err());
        } else {
            assert_eq!(
                exec_command("sh", &args, &options).await.stderr,
                "Command timed out"
            );
        }
        let pid = std::fs::read_to_string(&path).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())) {
            assert_eq!(
                status.split_whitespace().nth(2),
                Some("Z"),
                "helper is still running"
            );
        }
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn timeout_stops_helper_descendants() {
        helper_is_cleaned_up(false).await;
    }
    #[tokio::test]
    async fn cancellation_stops_helper_descendants() {
        helper_is_cleaned_up(true).await;
    }
}

pub struct ExecOptions {
    pub session: Option<Session>,
    pub timeout_ms: u64,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            session: None,
            timeout_ms: 60_000,
        }
    }
}

/// Execute a command with fixed arguments (no shell interpolation).
///
/// If a session is provided, the command runs with that session's
/// DISPLAY and DBUS_SESSION_BUS_ADDRESS environment.
pub async fn exec_command(command: &str, args: &[&str], options: &ExecOptions) -> CommandResult {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("QT_ACCESSIBILITY".into(), "1".into());
    env.insert("QT_LINUX_ACCESSIBILITY_ALWAYS_ON".into(), "1".into());

    if let Some(session) = &options.session {
        env.insert("DISPLAY".into(), session.display.clone());
        env.insert(
            "DBUS_SESSION_BUS_ADDRESS".into(),
            session.dbus_address.clone().unwrap_or_default(),
        );
        env.insert("HOME".into(), format!("/home/{}", session.linux_user));
    } else {
        env.entry("DISPLAY".into()).or_insert_with(|| ":99".into());
    }

    let timeout = std::time::Duration::from_millis(options.timeout_ms);

    let result = tokio::time::timeout(timeout, async {
        let mut command = Command::new(command);
        command
            .args(args)
            .envs(&env)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        // Timeout/cancellation must reap helper descendants, including extraction.
        struct Group {
            pid: Option<u32>,
            armed: bool,
        }
        impl Drop for Group {
            fn drop(&mut self) {
                #[cfg(unix)]
                if self.armed {
                    if let Some(pid) = self.pid {
                        let _ = std::process::Command::new("kill")
                            .args(["-KILL", "--", &format!("-{pid}")])
                            .status();
                    }
                }
            }
        }
        let output = match command.spawn() {
            Ok(child) => {
                let mut group = Group {
                    pid: child.id(),
                    armed: true,
                };
                let output = child.wait_with_output().await;
                group.armed = false;
                output
            }
            Err(error) => Err(error),
        };

        match output {
            Ok(out) => CommandResult {
                stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                exit_code: out.status.code().unwrap_or(1),
            },
            Err(err) => CommandResult {
                stdout: String::new(),
                stderr: err.to_string(),
                exit_code: 1,
            },
        }
    })
    .await;

    match result {
        Ok(r) => r,
        Err(_) => CommandResult {
            stdout: String::new(),
            stderr: "Command timed out".to_string(),
            exit_code: 1,
        },
    }
}
