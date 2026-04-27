use std::time::Instant;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use crossterm::event::{KeyCode, KeyModifiers};
use yach_bench::fixtures::{connected_event, ready_state_event};
use yach_ui::BenchmarkApp;

fn bench_backend_ready_to_first_interactive_headless(c: &mut Criterion) {
    c.bench_function("startup/backend_ready_to_first_interactive_headless", |b| {
        b.iter(|| {
            let mut app = BenchmarkApp::new();
            app.handle_backend_event(black_box(connected_event()));

            let start = Instant::now();
            app.handle_backend_event(black_box(ready_state_event()));
            app.render_headless(100, 30);
            app.handle_key(KeyCode::Char('x'), KeyModifiers::empty());
            app.render_headless(100, 30);
            black_box(start.elapsed())
        });
    });
}

criterion_group!(benches, bench_backend_ready_to_first_interactive_headless);
criterion_main!(benches);
