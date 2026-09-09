pub mod alloc;
pub mod provenance;
pub mod registry;
pub mod schema;
pub mod worker;
pub mod workloads;

pub struct Outcome {
    pub lines: Vec<String>,
    pub exit_code: u8,
}

pub(crate) const USAGE: &str = "usage: yach-bench perf run|worker|external-sampler|ab|report [--samples N] [--filter GLOB] [--out FILE]";

pub fn dispatch(args: &[String]) -> Result<Outcome, String> {
    let Some((command, rest)) = args.split_first() else {
        return Ok(usage_outcome());
    };
    match command.as_str() {
        "worker" => worker::cmd_worker(rest),
        "external-sampler" => worker::cmd_external_sampler(rest),
        "run" => worker::cmd_run(rest),
        _ => Ok(usage_outcome()),
    }
}

fn usage_outcome() -> Outcome {
    Outcome {
        lines: vec![String::from(USAGE)],
        exit_code: 2,
    }
}
