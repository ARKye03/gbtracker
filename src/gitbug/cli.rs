use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use super::GitBugError;

const LOCK_MSG: &str = "already locked by the process";
const RETRIES: u32 = 5;
const BACKOFF: Duration = Duration::from_millis(500);

pub async fn run(root: &Path, args: &[&str]) -> Result<String, GitBugError> {
    retry_locked(RETRIES, BACKOFF, || run_once(root, args)).await
}

async fn run_once(root: &Path, args: &[&str]) -> Result<String, GitBugError> {
    let output = Command::new("git-bug")
        .args(args)
        .current_dir(root)
        .env("GIT_EDITOR", "false")
        .kill_on_drop(true)
        .output()
        .await
        .map_err(GitBugError::Spawn)?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    interpret(
        &args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
        output.status.success(),
        &stderr,
        stdout,
    )
}

pub(crate) async fn retry_locked<F, Fut>(
    retries: u32,
    backoff: Duration,
    mut attempt: F,
) -> Result<String, GitBugError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<String, GitBugError>>,
{
    let mut tried = 0;
    loop {
        match attempt().await {
            Err(GitBugError::Locked { .. }) if tried < retries => {
                tried += 1;
                tokio::time::sleep(backoff).await;
            }
            other => return other,
        }
    }
}

/// git-bug prints "Aborting" and exits 0 when a required message is missing; that is a failure.
pub(crate) fn interpret(
    command: &str,
    success: bool,
    stderr: &str,
    stdout: String,
) -> Result<String, GitBugError> {
    if !success && stderr.contains(LOCK_MSG) {
        return Err(GitBugError::Locked {
            pid: lock_pid(stderr),
        });
    }
    let stderr = stderr
        .lines()
        .filter(|l| !l.starts_with("Building cache"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    if !success {
        return Err(GitBugError::Failed {
            command: command.to_owned(),
            stderr,
        });
    }
    if stderr.contains("Aborting") {
        return Err(GitBugError::Aborted {
            command: command.to_owned(),
            stderr,
        });
    }
    Ok(stdout)
}

fn lock_pid(stderr: &str) -> Option<u32> {
    let tail = stderr.rsplit("pid ").next()?;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
