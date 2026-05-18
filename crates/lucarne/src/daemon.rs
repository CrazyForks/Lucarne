use std::{path::Path, sync::Arc};

use crate::{
    agent_runtime::AgentRuntime,
    control_plane::ControlPlaneSqliteStore,
    core_service::{CoreError, LucarneCore},
};
use tracing::info;

#[derive(Clone)]
pub struct LucarneDaemon {
    core: Arc<LucarneCore>,
}

impl LucarneDaemon {
    pub fn new() -> Self {
        let runtime = Arc::new(AgentRuntime::new());
        runtime.register_defaults();
        let store = ControlPlaneSqliteStore::open(":memory:")
            .expect("in-memory control-plane store must open");
        Self::from_runtime_and_store(runtime, store).expect("core must open")
    }

    pub fn open_sqlite(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        let path = path.as_ref();
        info!(
            target: "lucarne::daemon",
            path = %path.display(),
            "opening sqlite daemon"
        );
        Ok(Self {
            core: LucarneCore::open_sqlite(path)?,
        })
    }

    pub fn new_with_store(control_plane_store: ControlPlaneSqliteStore) -> Result<Self, CoreError> {
        let runtime = Arc::new(AgentRuntime::new());
        runtime.register_defaults();
        Self::from_runtime_and_store(runtime, control_plane_store)
    }

    pub fn from_runtime_and_store(
        runtime: Arc<AgentRuntime>,
        control_plane_store: ControlPlaneSqliteStore,
    ) -> Result<Self, CoreError> {
        info!(
            target: "lucarne::daemon",
            "daemon core attached"
        );
        Ok(Self {
            core: LucarneCore::from_runtime_and_store(runtime, control_plane_store)?,
        })
    }

    pub fn core(&self) -> Arc<LucarneCore> {
        Arc::clone(&self.core)
    }

    pub fn runtime(&self) -> Arc<AgentRuntime> {
        self.core.runtime()
    }

    pub fn provider_ids(&self) -> &[&'static str] {
        self.core.provider_ids()
    }

    pub fn control_plane_store(&self) -> Option<ControlPlaneSqliteStore> {
        Some(self.core.control_plane_store())
    }
}

impl Default for LucarneDaemon {
    fn default() -> Self {
        Self::new()
    }
}
