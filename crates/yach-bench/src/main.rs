use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdout, Command, Stdio};
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
use yach_bench::fixtures::{
    PayloadScale, TranscriptScale, connected_event, heavy_tool_events, large_paste_payload,
    prompt_delta_events, ready_state_event, transcript_fixture,
};
use yach_bench::latency::LatencySummary;
use yach_bench::replay::{ReplayStep, replay_headless};
use yach_ui::BenchmarkApp;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lines = match args.first().map(String::as_str) {
        Some("headless-report") => headless_report_lines(sample_count(&args)),
        Some("terminal-report") => terminal_report_lines(sample_count(&args)),
        Some("terminal-keypress-report") => terminal_keypress_report_lines(sample_count(&args)),
        Some("terminal-active-stream-report") => {
            terminal_active_stream_report_lines(sample_count(&args))
        }
        Some("terminal-stream-backlog-report") => {
            terminal_stream_backlog_report_lines(sample_count(&args))
        }
        Some("terminal-async-backlog-report") => {
            terminal_async_backlog_report_lines(sample_count(&args), AsyncBacklogProfile::Baseline)
        }
        Some("terminal-async-backlog-stress-report") => {
            terminal_async_backlog_report_lines(sample_count(&args), AsyncBacklogProfile::Stress)
        }
        Some("terminal-heavy-output-report") => {
            terminal_heavy_output_report_lines(sample_count(&args))
        }
        Some("terminal-transcript-scroll-report") => {
            terminal_transcript_scroll_report_lines(sample_count(&args), TranscriptScale::Large)
        }
        Some("terminal-transcript-scroll-stress-report") => {
            terminal_transcript_scroll_report_lines(sample_count(&args), TranscriptScale::Huge)
        }
        Some("pi-clean-startup-report") => pi_clean_startup_report_lines(sample_count(&args)),
        Some("yach-cli-startup-report") => yach_cli_startup_report_lines(sample_count(&args)),
        Some("yach-tui-startup-report") => yach_tui_startup_report_lines(sample_count(&args)),
        Some("yach-tui-ready-startup-report") => {
            yach_tui_ready_startup_report_lines(sample_count(&args))
        }
        _ => usage_lines(),
    };
    let failed = lines.iter().any(|line| {
        line.contains("_error=") || line == "samples_collected=0" || line.contains(" count=0 ")
    });
    let _ = emit_lines(&lines);
    if failed {
        std::process::exit(1);
    }
}

fn sample_count(args: &[String]) -> usize {
    args.windows(2)
        .find_map(|window| {
            if window.first().map(String::as_str) == Some("--samples") {
                window.get(1).and_then(|value| value.parse::<usize>().ok())
            } else {
                None
            }
        })
        .unwrap_or(100)
        .max(1)
}

fn usage_lines() -> Vec<String> {
    vec![String::from(
        "usage: yach-bench headless-report|terminal-report|terminal-keypress-report|terminal-active-stream-report|terminal-stream-backlog-report|terminal-async-backlog-report|terminal-async-backlog-stress-report|terminal-heavy-output-report|terminal-transcript-scroll-report|terminal-transcript-scroll-stress-report|pi-clean-startup-report|yach-cli-startup-report|yach-tui-startup-report|yach-tui-ready-startup-report [--samples N]",
    )]
}

fn emit_lines(lines: &[String]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    for line in lines {
        handle.write_all(line.as_bytes())?;
        handle.write_all(b"\n")?;
    }
    handle.flush()
}

fn headless_report_lines(samples: usize) -> Vec<String> {
    let mut lines = vec![format!("samples={samples}")];

    let workloads = [
        (
            "startup/backend_ready_to_first_interactive_headless",
            sample_startup(samples),
        ),
        (
            "keypress/idle_keypress_to_paint_headless",
            sample_replay(samples, &idle_keypress_steps()),
        ),
        (
            "keypress/active_stream_replay_headless/100",
            sample_replay(samples, &active_stream_steps(100)),
        ),
        (
            "replay/heavy_tool_output_tail_headless/102400",
            sample_replay(samples, &heavy_tool_steps(PayloadScale::Medium)),
        ),
        (
            "paste/large_multiline_component/102400",
            sample_replay(samples, &paste_steps(PayloadScale::Medium)),
        ),
        (
            "viewport/huge_transcript_scroll_headless/10000",
            sample_replay(samples, &transcript_scroll_steps(TranscriptScale::Large)),
        ),
    ];

    for (label, summary) in workloads {
        lines.push(render_summary(label, &summary));
    }

    lines
}

