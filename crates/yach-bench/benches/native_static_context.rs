use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use yach_backend::{NativeStaticContextPolicy, assemble_project_static_context};

static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempProject {
    root: PathBuf,
    cwd: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let sequence = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "yach-native-static-context-{label}-{}-{sequence}",
            std::process::id()
        ));
        if fs::create_dir_all(&root).is_err() {
            process::abort();
        }
        Self {
            cwd: root.clone(),
            root,
        }
    }

    fn with_cwd(mut self, relative_path: &str) -> Self {
        self.cwd = self.root.join(relative_path);
        if fs::create_dir_all(&self.cwd).is_err() {
            process::abort();
        }
        self
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn write(&self, relative_path: &str, content: &str) {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                process::abort();
            }
        }
        if fs::write(path, content).is_err() {
            process::abort();
        }
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assemble_context(project: &TempProject) {
    black_box(assemble_project_static_context(
        project.root(),
        project.cwd(),
        NativeStaticContextPolicy::conservative(),
    ));
}

fn bench_static_context_empty(c: &mut Criterion) {
    c.bench_function("native_static_context_empty", |b| {
        b.iter_batched_ref(
            || TempProject::new("empty"),
            |project| assemble_context(project),
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_one_agents(c: &mut Criterion) {
    c.bench_function("native_static_context_one_agents", |b| {
        b.iter_batched_ref(
            || {
                let project = TempProject::new("one-agents");
                project.write("AGENTS.md", "root rules\n");
                project
            },
            |project| assemble_context(project),
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_nested_agents(c: &mut Criterion) {
    c.bench_function("native_static_context_nested_agents", |b| {
        b.iter_batched_ref(
            || {
                let project = TempProject::new("nested-agents").with_cwd("crates/backend/src");
                project.write("AGENTS.md", "root rules\n");
                project.write("crates/AGENTS.md", "crates rules\n");
                project.write("crates/backend/AGENTS.md", "backend rules\n");
                project
            },
            |project| assemble_context(project),
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_append_system(c: &mut Criterion) {
    c.bench_function("native_static_context_append_system", |b| {
        b.iter_batched_ref(
            || {
                let project = TempProject::new("append-system");
                project.write("AGENTS.md", "root rules\n");
                project.write(".yach/APPEND_SYSTEM.md", "system rules\n");
                project
            },
            |project| assemble_context(project),
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_static_context_empty,
    bench_static_context_one_agents,
    bench_static_context_nested_agents,
    bench_static_context_append_system,
);
criterion_main!(benches);
