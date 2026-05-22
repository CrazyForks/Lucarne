#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
#[allow(unused_imports)]
pub(crate) use unix::{ManagedProcess, configure_command, pid_is_alive};
#[cfg(windows)]
#[allow(unused_imports)]
pub(crate) use windows::{ManagedProcess, configure_command, pid_is_alive};
