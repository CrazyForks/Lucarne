use std::{
    collections::HashMap,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    time::Instant,
};

use once_cell::sync::Lazy;
use tracing::trace;
use windows_sys::Win32::{
    Foundation::{FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
        },
    },
};

use super::ProcessSample;

static PROCESS_SAMPLER: Lazy<std::sync::Mutex<ProcessSampler>> =
    Lazy::new(|| std::sync::Mutex::new(ProcessSampler::default()));

pub(super) async fn snapshot() -> Result<Vec<ProcessSample>, String> {
    trace!(target: "lucarne::host::process_table", "sampling windows process table");
    let entries = process_entries()?;
    let mut sampler = PROCESS_SAMPLER.lock().expect("process sampler lock");
    let samples = sampler.sample(entries);
    trace!(target: "lucarne::host::process_table", count = samples.len(), "sampled windows process table");
    Ok(samples)
}

fn process_entries() -> Result<Vec<ProcessEntry>, String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateToolhelp32Snapshot"));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    let mut entries = Vec::new();
    let mut ok = unsafe { Process32FirstW(handle(&snapshot), &mut entry) };
    while ok != 0 {
        if let Some(process_entry) = ProcessEntry::from_toolhelp(entry) {
            entries.push(process_entry);
        }
        ok = unsafe { Process32NextW(handle(&snapshot), &mut entry) };
    }
    Ok(entries)
}

#[derive(Debug, Clone, Copy)]
struct ProcessEntry {
    pid: i32,
    parent_pid: Option<i32>,
}

impl ProcessEntry {
    fn from_toolhelp(entry: PROCESSENTRY32W) -> Option<Self> {
        let pid = i32::try_from(entry.th32ProcessID).ok()?;
        if pid <= 0 {
            return None;
        }
        let parent_pid = match i32::try_from(entry.th32ParentProcessID).ok() {
            Some(parent_pid) if parent_pid > 0 => Some(parent_pid),
            _ => None,
        };
        Some(Self { pid, parent_pid })
    }
}

#[derive(Debug, Default)]
struct ProcessSampler {
    previous: HashMap<i32, ProcessCpuSample>,
}

impl ProcessSampler {
    fn sample(&mut self, entries: Vec<ProcessEntry>) -> Vec<ProcessSample> {
        let now = Instant::now();
        let mut current = HashMap::with_capacity(entries.len());
        let mut samples = Vec::with_capacity(entries.len());

        for entry in entries {
            let metrics = process_metrics(entry.pid);
            let total_cpu_100ns = metrics.and_then(|metrics| metrics.total_cpu_100ns);
            let cpu_percent = total_cpu_100ns
                .and_then(|total_cpu_100ns| {
                    current.insert(
                        entry.pid,
                        ProcessCpuSample {
                            total_cpu_100ns,
                            observed_at: now,
                        },
                    );
                    self.cpu_percent(entry.pid, total_cpu_100ns, now)
                })
                .unwrap_or(0.0);
            samples.push(ProcessSample {
                pid: entry.pid,
                parent_pid: entry.parent_pid,
                group_id: None,
                rss_bytes: metrics.map(|metrics| metrics.rss_bytes).unwrap_or(0),
                cpu_percent,
            });
        }

        self.previous = current;
        samples
    }

    fn cpu_percent(&self, pid: i32, total_cpu_100ns: u64, observed_at: Instant) -> Option<f32> {
        let previous = self.previous.get(&pid)?;
        let process_delta = total_cpu_100ns.checked_sub(previous.total_cpu_100ns)?;
        let wall_delta = observed_at.checked_duration_since(previous.observed_at)?;
        let wall_100ns = u64::try_from(wall_delta.as_nanos() / 100).ok()?;
        if wall_100ns == 0 {
            return Some(0.0);
        }
        Some((process_delta as f64 / wall_100ns as f64 * 100.0) as f32)
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcessCpuSample {
    total_cpu_100ns: u64,
    observed_at: Instant,
}

#[derive(Debug, Clone, Copy)]
struct ProcessMetrics {
    rss_bytes: u64,
    total_cpu_100ns: Option<u64>,
}

fn process_metrics(pid: i32) -> Option<ProcessMetrics> {
    if pid <= 0 {
        return None;
    }
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            0,
            pid as u32,
        )
    };
    if process.is_null() {
        return None;
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let rss_bytes = process_working_set_bytes(handle(&process));
    let total_cpu_100ns = process_total_cpu_100ns(handle(&process));
    Some(ProcessMetrics {
        rss_bytes,
        total_cpu_100ns,
    })
}

fn process_working_set_bytes(process: HANDLE) -> u64 {
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    let ok = unsafe {
        GetProcessMemoryInfo(
            process,
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if ok == 0 {
        return 0;
    }
    counters.WorkingSetSize as u64
}

fn process_total_cpu_100ns(process: HANDLE) -> Option<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let ok = unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    Some(filetime_to_u64(kernel).saturating_add(filetime_to_u64(user)))
}

fn filetime_to_u64(filetime: FILETIME) -> u64 {
    (u64::from(filetime.dwHighDateTime) << 32) | u64::from(filetime.dwLowDateTime)
}

fn handle(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle()
}

fn last_error(context: &str) -> String {
    format!("{}: OS error {}", context, unsafe { GetLastError() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_to_u64_combines_high_and_low_words() {
        let filetime = FILETIME {
            dwLowDateTime: 2,
            dwHighDateTime: 1,
        };

        assert_eq!(filetime_to_u64(filetime), (1_u64 << 32) | 2);
    }
}
