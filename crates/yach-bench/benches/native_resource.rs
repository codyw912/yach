use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use yach_backend::{NativeResourceContextPolicy, NativeResourceRoot, NativeResourceSearchPolicy};

fn bench_native_resource(c: &mut Criterion) {
    let root_path = fixture_project();
    let Ok(root) = NativeResourceRoot::project(&root_path) else {
        process::abort();
    };

    c.bench_function("native_resource_path_metadata", |b| {
        b.iter(|| root.path_metadata("src/file-010.rs"));
    });
    c.bench_function("native_resource_context_10_files", |b| {
        b.iter(|| {
            root.read_context_package(
                (0..10).map(|index| format!("src/file-{index:03}.rs")),
                NativeResourceContextPolicy {
                    max_file_bytes: 4096,
                    max_files: 16,
                },
            )
        });
    });
    c.bench_function("native_resource_search_100_files", |b| {
        b.iter(|| root.search_text("needle", NativeResourceSearchPolicy::small()));
    });

    let _ = fs::remove_dir_all(root_path);
}

fn fixture_project() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let root = std::env::temp_dir().join(format!("yach-native-resource-bench-{unique}"));
    let src = root.join("src");
    if fs::create_dir_all(&src).is_err() {
        process::abort();
    }
    for index in 0..100 {
        let content = if index % 10 == 0 {
            format!("pub fn file_{index}() {{}}\n// needle\n")
        } else {
            format!("pub fn file_{index}() {{}}\n")
        };
        if fs::write(src.join(format!("file-{index:03}.rs")), content).is_err() {
            process::abort();
        }
    }
    root
}

criterion_group!(benches, bench_native_resource);
criterion_main!(benches);
