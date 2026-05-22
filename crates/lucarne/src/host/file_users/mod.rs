#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::observed_session_writer_pid;
#[cfg(windows)]
pub(crate) use windows::observed_session_writer_pid;
