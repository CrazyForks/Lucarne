use crate::error::{LucarneError, Result};
use std::{
    io,
    mem::size_of,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
};
use tokio::process::{Child, Command};
use tracing::trace;
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_ACCESS_DENIED, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE},
    System::{
        Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT},
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            GetExitCodeProcess, OpenProcess, OpenThread, ResumeThread, CREATE_NEW_PROCESS_GROUP,
            CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
        },
    },
};

#[derive(Debug)]
pub(crate) struct ManagedProcess {
    job: OwnedHandle,
}

impl ManagedProcess {
    pub(crate) fn attach(child: &Child) -> Result<Self> {
        let pid = child
            .id()
            .ok_or_else(|| LucarneError::launcher("no pid for managed process"))?;
        let child_handle = child
            .raw_handle()
            .ok_or_else(|| LucarneError::launcher("no process handle"))?
            as HANDLE;

        trace!(target: "lucarne::host::process", pid, "attaching process to windows job");
        let job = create_job()?;
        configure_job(&job)?;
        assign_process(&job, child_handle)?;
        resume_process_threads(pid)?;
        trace!(target: "lucarne::host::process", pid, "attached process to windows job");

        Ok(Self { job })
    }

    pub(crate) fn signal(&self, pid: i32, name: &str) -> Result<()> {
        trace!(target: "lucarne::host::process", pid, signal = name, "sending windows process signal");
        match name {
            "SIGINT" => send_console_break(pid as u32),
            "SIGTERM" | "SIGKILL" => self.terminate_force(pid),
            "SIGHUP" => Err(LucarneError::runtime("SIGHUP is not supported on Windows")),
            other => Err(LucarneError::runtime(format!("unknown signal {}", other))),
        }
    }

    pub(crate) fn terminate_graceful(&self, pid: i32) -> Result<()> {
        trace!(target: "lucarne::host::process", pid, "sending windows CTRL_BREAK_EVENT");
        send_console_break(pid as u32)
    }

    pub(crate) fn terminate_force(&self, pid: i32) -> Result<()> {
        trace!(target: "lucarne::host::process", pid, "terminating windows job");
        let result = unsafe { TerminateJobObject(handle(&self.job), 1) };
        if result == 0 {
            return Err(last_error("TerminateJobObject"));
        }
        Ok(())
    }
}

pub(crate) fn configure_command(command: &mut Command) {
    command
        .as_std_mut()
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED);
}

#[allow(dead_code)]
pub(crate) fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32) };
    if process.is_null() {
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }

    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let mut exit_code = 0;
    let ok = unsafe { GetExitCodeProcess(handle(&process), &mut exit_code) };
    ok != 0 && exit_code == STILL_ACTIVE as u32
}

fn create_job() -> Result<OwnedHandle> {
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(last_error("CreateJobObjectW"));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(job) })
}

fn configure_job(job: &OwnedHandle) -> Result<()> {
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

    let ok = unsafe {
        SetInformationJobObject(
            handle(job),
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error("SetInformationJobObject"));
    }
    Ok(())
}

fn assign_process(job: &OwnedHandle, process: HANDLE) -> Result<()> {
    let ok = unsafe { AssignProcessToJobObject(handle(job), process) };
    if ok == 0 {
        return Err(last_error("AssignProcessToJobObject"));
    }
    Ok(())
}

fn resume_process_threads(pid: u32) -> Result<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateToolhelp32Snapshot"));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };

    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..THREADENTRY32::default()
    };
    let mut ok = unsafe { Thread32First(handle(&snapshot), &mut entry) };
    let mut resumed = 0;
    while ok != 0 {
        if entry.th32OwnerProcessID == pid {
            resume_thread(entry.th32ThreadID)?;
            resumed += 1;
        }
        ok = unsafe { Thread32Next(handle(&snapshot), &mut entry) };
    }

    if resumed == 0 {
        return Err(LucarneError::runtime(format!(
            "no threads found for suspended process {}",
            pid
        )));
    }
    Ok(())
}

fn resume_thread(thread_id: u32) -> Result<()> {
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(last_error("OpenThread"));
    }
    let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
    let previous = unsafe { ResumeThread(handle(&thread)) };
    if previous == u32::MAX {
        return Err(last_error("ResumeThread"));
    }
    Ok(())
}

fn send_console_break(pid: u32) -> Result<()> {
    let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
    if ok == 0 {
        return Err(last_error("GenerateConsoleCtrlEvent"));
    }
    Ok(())
}

fn handle(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle()
}

fn last_error(context: &str) -> LucarneError {
    LucarneError::runtime(format!("{}: {}", context, io::Error::last_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, process::Stdio, time::Duration};
    use tokio::io::AsyncReadExt;

    #[test]
    fn pid_is_alive_rejects_non_positive_pid() {
        assert!(!pid_is_alive(0));
        assert!(!pid_is_alive(-1));
    }

    #[tokio::test]
    async fn attach_resumes_suspended_child() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("resumed.txt");
        let script = temp.path().join("mark-resumed.cmd");
        fs::write(
            &script,
            format!("@echo off\r\necho resumed>\"{}\"\r\n", marker.display()),
        )
        .expect("write script");

        let mut command = Command::new("cmd");
        command
            .arg("/C")
            .arg(&script)
            .current_dir(temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        configure_command(&mut command);

        let mut child = command.spawn().expect("spawn suspended child");
        let mut stderr = child.stderr.take().expect("stderr");
        let mut stderr_buf = String::new();
        let stderr_task = tokio::spawn(async move {
            let _ = stderr.read_to_string(&mut stderr_buf).await;
            stderr_buf
        });
        let _managed = ManagedProcess::attach(&child).expect("attach managed process");
        let status = child.wait().await.expect("wait child");
        let stderr = stderr_task.await.expect("stderr task");

        assert!(
            status.success(),
            "status={status:?} marker_exists={} stderr={stderr}",
            marker.exists()
        );
        assert!(marker.exists());
    }

    #[tokio::test]
    async fn job_close_kills_grandchild() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("spawn-grandchild.cmd");
        let started = temp.path().join("started.txt");
        let marker = temp.path().join("grandchild.txt");
        fs::write(
            &script,
            r#"@echo off
if "%~1"=="grandchild" goto grandchild
start "" /B "%COMSPEC%" /C ""%~f0" grandchild "%~2" "%~3""
ping -n 6 127.0.0.1 > nul
exit /b
:grandchild
set "STARTED=%~2"
set "MARKER=%~3"
echo started>"%STARTED%"
ping -n 6 127.0.0.1 > nul
echo grandchild>"%MARKER%"
"#,
        )
        .expect("write script");

        let mut command = Command::new("cmd");
        command
            .arg("/C")
            .arg(&script)
            .arg("parent")
            .arg(&started)
            .arg(&marker)
            .current_dir(temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command(&mut command);

        let mut child = command.spawn().expect("spawn suspended child");
        let managed = ManagedProcess::attach(&child).expect("attach managed process");
        assert!(
            wait_for_path(&started, Duration::from_secs(5)).await,
            "grandchild did not start"
        );
        managed
            .terminate_force(child.id().expect("pid") as i32)
            .expect("terminate job");
        let _ = child.wait().await.expect("wait child");
        tokio::time::sleep(Duration::from_secs(7)).await;

        assert_eq!(fs::read_to_string(&marker).ok(), None);
    }

    async fn wait_for_path(path: &Path, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        path.exists()
    }
}
