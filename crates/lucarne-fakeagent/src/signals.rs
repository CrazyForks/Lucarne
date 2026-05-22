use std::sync::atomic::AtomicUsize;

static SIGINT_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGTERM_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGHUP_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(unix)]
pub(crate) fn install_signal_handlers() {
    use nix::sys::signal::{self, SigHandler, Signal};

    for signal in [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP] {
        unsafe {
            signal::signal(signal, SigHandler::Handler(handle_signal))
                .unwrap_or_else(|err| panic!("install signal handler for {signal:?}: {err}"));
        }
    }
}

#[cfg(windows)]
pub(crate) fn install_signal_handlers() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    let ok = unsafe { SetConsoleCtrlHandler(Some(handle_console_ctrl), 1) };
    if ok == 0 {
        panic!("install console control handler failed");
    }
}

#[cfg(unix)]
extern "C" fn handle_signal(sig: i32) {
    use nix::sys::signal::Signal;
    use std::sync::atomic::Ordering;

    match sig {
        x if x == Signal::SIGINT as i32 => {
            SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        x if x == Signal::SIGTERM as i32 => {
            SIGTERM_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        x if x == Signal::SIGHUP as i32 => {
            SIGHUP_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
}

#[cfg(windows)]
unsafe extern "system" fn handle_console_ctrl(ctrl_type: u32) -> i32 {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT};

    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => {
            SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
            1
        }
        CTRL_CLOSE_EVENT => {
            SIGTERM_COUNT.fetch_add(1, Ordering::SeqCst);
            1
        }
        _ => 0,
    }
}

#[cfg(unix)]
pub(crate) fn normalize_signal_name(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        "SIGINT" | "INT" => Some("SIGINT"),
        "SIGTERM" | "TERM" => Some("SIGTERM"),
        "SIGHUP" | "HUP" => Some("SIGHUP"),
        _ => None,
    }
}

#[cfg(windows)]
pub(crate) fn normalize_signal_name(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        "SIGINT" | "INT" | "SIGTERM" | "TERM" => Some("SIGINT"),
        _ => None,
    }
}

pub(crate) fn signal_counter(name: &'static str) -> &'static AtomicUsize {
    match name {
        "SIGINT" => &SIGINT_COUNT,
        "SIGTERM" => &SIGTERM_COUNT,
        "SIGHUP" => &SIGHUP_COUNT,
        other => panic!("missing counter for {other}"),
    }
}
