use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use yach_ui::BenchmarkApp;

use crate::fixtures::{
    PayloadScale, TranscriptScale, connected_event, heavy_tool_events, large_paste_payload,
    prompt_delta_events, ready_state_event, transcript_fixture,
};
use crate::perf::registry::{Measured, Workload};
use crate::perf::schema::{Class, Isolation};
use crate::replay::{ReplayStep, replay_headless};

pub static HEADLESS: [Workload; 6] = [
    Workload {
        id: "startup/backend_ready_to_first_interactive_headless",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| {
            Ok(Measured::Latency {
                samples: sample_startup(ctx.samples),
                alloc: None,
            })
        },
    },
    Workload {
        id: "keypress/idle_keypress_to_paint_headless",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| {
            Ok(Measured::Latency {
                samples: sample_replay(ctx.samples, &idle_keypress_steps()),
                alloc: None,
            })
        },
    },
    Workload {
        id: "keypress/active_stream_replay_headless/100",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| {
            Ok(Measured::Latency {
                samples: sample_replay(ctx.samples, &active_stream_steps(100)),
                alloc: None,
            })
        },
    },
    Workload {
        id: "replay/heavy_tool_output_tail_headless/102400",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| {
            Ok(Measured::Latency {
                samples: sample_replay(ctx.samples, &heavy_tool_steps(PayloadScale::Medium)),
                alloc: None,
            })
        },
    },
    Workload {
        id: "paste/large_multiline_component/102400",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| {
            Ok(Measured::Latency {
                samples: sample_replay(ctx.samples, &paste_steps(PayloadScale::Medium)),
                alloc: None,
            })
        },
    },
    Workload {
        id: "viewport/huge_transcript_scroll_headless/10000",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| {
            Ok(Measured::Latency {
                samples: sample_replay(ctx.samples, &transcript_scroll_steps(TranscriptScale::Large)),
                alloc: None,
            })
        },
    },
];

fn sample_replay(samples: usize, steps: &[ReplayStep]) -> Vec<Duration> {
    (0..samples)
        .map(|_| {
            let result = replay_headless(steps, 100, 30);
            result.samples.into_iter().sum()
        })
        .collect()
}

fn sample_startup(samples: usize) -> Vec<Duration> {
    (0..samples)
        .map(|_| {
            let mut app = BenchmarkApp::new();
            app.handle_backend_event(connected_event());
            let start = std::time::Instant::now();
            app.handle_backend_event(ready_state_event());
            app.render_headless(100, 30);
            app.handle_key(KeyCode::Char('x'), KeyModifiers::empty());
            app.render_headless(100, 30);
            start.elapsed()
        })
        .collect()
}

fn idle_keypress_steps() -> Vec<ReplayStep> {
    vec![
        ReplayStep::Backend(connected_event()),
        ReplayStep::Key {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::empty(),
        },
    ]
}

fn active_stream_steps(count: usize) -> Vec<ReplayStep> {
    let mut steps = vec![ReplayStep::Backend(connected_event())];
    steps.extend(
        prompt_delta_events(count)
            .into_iter()
            .map(ReplayStep::Backend),
    );
    steps.push(ReplayStep::Key {
        code: KeyCode::Char('x'),
        modifiers: KeyModifiers::empty(),
    });
    steps
}

fn heavy_tool_steps(scale: PayloadScale) -> Vec<ReplayStep> {
    let mut steps = vec![ReplayStep::Backend(connected_event())];
    steps.extend(
        heavy_tool_events(scale)
            .into_iter()
            .map(ReplayStep::Backend),
    );
    steps
}

fn paste_steps(scale: PayloadScale) -> Vec<ReplayStep> {
    vec![
        ReplayStep::Backend(connected_event()),
        ReplayStep::PromptText(large_paste_payload(scale)),
    ]
}

fn transcript_scroll_steps(scale: TranscriptScale) -> Vec<ReplayStep> {
    vec![
        ReplayStep::Backend(connected_event()),
        ReplayStep::Transcript(transcript_fixture(scale)),
        ReplayStep::ScrollDown(20),
    ]
}
