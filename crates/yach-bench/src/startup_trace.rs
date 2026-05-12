use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupTraceMark {
    pub label: String,
    pub elapsed: Duration,
}

#[must_use]
pub fn parse_startup_trace_marks(contents: &str) -> Vec<StartupTraceMark> {
    contents
        .lines()
        .filter_map(|line| {
            let (elapsed, label) = line.split_once(' ')?;
            let elapsed = elapsed.parse::<u64>().ok()?;
            let label = label.trim();
            if label.is_empty() {
                return None;
            }
            Some(StartupTraceMark {
                label: label.to_owned(),
                elapsed: Duration::from_micros(elapsed),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{StartupTraceMark, parse_startup_trace_marks};

    #[test]
    fn parses_elapsed_microsecond_trace_lines() {
        let marks = parse_startup_trace_marks(
            "0 process_main_start\n128 cli_args_parsed\n2500 tui_first_render_end\n",
        );

        assert_eq!(
            marks,
            vec![
                StartupTraceMark {
                    label: String::from("process_main_start"),
                    elapsed: Duration::from_micros(0),
                },
                StartupTraceMark {
                    label: String::from("cli_args_parsed"),
                    elapsed: Duration::from_micros(128),
                },
                StartupTraceMark {
                    label: String::from("tui_first_render_end"),
                    elapsed: Duration::from_micros(2500),
                },
            ]
        );
    }
}
