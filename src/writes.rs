//! Per-repo write ordering: git-bug holds a repository lock, so writes to one repo run one at a time, in the
//! order they were issued. Different repos never wait on each other.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

#[derive(Default)]
struct Lane {
    state: Mutex<LaneState>,
    changed: Notify,
}

#[derive(Default)]
struct LaneState {
    next: u64,
    serving: u64,
    /// Finished or abandoned tickets ahead of `serving`.
    done: BTreeSet<u64>,
}

impl Lane {
    fn finish(&self, id: u64) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let state = &mut *guard;
        state.done.insert(id);
        while state.done.remove(&state.serving) {
            state.serving += 1;
        }
        drop(guard);
        self.changed.notify_waiters();
    }

    fn is_serving(&self, id: u64) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).serving == id
    }
}

#[derive(Default)]
pub struct WriteQueue {
    lanes: HashMap<PathBuf, Arc<Lane>>,
}

impl WriteQueue {
    /// Takes the next place in the repo's line. Call it synchronously, in issue order; await `Ticket::turn` later.
    pub fn enter(&mut self, root: &Path) -> Ticket {
        let lane = Arc::clone(self.lanes.entry(root.to_owned()).or_default());
        let mut state = lane.state.lock().unwrap_or_else(|e| e.into_inner());
        let id = state.next;
        state.next += 1;
        drop(state);
        Ticket { lane, id }
    }
}

/// Dropping it, finished or not, lets the next ticket in line go.
pub struct Ticket {
    lane: Arc<Lane>,
    id: u64,
}

impl Ticket {
    pub async fn turn(&self) {
        loop {
            let changed = self.lane.changed.notified();
            if self.lane.is_serving(self.id) {
                return;
            }
            changed.await;
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.lane.finish(self.id);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::{sleep, timeout};

    use super::*;

    #[tokio::test]
    async fn same_repo_runs_in_issue_order_even_if_polled_out_of_order() {
        let mut queue = WriteQueue::default();
        let root = Path::new("/r/a");
        let tickets: Vec<Ticket> = (0..4).map(|_| queue.enter(root)).collect();
        let log = Arc::new(Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for (n, ticket) in tickets.into_iter().enumerate().rev() {
            let log = Arc::clone(&log);
            handles.push(tokio::spawn(async move {
                ticket.turn().await;
                log.lock().unwrap().push(format!("start {n}"));
                sleep(Duration::from_millis(15 - 3 * n as u64)).await;
                log.lock().unwrap().push(format!("end {n}"));
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let expected: Vec<String> = (0..4)
            .flat_map(|n| [format!("start {n}"), format!("end {n}")])
            .collect();
        assert_eq!(*log.lock().unwrap(), expected);
    }

    #[tokio::test]
    async fn different_repos_do_not_wait_on_each_other() {
        let mut queue = WriteQueue::default();
        let a = queue.enter(Path::new("/r/a"));
        let b = queue.enter(Path::new("/r/b"));
        a.turn().await;
        timeout(Duration::from_millis(200), b.turn())
            .await
            .expect("repo b is not blocked by repo a");
    }

    #[tokio::test]
    async fn dropped_tickets_never_stall_the_line() {
        let mut queue = WriteQueue::default();
        let root = Path::new("/r/a");
        let first = queue.enter(root);
        let abandoned = queue.enter(root);
        let last = queue.enter(root);

        drop(abandoned);
        assert!(
            timeout(Duration::from_millis(30), last.turn())
                .await
                .is_err(),
            "still behind the first ticket"
        );
        drop(first);
        timeout(Duration::from_millis(200), last.turn())
            .await
            .expect("abandoned ticket is skipped");
    }
}
