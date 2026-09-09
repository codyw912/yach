use std::io;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use yach_ui::BenchmarkApp;

use crate::fixtures::{
    PayloadScale, TranscriptScale, connected_event, heavy_tool_events, large_paste_payload,
    prompt_delta_events, ready_state_event, transcript_fixture,
};
use crate::perf::registry::{Measured, Requirement, Workload};
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
                samples: sample_replay(
                    ctx.samples,
                    &transcript_scroll_steps(TranscriptScale::Large),
                ),
                alloc: None,
            })
        },
    },
];

pub static LIVE: [Workload; 9] = [
    Workload {
        id: "terminal/startup_ready_keypress_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| live_latency(sample_live_terminal(ctx.samples)),
    },
    Workload {
        id: "terminal/idle_keypress_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| live_latency(sample_live_terminal_keypress(ctx.samples)),
    },
    Workload {
        id: "terminal/active_stream_keypress_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| live_latency(sample_live_terminal_active_stream(ctx.samples)),
    },
    Workload {
        id: "terminal/stream_backlog_keypress_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| live_latency(sample_live_terminal_stream_backlog(ctx.samples)),
    },
    Workload {
        id: "terminal/async_backlog_keypress_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessThreaded,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| {
            live_latency(sample_live_terminal_async_backlog(
                ctx.samples,
                AsyncBacklogProfile::Baseline,
            ))
        },
    },
    Workload {
        id: "terminal/async_backlog_stress_keypress_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessThreaded,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| {
            live_latency(sample_live_terminal_async_backlog(
                ctx.samples,
                AsyncBacklogProfile::Stress,
            ))
        },
    },
    Workload {
        id: "terminal/heavy_output_keypress_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| live_latency(sample_live_terminal_heavy_output(ctx.samples)),
    },
    Workload {
        id: "terminal/large_transcript_scroll_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| {
            live_latency(sample_live_terminal_transcript_scroll(
                ctx.samples,
                TranscriptScale::Large,
            ))
        },
    },
    Workload {
        id: "terminal/huge_transcript_scroll_to_draw_flush_live",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[Requirement::Tty],
        bin: None,
        run: |ctx| {
            live_latency(sample_live_terminal_transcript_scroll(
                ctx.samples,
                TranscriptScale::Huge,
            ))
        },
    },
];

fn live_latency(result: io::Result<Vec<Duration>>) -> Result<Measured, String> {
    result
        .map(|samples| Measured::Latency {
            samples,
            alloc: None,
        })
        .map_err(|error| error.to_string())
}

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

