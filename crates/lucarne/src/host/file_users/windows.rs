use std::{iter, os::windows::ffi::OsStrExt, path::Path, ptr};

use tracing::trace;
use windows_sys::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, WIN32_ERROR},
        System::RestartManager::{
            RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
            RM_PROCESS_INFO,
        },
    },
};

pub(crate) fn observed_session_writer_pid(path: &Path) -> Option<i32> {
    match observed_session_writer_pid_inner(path) {
        Ok(pid) => pid,
        Err(RestartManagerError::Api { operation, code }) => {
            trace!(target: "lucarne::host::file_users", operation, code, path = %path.display(), "restart manager file user lookup failed");
            None
        }
    }
}

fn observed_session_writer_pid_inner(path: &Path) -> Result<Option<i32>, RestartManagerError> {
    let session = RestartManagerSession::start()?;
    session.register_path(path)?;
    session.first_external_positive_pid()
}

struct RestartManagerSession {
    handle: u32,
}

impl RestartManagerSession {
    fn start() -> Result<Self, RestartManagerError> {
        let mut handle = 0;
        let mut session_key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        let error = unsafe { RmStartSession(&mut handle, 0, session_key.as_mut_ptr()) };
        if error != ERROR_SUCCESS {
            return Err(RestartManagerError::Api {
                operation: "RmStartSession",
                code: error,
            });
        }
        Ok(Self { handle })
    }

    fn register_path(&self, path: &Path) -> Result<(), RestartManagerError> {
        let path = path_to_utf16_null(path);
        let paths = [path.as_ptr() as PCWSTR];
        let error = unsafe {
            RmRegisterResources(
                self.handle,
                paths.len() as u32,
                paths.as_ptr(),
                0,
                ptr::null(),
                0,
                ptr::null(),
            )
        };
        if error != ERROR_SUCCESS {
            return Err(RestartManagerError::Api {
                operation: "RmRegisterResources",
                code: error,
            });
        }
        Ok(())
    }

    fn first_external_positive_pid(&self) -> Result<Option<i32>, RestartManagerError> {
        let current_pid = std::process::id();
        let mut needed = 0;
        let mut count = 0;
        let mut reboot_reasons = 0;
        let mut error = unsafe {
            RmGetList(
                self.handle,
                &mut needed,
                &mut count,
                ptr::null_mut(),
                &mut reboot_reasons,
            )
        };

        if error == ERROR_SUCCESS {
            return Ok(None);
        }
        if error != ERROR_MORE_DATA {
            return Err(RestartManagerError::Api {
                operation: "RmGetList",
                code: error,
            });
        }

        for _ in 0..2 {
            count = needed;
            let mut affected_apps = vec![RM_PROCESS_INFO::default(); count as usize];
            error = unsafe {
                RmGetList(
                    self.handle,
                    &mut needed,
                    &mut count,
                    affected_apps.as_mut_ptr(),
                    &mut reboot_reasons,
                )
            };
            if error == ERROR_MORE_DATA {
                continue;
            }
            if error != ERROR_SUCCESS {
                return Err(RestartManagerError::Api {
                    operation: "RmGetList",
                    code: error,
                });
            }

            return Ok(affected_apps
                .into_iter()
                .take(count as usize)
                .map(|process| process.Process.dwProcessId)
                .find_map(|pid| {
                    (pid > 0 && pid != current_pid && pid <= i32::MAX as u32).then_some(pid as i32)
                }));
        }

        Err(RestartManagerError::Api {
            operation: "RmGetList",
            code: ERROR_MORE_DATA,
        })
    }
}

impl Drop for RestartManagerSession {
    fn drop(&mut self) {
        let error = unsafe { RmEndSession(self.handle) };
        if error != ERROR_SUCCESS {
            trace!(target: "lucarne::host::file_users", error, "RmEndSession failed");
        }
    }
}

#[derive(Debug)]
enum RestartManagerError {
    Api {
        operation: &'static str,
        code: WIN32_ERROR,
    },
}

fn path_to_utf16_null(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect()
}