fn terminal_report_lines(samples: usize) -> Vec<String> {
    match sample_live_terminal(samples) {
        Ok(summary) => vec![
            format!("samples={samples}"),
            render_summary("terminal/startup_ready_keypress_draw_flush_live", &summary),
        ],
        Err(error) => vec![format!("terminal_report_error={error}")],
    }
}

fn terminal_keypress_report_lines(samples: usize) -> Vec<String> {
    match sample_live_terminal_keypress(samples) {
        Ok(summary) => vec![
            format!("samples={samples}"),
            render_summary("terminal/idle_keypress_to_draw_flush_live", &summary),
        ],
        Err(error) => vec![format!("terminal_keypress_report_error={error}")],
    }
}

fn terminal_stream_backlog_report_lines(samples: usize) -> Vec<String> {
    match sample_live_terminal_stream_backlog(samples) {
        Ok(summary) => vec![
            format!("samples={samples}"),
            render_summary(
                "terminal/stream_backlog_keypress_to_draw_flush_live",
                &summary,
            ),
        ],
        Err(error) => vec![format!("terminal_stream_backlog_report_error={error}")],
    }
}

fn terminal_heavy_output_report_lines(samples: usize) -> Vec<String> {
    match sample_live_terminal_heavy_output(samples) {
        Ok(summary) => vec![
            format!("samples={samples}"),
            render_summary(
                "terminal/heavy_output_keypress_to_draw_flush_live",
                &summary,
            ),
        ],
        Err(error) => vec![format!("terminal_heavy_output_report_error={error}")],
    }
}

fn terminal_transcript_scroll_report_lines(samples: usize, scale: TranscriptScale) -> Vec<String> {
    match sample_live_terminal_transcript_scroll(samples, scale) {
        Ok(summary) => vec![
            format!("samples={samples}"),
            format!(
                "transcript_entries={} scroll_lines_per_sample=1",
                scale.entries()
            ),
            render_summary(transcript_scroll_workload(scale), &summary),
        ],
        Err(error) => vec![format!("terminal_transcript_scroll_report_error={error}")],
    }
}

fn transcript_scroll_workload(scale: TranscriptScale) -> &'static str {
    match scale {
        TranscriptScale::Huge => "terminal/huge_transcript_scroll_to_draw_flush_live",
        _ => "terminal/large_transcript_scroll_to_draw_flush_live",
    }
}

fn terminal_async_backlog_report_lines(
    samples: usize,
    profile: AsyncBacklogProfile,
) -> Vec<String> {
    match sample_live_terminal_async_backlog(samples, profile) {
        Ok(result) => vec![
            format!("samples={samples}"),
            render_summary(profile.workload(), &result.summary),
            format!(
                "async_backlog_profile={} events_per_burst={} producer_sleep_us={} events_sent={} drained={} max_drained_per_sample={}",
                profile.name(),
                profile.events_per_burst(),
                profile.producer_sleep().as_micros(),
                result.events_sent,
                result.events_drained,
                result.max_drained_per_sample
            ),
        ],
        Err(error) => vec![format!("terminal_async_backlog_report_error={error}")],
    }
}

fn terminal_active_stream_report_lines(samples: usize) -> Vec<String> {
    match sample_live_terminal_active_stream(samples) {
        Ok(summary) => vec![
            format!("samples={samples}"),
            render_summary(
                "terminal/active_stream_keypress_to_draw_flush_live",
                &summary,
            ),
        ],
        Err(error) => vec![format!("terminal_active_stream_report_error={error}")],
    }
}

fn sample_live_terminal(samples: usize) -> io::Result<LatencySummary> {
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

        Ok(LatencySummary::from_samples(None, &durations))
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_stream_backlog(samples: usize) -> io::Result<LatencySummary> {
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

        Ok(LatencySummary::from_samples(None, &durations))
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[derive(Debug, Clone, Copy)]
enum AsyncBacklogProfile {
    Baseline,
    Stress,
}

impl AsyncBacklogProfile {
    const fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Stress => "stress",
        }
    }

    const fn workload(self) -> &'static str {
        match self {
            Self::Baseline => "terminal/async_backlog_keypress_to_draw_flush_live",
            Self::Stress => "terminal/async_backlog_stress_keypress_to_draw_flush_live",
        }
    }

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

