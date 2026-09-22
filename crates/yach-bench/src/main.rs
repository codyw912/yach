use std::io::{self, Write};
use std::process::ExitCode;

#[global_allocator]
static ALLOC: yach_bench::perf::alloc::Counting = yach_bench::perf::alloc::Counting;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (lines, code) = match args.first().map(String::as_str) {
        Some("perf") => match yach_bench::perf::dispatch(&args[1..]) {
            Ok(outcome) => (outcome.lines, outcome.exit_code),
            Err(message) => (vec![format!("error: {message}")], 1),
        },
        Some("eval-review") => match yach_bench::eval_review::dispatch(&args[1..]) {
            Ok(lines) => (lines, 0),
            Err(message) => (vec![format!("error: {message}")], 1),
        },
        _ => (
            vec![String::from(
                "usage: yach-bench perf run|worker|ab|report|host-fingerprint … | eval-review --corpus <dir> --reviewer fixture|jev --out <json>",
            )],
            2,
        ),
    };
    let _ = emit_lines(&lines);
    ExitCode::from(code)
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
