use std::path::{Path, PathBuf};
use std::sync::OnceLock;
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
    let output = Command::new(git_bug())
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

pub(crate) fn git_bug() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| find_git_bug(search_dirs()).unwrap_or_else(|| PathBuf::from("git-bug")))
}

// Apps launched from Finder/Dock inherit launchd's minimal PATH, which skips
// Homebrew and Go install dirs, so probe those after PATH.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.extend(["go/bin", ".local/bin", ".nix-profile/bin"].map(|d| home.join(d)));
    }
    dirs
}

fn find_git_bug(dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    dirs.into_iter()
        .map(|d| d.join("git-bug"))
        .find(|p| p.is_file())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_git_bug_picks_first_dir_containing_binary() {
        let empty = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("git-bug"), "").unwrap();
        std::fs::write(second.path().join("git-bug"), "").unwrap();
        let dirs = [empty.path(), first.path(), second.path()].map(Path::to_path_buf);
        assert_eq!(find_git_bug(dirs), Some(first.path().join("git-bug")));
    }

    #[test]
    fn find_git_bug_returns_none_when_missing() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(find_git_bug([empty.path().to_path_buf()]), None);
    }
}
