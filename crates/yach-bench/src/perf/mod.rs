pub mod thresholds;
pub mod verdict;
pub mod alloc;
pub mod provenance;
pub mod registry;
pub mod rss;
pub mod schema;
pub mod worker;
pub mod ab;
pub mod report;

pub mod workloads;

pub struct Outcome {
    pub lines: Vec<String>,
    pub exit_code: u8,
}
pub(crate) const USAGE: &str = "usage: yach-bench perf run|worker|external-sampler|ab|report|host-fingerprint [--samples N] [--filter GLOB] [--out FILE]";

pub fn dispatch(args: &[String]) -> Result<Outcome, String> {
    let Some((command, rest)) = args.split_first() else {
        return Ok(usage_outcome());
    };
    match command.as_str() {
        "worker" => worker::cmd_worker(rest),
        "external-sampler" => worker::cmd_external_sampler(rest),
        "ab" => ab::cmd_ab(rest),
        "run" => worker::cmd_run(rest),
        "report" => report::cmd_report(rest),
        "host-fingerprint" => Ok(Outcome {
            lines: vec![provenance::capture_host().fingerprint],
            exit_code: 0,
        }),
        "__alloc-and-wait" => cmd_alloc_and_wait(rest),
        _ => Ok(usage_outcome()),
    }
}

fn usage_outcome() -> Outcome {
    Outcome {
        lines: vec![String::from(USAGE)],
        exit_code: 2,
    }
}

fn cmd_alloc_and_wait(args: &[String]) -> Result<Outcome, String> {
    let raw = args
        .first()
        .ok_or_else(|| String::from("missing byte count"))?;
    let bytes = raw
        .parse::<usize>()
        .map_err(|error| format!("invalid byte count: {error}"))?;
    rss::alloc_and_wait(bytes)
}