fn sample_live_terminal(samples: usize) -> io::Result<Vec<Duration>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let result = (|| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let mut durations = Vec::with_capacity(samples);

        for _ in 0..samples {
            let mut app = BenchmarkApp::new();
            app.handle_backend_event(connected_event());
            let start = std::time::Instant::now();
            app.handle_backend_event(ready_state_event());
            app.render_live_terminal(&mut terminal)?;
            app.handle_key(KeyCode::Char('x'), KeyModifiers::empty());
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        Ok(durations)
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(durations), Ok(())) => Ok(durations),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_stream_backlog(samples: usize) -> io::Result<Vec<Duration>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let result = (|| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let mut app = BenchmarkApp::new();
        app.handle_backend_event(connected_event());
        app.handle_backend_event(ready_state_event());
        app.render_live_terminal(&mut terminal)?;

        let mut durations = Vec::with_capacity(samples);
        for i in 0..samples {
            let start = std::time::Instant::now();
            for event in prompt_delta_events(10) {
                app.handle_backend_event(event);
            }
            let key = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
            app.handle_key(KeyCode::Char(key), KeyModifiers::empty());
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        Ok(durations)
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(durations), Ok(())) => Ok(durations),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[derive(Debug, Clone, Copy)]
enum AsyncBacklogProfile {
    Baseline,
    Stress,
}

impl AsyncBacklogProfile {
    const fn events_per_burst(self) -> usize {
        match self {
            Self::Baseline => 10,
            Self::Stress => 50,
        }
    }

    const fn producer_sleep(self) -> Duration {
        match self {
            Self::Baseline => Duration::from_micros(500),
            Self::Stress => Duration::from_micros(100),
        }
    }
}

fn sample_live_terminal_async_backlog(
    samples: usize,
    profile: AsyncBacklogProfile,
) -> io::Result<Vec<Duration>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let result = (|| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let mut app = BenchmarkApp::new();
        app.handle_backend_event(connected_event());
        app.handle_backend_event(ready_state_event());
        app.render_live_terminal(&mut terminal)?;

        let events_per_burst = profile.events_per_burst();
        let producer_sleep = profile.producer_sleep();
        let (tx, rx) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            let mut sent = 0;
            for _ in 0..samples {
                for event in prompt_delta_events(events_per_burst) {
                    if tx.send(event).is_err() {
                        return sent;
                    }
                    sent += 1;
                }
                std::thread::sleep(producer_sleep);
            }
            sent
        });

        let mut durations = Vec::with_capacity(samples);
        for i in 0..samples {
            std::thread::sleep(Duration::from_micros(500));
            let start = std::time::Instant::now();
            while let Ok(event) = rx.try_recv() {
                app.handle_backend_event(event);
            }
            let key = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
            app.handle_key(KeyCode::Char(key), KeyModifiers::empty());
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        let _events_sent = producer
            .join()
            .map_err(|_| io::Error::other("async backlog producer panicked"))?;
        while let Ok(event) = rx.try_recv() {
            app.handle_backend_event(event);
        }
        Ok(durations)
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(durations), Ok(())) => Ok(durations),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_transcript_scroll(
    samples: usize,
    scale: TranscriptScale,
) -> io::Result<Vec<Duration>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let result = (|| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let mut app = BenchmarkApp::new();
        app.handle_backend_event(connected_event());
        app.handle_backend_event(ready_state_event());
        app.set_transcript(transcript_fixture(scale));
        app.render_live_terminal(&mut terminal)?;

        let mut durations = Vec::with_capacity(samples);
        for _ in 0..samples {
            let start = std::time::Instant::now();
            app.scroll_down(1);
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        Ok(durations)
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(durations), Ok(())) => Ok(durations),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_heavy_output(samples: usize) -> io::Result<Vec<Duration>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let result = (|| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let mut app = BenchmarkApp::new();
        app.handle_backend_event(connected_event());
        app.handle_backend_event(ready_state_event());
        for event in heavy_tool_events(PayloadScale::Large) {
            app.handle_backend_event(event);
        }
        app.render_live_terminal(&mut terminal)?;

        let mut durations = Vec::with_capacity(samples);
        for i in 0..samples {
            let key = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
            let start = std::time::Instant::now();
            app.handle_key(KeyCode::Char(key), KeyModifiers::empty());
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        Ok(durations)
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(durations), Ok(())) => Ok(durations),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_active_stream(samples: usize) -> io::Result<Vec<Duration>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let result = (|| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let mut app = BenchmarkApp::new();
        app.handle_backend_event(connected_event());
        app.handle_backend_event(ready_state_event());
        for event in prompt_delta_events(100) {
            app.handle_backend_event(event);
        }
        app.render_live_terminal(&mut terminal)?;

        let mut durations = Vec::with_capacity(samples);
        for i in 0..samples {
            if let Some(event) = prompt_delta_events(1).into_iter().next() {
                app.handle_backend_event(event);
            }
            let key = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
            let start = std::time::Instant::now();
            app.handle_key(KeyCode::Char(key), KeyModifiers::empty());
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        Ok(durations)
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(durations), Ok(())) => Ok(durations),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_keypress(samples: usize) -> io::Result<Vec<Duration>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(Hide)?;

    let result = (|| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let mut app = BenchmarkApp::new();
        app.handle_backend_event(connected_event());
        app.handle_backend_event(ready_state_event());
        app.render_live_terminal(&mut terminal)?;

        let mut durations = Vec::with_capacity(samples);
        for i in 0..samples {
            let key = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
            let start = std::time::Instant::now();
            app.handle_key(KeyCode::Char(key), KeyModifiers::empty());
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        Ok(durations)
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(durations), Ok(())) => Ok(durations),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn restore_terminal() -> io::Result<()> {
    let mut first_error = None;
    let mut stdout = io::stdout();
    if let Err(error) = stdout.execute(Show) {
        first_error = Some(error);
    }
    if let Err(error) = stdout.execute(LeaveAlternateScreen)
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    if let Err(error) = disable_raw_mode()
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(())
    }
}
