use std::collections::{HashMap, HashSet};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProcessSample {
    pub(crate) pid: i32,
    pub(crate) parent_pid: Option<i32>,
    pub(crate) group_id: Option<i32>,
    pub(crate) rss_bytes: u64,
    pub(crate) cpu_percent: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProcessAggregate {
    pub(crate) process_count: usize,
    pub(crate) memory_bytes: u64,
    pub(crate) cpu_percent: f32,
}

pub(crate) async fn snapshot() -> Result<Vec<ProcessSample>, String> {
    #[cfg(unix)]
    {
        unix::snapshot().await
    }
    #[cfg(windows)]
    {
        windows::snapshot().await
    }
}

pub(crate) fn aggregate_for_root(root_pid: i32, samples: &[ProcessSample]) -> ProcessAggregate {
    let mut pids = descendants_of(root_pid, &children_by_parent(samples));
    pids.insert(root_pid);
    for sample in samples {
        if sample.group_id == Some(root_pid) {
            pids.insert(sample.pid);
        }
    }

    let mut aggregate = ProcessAggregate {
        process_count: 0,
        memory_bytes: 0,
        cpu_percent: 0.0,
    };
    for sample in samples {
        if pids.contains(&sample.pid) {
            aggregate.process_count += 1;
            aggregate.memory_bytes = aggregate.memory_bytes.saturating_add(sample.rss_bytes);
            aggregate.cpu_percent += sample.cpu_percent;
        }
    }
    aggregate
}

fn children_by_parent(samples: &[ProcessSample]) -> HashMap<i32, Vec<i32>> {
    let mut children = HashMap::<i32, Vec<i32>>::new();
    for sample in samples {
        if let Some(parent_pid) = sample.parent_pid {
            children.entry(parent_pid).or_default().push(sample.pid);
        }
    }
    children
}

fn descendants_of(root_pid: i32, children_by_parent: &HashMap<i32, Vec<i32>>) -> HashSet<i32> {
    let mut descendants = HashSet::new();
    let mut stack = children_by_parent
        .get(&root_pid)
        .cloned()
        .unwrap_or_default();
    while let Some(pid) = stack.pop() {
        if !descendants.insert(pid) {
            continue;
        }
        if let Some(children) = children_by_parent.get(&pid) {
            stack.extend(children.iter().copied());
        }
    }
    descendants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_for_root_counts_unix_group_and_descendants() {
        let samples = vec![
            ProcessSample {
                pid: 10,
                parent_pid: Some(1),
                group_id: Some(10),
                rss_bytes: 1024,
                cpu_percent: 1.0,
            },
            ProcessSample {
                pid: 11,
                parent_pid: Some(10),
                group_id: Some(10),
                rss_bytes: 2048,
                cpu_percent: 2.5,
            },
            ProcessSample {
                pid: 12,
                parent_pid: Some(1),
                group_id: Some(10),
                rss_bytes: 4096,
                cpu_percent: 0.5,
            },
            ProcessSample {
                pid: 20,
                parent_pid: Some(1),
                group_id: Some(20),
                rss_bytes: 8192,
                cpu_percent: 9.0,
            },
        ];

        let aggregate = aggregate_for_root(10, &samples);

        assert_eq!(aggregate.process_count, 3);
        assert_eq!(aggregate.memory_bytes, 7168);
        assert_eq!(aggregate.cpu_percent, 4.0);
    }

    #[test]
    fn aggregate_for_root_counts_windows_descendants_without_group_id() {
        let samples = vec![
            ProcessSample {
                pid: 10,
                parent_pid: None,
                group_id: None,
                rss_bytes: 1024,
                cpu_percent: 1.0,
            },
            ProcessSample {
                pid: 11,
                parent_pid: Some(10),
                group_id: None,
                rss_bytes: 2048,
                cpu_percent: 2.5,
            },
            ProcessSample {
                pid: 12,
                parent_pid: Some(11),
                group_id: None,
                rss_bytes: 4096,
                cpu_percent: 0.5,
            },
            ProcessSample {
                pid: 20,
                parent_pid: None,
                group_id: None,
                rss_bytes: 8192,
                cpu_percent: 9.0,
            },
        ];

        let aggregate = aggregate_for_root(10, &samples);

        assert_eq!(aggregate.process_count, 3);
        assert_eq!(aggregate.memory_bytes, 7168);
        assert_eq!(aggregate.cpu_percent, 4.0);
    }
}
