#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
#[allow(unused_imports)]
pub(crate) use unix::{configure_command, pid_is_alive, ManagedProcess};
#[cfg(windows)]
#[allow(unused_imports)]
pub(crate) use windows::{configure_command, pid_is_alive, ManagedProcess};
