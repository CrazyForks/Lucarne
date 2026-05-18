use super::types::WorkspaceId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock},
};
use tokio::{
    sync::{Mutex as AsyncMutex, OwnedMutexGuard},
    task::JoinHandle,
};
use tracing::debug;

#[derive(Debug)]
pub enum TurnAdmission {
    Ready(TurnPermit),
    Queued(QueuedTurn),
}

impl TurnAdmission {
    pub fn queued_position(&self) -> Option<usize> {
        match self {
            Self::Ready(permit) => permit.queued_position(),
            Self::Queued(queued) => Some(queued.position),
        }
    }

    pub async fn wait(self) -> TurnPermit {
        match self {
            Self::Ready(permit) => permit,
            Self::Queued(queued) => queued.wait().await,
        }
    }
}

#[derive(Debug)]
pub struct QueuedTurn {
    workspace_id: WorkspaceId,
    position: usize,
    scheduler: TurnScheduler,
    waiter: Option<JoinHandle<OwnedMutexGuard<()>>>,
    counted: bool,
}

impl QueuedTurn {
    pub fn position(&self) -> usize {
        self.position
    }

    pub async fn wait(mut self) -> TurnPermit {
        let guard = self
            .waiter
            .take()
            .expect("queued turn waiter")
            .await
            .expect("queued turn waiter task");
        self.scheduler.note_dequeued(&self.workspace_id);
        self.counted = false;
        TurnPermit {
            queued_position: Some(self.position),
            _guard: guard,
        }
    }
}

impl Drop for QueuedTurn {
    fn drop(&mut self) {
        if self.counted {
            if let Some(waiter) = self.waiter.take() {
                waiter.abort();
            }
            self.scheduler.note_dequeued(&self.workspace_id);
        }
    }
}

#[derive(Debug)]
pub struct TurnPermit {
    queued_position: Option<usize>,
    _guard: OwnedMutexGuard<()>,
}

impl TurnPermit {
    pub fn queued_position(&self) -> Option<usize> {
        self.queued_position
    }
}

#[derive(Debug, Clone, Default)]
pub struct TurnScheduler {
    locks: Arc<StdRwLock<HashMap<WorkspaceId, Arc<AsyncMutex<()>>>>>,
    waiters: Arc<StdMutex<HashMap<WorkspaceId, usize>>>,
}

