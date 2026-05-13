use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use yach_backend::{NativeStaticContextPolicy, assemble_project_static_context};

static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempProject {
    root: PathBuf,
    cwd: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Option<Self> {
        let sequence = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "yach-native-static-context-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).ok()?;
        Some(Self {
            cwd: root.clone(),
            root,
        })
    }

    fn with_cwd(mut self, relative_path: &str) -> Option<Self> {
        self.cwd = self.root.join(relative_path);
        fs::create_dir_all(&self.cwd).ok()?;
        Some(self)
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn write(&self, relative_path: &str, content: &str) -> Option<()> {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        fs::write(path, content).ok()
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assemble_context(project: Option<TempProject>) {
    let Some(project) = project else {
        black_box(false);
        return;
    };
    black_box(assemble_project_static_context(
        project.root(),
        project.cwd(),
        NativeStaticContextPolicy::conservative(),
    ));
}

fn bench_static_context_empty(c: &mut Criterion) {
    c.bench_function("native_static_context_empty", |b| {
        b.iter_batched(
            || TempProject::new("empty"),
            assemble_context,
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_one_agents(c: &mut Criterion) {
    c.bench_function("native_static_context_one_agents", |b| {
        b.iter_batched(
            || {
                let project = TempProject::new("one-agents")?;
                project.write("AGENTS.md", "root rules\n")?;
                Some(project)
            },
            assemble_context,
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_nested_agents(c: &mut Criterion) {
    c.bench_function("native_static_context_nested_agents", |b| {
        b.iter_batched(
            || {
                let project = TempProject::new("nested-agents")?.with_cwd("crates/backend/src")?;
                project.write("AGENTS.md", "root rules\n")?;
                project.write("crates/AGENTS.md", "crates rules\n")?;
                project.write("crates/backend/AGENTS.md", "backend rules\n")?;
                Some(project)
            },
            assemble_context,
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_append_system(c: &mut Criterion) {
    c.bench_function("native_static_context_append_system", |b| {
        b.iter_batched(
            || {
                let project = TempProject::new("append-system")?;
                project.write("AGENTS.md", "root rules\n")?;
                project.write(".yach/APPEND_SYSTEM.md", "system rules\n")?;
                Some(project)
            },
            assemble_context,
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