struct AsyncBacklogResult {
    summary: LatencySummary,
    events_sent: usize,
    events_drained: usize,
    max_drained_per_sample: usize,
}

fn sample_live_terminal_async_backlog(
    samples: usize,
    profile: AsyncBacklogProfile,
) -> io::Result<AsyncBacklogResult> {
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
        let mut events_drained = 0;
        let mut max_drained_per_sample = 0;
        for i in 0..samples {
            std::thread::sleep(Duration::from_micros(500));
            let start = std::time::Instant::now();
            let mut drained_this_sample = 0;
            while let Ok(event) = rx.try_recv() {
                app.handle_backend_event(event);
                drained_this_sample += 1;
            }
            events_drained += drained_this_sample;
            max_drained_per_sample = max_drained_per_sample.max(drained_this_sample);
            let key = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
            app.handle_key(KeyCode::Char(key), KeyModifiers::empty());
            app.render_live_terminal(&mut terminal)?;
            durations.push(start.elapsed());
        }

        let events_sent = producer
            .join()
            .map_err(|_| io::Error::other("async backlog producer panicked"))?;
        while let Ok(event) = rx.try_recv() {
            app.handle_backend_event(event);
            events_drained += 1;
        }
        Ok(AsyncBacklogResult {
            summary: LatencySummary::from_samples(None, &durations),
            events_sent,
            events_drained,
            max_drained_per_sample,
        })
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_transcript_scroll(
    samples: usize,
    scale: TranscriptScale,
) -> io::Result<LatencySummary> {
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

        Ok(LatencySummary::from_samples(None, &durations))
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_heavy_output(samples: usize) -> io::Result<LatencySummary> {
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

        Ok(LatencySummary::from_samples(None, &durations))
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_active_stream(samples: usize) -> io::Result<LatencySummary> {
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

        Ok(LatencySummary::from_samples(None, &durations))
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn sample_live_terminal_keypress(samples: usize) -> io::Result<LatencySummary> {
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

        Ok(LatencySummary::from_samples(None, &durations))
    })();

    let restore_result = restore_terminal();
    match (result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
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

fn yach_tui_ready_startup_report_lines(samples: usize) -> Vec<String> {
    yach_tui_startup_report_lines_for(
        samples,
        "tui-bench-ready",
        "yach/tui_ready_startup_first_output_pty",
    )
}

fn yach_tui_startup_report_lines(samples: usize) -> Vec<String> {
    yach_tui_startup_report_lines_for(samples, "tui", "yach/tui_startup_first_output_pty")
}

fn yach_tui_startup_report_lines_for(samples: usize, command: &str, workload: &str) -> Vec<String> {
    let mut durations = Vec::with_capacity(samples);
    let mut errors = Vec::new();

    for _ in 0..samples {
        match sample_yach_tui_first_output(command) {
            Ok(duration) => durations.push(duration),
            Err(error) => errors.push(error.to_string()),
        }
    }

    let mut lines = vec![format!("samples_requested={samples}")];
    lines.push(format!("samples_collected={}", durations.len()));
    if !errors.is_empty() {
        lines.push(format!("errors={}", errors.len()));
        if let Some(first_error) = errors.first() {
            lines.push(format!("first_error={first_error}"));
        }
    }
    let summary = LatencySummary::from_samples(None, &durations);
    lines.push(render_summary(workload, &summary));
    lines
}

fn sample_yach_tui_first_output(command: &str) -> io::Result<Duration> {
    let bin = resolve_yach_cli_bin()?;
    let start = std::time::Instant::now();
    let mut child = Command::new("script")
        .args(["-q", "/dev/null", &bin, command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing script stdout"))?;
    let read_result = read_first_byte_with_timeout(stdout, Duration::from_secs(5));
    let elapsed = start.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    read_result.map(|()| elapsed)
}

fn yach_cli_startup_report_lines(samples: usize) -> Vec<String> {
    let mut durations = Vec::with_capacity(samples);
    let mut errors = Vec::new();

    for _ in 0..samples {
        match sample_yach_cli_first_output() {
            Ok(duration) => durations.push(duration),
            Err(error) => errors.push(error.to_string()),
        }
    }

    let mut lines = vec![format!("samples_requested={samples}")];
    lines.push(format!("samples_collected={}", durations.len()));
    if !errors.is_empty() {
        lines.push(format!("errors={}", errors.len()));
        if let Some(first_error) = errors.first() {
            lines.push(format!("first_error={first_error}"));
        }
    }
    let summary = LatencySummary::from_samples(None, &durations);
    lines.push(render_summary("yach/cli_startup_first_output", &summary));
    lines
}

fn sample_yach_cli_first_output() -> io::Result<Duration> {
    let bin = resolve_yach_cli_bin()?;
    let start = std::time::Instant::now();
    let mut child = Command::new(bin)
        .arg("--quiet")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing yach stdout"))?;
    let read_result = read_first_byte_with_timeout(stdout, Duration::from_secs(5));
    let elapsed = start.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    read_result.map(|()| elapsed)
}

fn pi_clean_startup_report_lines(samples: usize) -> Vec<String> {
    let mut durations = Vec::with_capacity(samples);
    let mut errors = Vec::new();

    for _ in 0..samples {
        match sample_pi_clean_first_output() {
            Ok(duration) => durations.push(duration),
            Err(error) => errors.push(error.to_string()),
        }
    }

    let mut lines = vec![format!("samples_requested={samples}")];
    lines.push(format!("samples_collected={}", durations.len()));
    if !errors.is_empty() {
        lines.push(format!("errors={}", errors.len()));
        if let Some(first_error) = errors.first() {
            lines.push(format!("first_error={first_error}"));
        }
    }
    let summary = LatencySummary::from_samples(None, &durations);
    lines.push(render_summary(
        "pi/clean_startup_first_output_pty",
        &summary,
    ));
    lines
}

fn sample_pi_clean_first_output() -> io::Result<Duration> {
    let start = std::time::Instant::now();
    let mut child = Command::new("script")
        .args([
            "-q",
            "/dev/null",
            "pi",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
            "--no-context-files",
            "--offline",
            "--no-session",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing script stdout"))?;
    let read_result = read_first_byte_with_timeout(stdout, Duration::from_secs(5));
    let elapsed = start.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    read_result.map(|()| elapsed)
}

fn resolve_yach_cli_bin() -> io::Result<String> {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_yach_cli") {
        return Ok(path);
    }

    let current_exe = std::env::current_exe()?;
    let candidate: PathBuf = current_exe
        .parent()
        .ok_or_else(|| io::Error::other("unable to resolve current executable directory"))?
        .join("yach-cli");
    if candidate.exists() {
        return Ok(candidate.to_string_lossy().into_owned());
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "yach-cli binary not found; set CARGO_BIN_EXE_yach_cli or build {}",
            candidate.display()
        ),
    ))
}

fn read_first_byte_with_timeout(mut stdout: ChildStdout, timeout: Duration) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut first_byte = [0_u8; 1];
        let result = stdout.read_exact(&mut first_byte);
        let _ = tx.send(result);
    });

    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for first output byte",
        ))
    })
}