impl TurnScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn admit(&self, workspace_id: &WorkspaceId) -> TurnAdmission {
        let lock = self.workspace_lock(workspace_id);
        match lock.clone().try_lock_owned() {
            Ok(guard) => {
                debug!(
                    target: "lucarne::control_plane::turn_queue",
                    workspace_id = %workspace_id.as_str(),
                    "turn admitted"
                );
                TurnAdmission::Ready(TurnPermit {
                    queued_position: None,
                    _guard: guard,
                })
            }
            Err(_) => {
                let position = self.note_queued(workspace_id);
                debug!(
                    target: "lucarne::control_plane::turn_queue",
                    workspace_id = %workspace_id.as_str(),
                    position,
                    "turn queued"
                );
                let waiter = tokio::spawn(async move { lock.lock_owned().await });
                TurnAdmission::Queued(QueuedTurn {
                    workspace_id: workspace_id.clone(),
                    position,
                    scheduler: self.clone(),
                    waiter: Some(waiter),
                    counted: true,
                })
            }
        }
    }

    pub async fn acquire(&self, workspace_id: &WorkspaceId) -> TurnPermit {
        self.admit(workspace_id).wait().await
    }

    fn workspace_lock(&self, workspace_id: &WorkspaceId) -> Arc<AsyncMutex<()>> {
        if let Some(lock) = self
            .locks
            .read()
            .expect("turn scheduler lock registry")
            .get(workspace_id)
            .cloned()
        {
            return lock;
        }

        self.locks
            .write()
            .expect("turn scheduler lock registry")
            .entry(workspace_id.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    fn note_queued(&self, workspace_id: &WorkspaceId) -> usize {
        let mut waiters = self.waiters.lock().unwrap();
        let count = waiters.entry(workspace_id.clone()).or_insert(0);
        *count += 1;
        *count
    }

    fn note_dequeued(&self, workspace_id: &WorkspaceId) {
        let mut waiters = self.waiters.lock().unwrap();
        match waiters.get_mut(workspace_id) {
            Some(1) => {
                waiters.remove(workspace_id);
            }
            Some(count) => {
                *count -= 1;
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    #[tokio::test]
    async fn same_workspace_turns_are_admitted_serially_with_position() {
        let scheduler = TurnScheduler::new();
        let workspace = WorkspaceId::new("ws");
        let first = scheduler.acquire(&workspace).await;
        assert_eq!(first.queued_position(), None);

        let second_scheduler = scheduler.clone();
        let second_workspace = workspace.clone();
        let second_admission = second_scheduler.admit(&second_workspace);
        assert_eq!(second_admission.queued_position(), Some(1));
        let second = tokio::spawn(async move { second_admission.wait().await });
        sleep(Duration::from_millis(20)).await;
        assert!(
            !second.is_finished(),
            "second turn must wait while the first permit is alive"
        );

        drop(first);
        let second = timeout(Duration::from_secs(1), second)
            .await
            .expect("second turn should acquire after first completes")
            .expect("second turn task");
        assert_eq!(second.queued_position(), Some(1));
    }

    #[test]
    fn turn_scheduler_emits_structured_tracing() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/control_plane/turn_queue.rs"),
        )
        .expect("read turn queue source");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source");

        for needle in [
            "lucarne::control_plane::turn_queue",
            "turn admitted",
            "turn queued",
        ] {
            assert!(
                production.contains(needle),
                "turn scheduler tracing must cover admission boundary: {needle}"
            );
        }
    }

    #[test]
    fn turn_scheduler_uses_read_optimized_workspace_lock_registry() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/control_plane/turn_queue.rs"),
        )
        .expect("read turn queue source");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source");

        assert!(
            production.contains("locks: Arc<StdRwLock<HashMap<WorkspaceId, Arc<AsyncMutex<()>>>>>"),
            "workspace lock registry should allow read-only lookups without taking an exclusive map lock"
        );
        assert!(
            !production.contains("locks: Arc<StdMutex<HashMap<WorkspaceId, Arc<AsyncMutex<()>>>>>"),
            "workspace lock registry should not use a global mutex for every turn admission"
        );
    }

    #[tokio::test]
    async fn dropped_queued_turn_releases_waiter_position() {
        let scheduler = TurnScheduler::new();
        let workspace = WorkspaceId::new("ws");
        let _first = scheduler.acquire(&workspace).await;

        let dropped = scheduler.admit(&workspace);
        assert_eq!(dropped.queued_position(), Some(1));
        drop(dropped);

        let next = scheduler.admit(&workspace);
        assert_eq!(next.queued_position(), Some(1));
    }

    #[tokio::test]
    async fn queued_turn_reserves_fifo_position_before_wait_is_called() {
        let scheduler = TurnScheduler::new();
        let workspace = WorkspaceId::new("ws");
        let first = scheduler.acquire(&workspace).await;

        let queued = scheduler.admit(&workspace);
        assert_eq!(queued.queued_position(), Some(1));
        drop(first);

        sleep(Duration::from_millis(20)).await;
        let later = scheduler.admit(&workspace);
        assert_eq!(
            later.queued_position(),
            Some(2),
            "a later turn must not overtake a queued turn while Telegram sends its queue notice"
        );

        drop(queued);
        timeout(Duration::from_secs(1), later.wait())
            .await
            .expect("later turn should acquire after queued turn is cancelled");
    }

    #[tokio::test]
    async fn different_workspaces_are_admitted_independently() {
        let scheduler = TurnScheduler::new();
        let first = scheduler.acquire(&WorkspaceId::new("ws-1")).await;
        let second = scheduler.acquire(&WorkspaceId::new("ws-2")).await;

        assert_eq!(first.queued_position(), None);
        assert_eq!(second.queued_position(), None);
    }
}
