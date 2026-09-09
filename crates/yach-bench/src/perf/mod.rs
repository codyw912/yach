pub mod alloc;
pub mod provenance;
pub mod registry;
pub mod schema;
pub mod workloads;

pub struct Outcome {
    pub lines: Vec<String>,
    pub exit_code: u8,
}

pub fn dispatch(_args: &[String]) -> Result<Outcome, String> {
    Ok(Outcome {
        lines: vec![String::from(
            "usage: yach-bench perf run|worker|ab|report [--samples N] [--filter GLOB] [--out FILE]",
        )],
        exit_code: 2,
    })
}
