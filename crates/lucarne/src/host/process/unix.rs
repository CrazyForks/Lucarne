use crate::error::{LucarneError, Result};
use nix::{
    errno::Errno,
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::os::unix::process::CommandExt;
use tokio::process::{Child, Command};
use tracing::trace;

#[derive(Debug, Default)]
pub(crate) struct ManagedProcess;

impl ManagedProcess {
    pub(crate) fn attach(_child: &Child) -> Result<Self> {
        Ok(Self)
    }

    pub(crate) fn signal(&self, pid: i32, name: &str) -> Result<()> {
        let sig = match name {
            "SIGINT" => Signal::SIGINT,
            "SIGTERM" => Signal::SIGTERM,
            "SIGKILL" => Signal::SIGKILL,
            "SIGHUP" => Signal::SIGHUP,
            other => return Err(LucarneError::runtime(format!("unknown signal {}", other))),
        };
        trace!(target: "lucarne::host::process", pid, signal = name, "sending unix process signal");
        kill(Pid::from_raw(pid), sig).map_err(|e| LucarneError::runtime(format!("kill: {}", e)))
    }

    pub(crate) fn terminate_graceful(&self, pid: i32) -> Result<()> {
        trace!(target: "lucarne::host::process", pid, "sending unix process-group SIGTERM");
        kill(Pid::from_raw(-pid), Signal::SIGTERM)
            .map_err(|e| LucarneError::runtime(format!("kill: {}", e)))
    }

    pub(crate) fn terminate_force(&self, pid: i32) -> Result<()> {
        trace!(target: "lucarne::host::process", pid, "sending unix process-group SIGKILL");
        kill(Pid::from_raw(-pid), Signal::SIGKILL)
            .map_err(|e| LucarneError::runtime(format!("kill: {}", e)))
    }
}

pub(crate) fn configure_command(command: &mut Command) {
    command.as_std_mut().process_group(0);
}

#[allow(dead_code)]
pub(crate) fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match kill(Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}
