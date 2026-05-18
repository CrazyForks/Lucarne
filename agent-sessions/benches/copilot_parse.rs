use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_sessions::ParseSelection;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
struct Sample {
    label: String,
    bytes: Vec<u8>,
}

fn copilot_benchmarks(c: &mut Criterion) {
    let provider = agent_sessions::agent_provider("copilot").expect("copilot provider");
    let samples = load_samples();
    assert!(
        !samples.is_empty(),
        "no Copilot CLI event samples found; set AGENT_SESSIONS_BENCH_COPILOT_SAMPLES or populate ~/.copilot/session-state"
    );

    let mut group = c.benchmark_group("copilot_parse");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(10));

    for sample in &samples {
        group.throughput(Throughput::Bytes(sample.bytes.len() as u64));
        for (name, selection) in [
            ("semantic_full", ParseSelection::full()),
            ("semantic_messages", ParseSelection::empty().with_messages()),
        ] {
            group.bench_with_input(
                BenchmarkId::new(name, &sample.label),
                sample,
                |b, sample| {
                    b.iter(|| {
                        let parsed = provider
                            .parse_agent_session_bytes(sample.bytes.clone(), selection)
                            .unwrap();
                        black_box(parsed.events.len())
                    });
                },
            );
        }
    }

    group.finish();
}

fn load_samples() -> Vec<Sample> {
    let paths = explicit_sample_paths().unwrap_or_else(discover_largest_samples);
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap_or_else(|err| {
                panic!("failed to read benchmark sample {}: {err}", path.display())
            });
            Sample {
                label: sample_label(&path),
                bytes,
            }
        })
        .collect()
}

fn explicit_sample_paths() -> Option<Vec<PathBuf>> {
    let raw = env::var_os("AGENT_SESSIONS_BENCH_COPILOT_SAMPLES")?;
    let paths: Vec<_> = env::split_paths(&raw).collect();
    if paths.is_empty() { None } else { Some(paths) }
}

fn discover_largest_samples() -> Vec<PathBuf> {
    let top_n = env::var("AGENT_SESSIONS_BENCH_TOP_N")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(3);

    let root = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".copilot")
        .join("session-state");

    let mut entries = Vec::new();
    for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file()
            || path.file_name().and_then(|name| name.to_str()) != Some("events.jsonl")
        {
            continue;
        }

        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        entries.push((metadata.len(), path.to_path_buf()));
    }

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    entries
        .into_iter()
        .take(top_n)
        .map(|(_, path)| path)
        .collect()
}

fn sample_label(path: &Path) -> String {
    path.parent()
        .and_then(|path| path.file_name())
        .and_then(|stem| stem.to_str())
        .unwrap_or("sample")
        .to_owned()
}

criterion_group!(benches, copilot_benchmarks);
criterion_main!(benches);