fn render_summary(label: &str, summary: &LatencySummary) -> String {
    format!(
        "workload={label} count={} p50={} p95={} p99={} max={}",
        summary.count,
        render_duration(summary.p50),
        render_duration(summary.p95),
        render_duration(summary.p99),
        render_duration(summary.max),
    )
}

fn render_duration(duration: Option<Duration>) -> String {
    let Some(duration) = duration else {
        return String::from("no-data");
    };
    let nanos = duration.as_nanos();
    if nanos >= 1_000_000 {
        format_decimal_duration(nanos, 1_000_000, "ms")
    } else if nanos >= 1_000 {
        format_decimal_duration(nanos, 1_000, "us")
    } else {
        format!("{nanos}ns")
    }
}

fn format_decimal_duration(nanos: u128, divisor: u128, suffix: &str) -> String {
    let whole = nanos / divisor;
    let fractional = (nanos % divisor).saturating_mul(1_000) / divisor;
    format!("{whole}.{fractional:03}{suffix}")
}

fn sample_replay(samples: usize, steps: &[ReplayStep]) -> LatencySummary {
    let durations: Vec<Duration> = (0..samples)
        .map(|_| {
            let result = replay_headless(steps, 100, 30);
            result.samples.into_iter().sum()
        })
        .collect();
    LatencySummary::from_samples(None, &durations)
}

fn sample_startup(samples: usize) -> LatencySummary {
    let durations: Vec<Duration> = (0..samples)
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
        .collect();
    LatencySummary::from_samples(None, &durations)
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
