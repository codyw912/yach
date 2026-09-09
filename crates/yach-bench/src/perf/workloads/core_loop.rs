use std::hint::black_box;
use std::time::Instant;

use crate::perf::alloc::AllocWindow;
use crate::perf::registry::{Measured, RunCtx, Workload};
use crate::perf::schema::{Class, Isolation};

pub static CORE_LOOP: [Workload; 3] = [
    Workload {
        id: "request/assemble/10_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| Ok(assemble_workload(10, ctx)),
    },
    Workload {
        id: "request/assemble/100_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| Ok(assemble_workload(100, ctx)),
    },
    Workload {
        id: "request/assemble/1000_turns",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| Ok(assemble_workload(1000, ctx)),
    },
];

fn assemble_workload(turns: usize, ctx: &RunCtx) -> Measured {
    let log = yach_backend::request_assembly::fixture_log(turns, 1);
    let current = yach_backend::TurnId(format!("turn-{}", turns + 1));
    let mut samples = Vec::with_capacity(ctx.samples);
    let window = AllocWindow::begin();
    for _ in 0..ctx.samples {
        let start = Instant::now();
        let messages = yach_backend::request_assembly::assemble(&log, &current, None);
        samples.push(start.elapsed());
        black_box(messages);
    }
    let alloc = window.end();
    Measured::Latency {
        samples,
        alloc: Some(alloc),
    }
}

