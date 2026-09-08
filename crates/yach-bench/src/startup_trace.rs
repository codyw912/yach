use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupTraceMark {
    pub label: String,
    pub elapsed: Duration,
}

pub fn parse_startup_trace_marks(contents: &str) -> Result<Vec<StartupTraceMark>, String> {
    match yach_trace::parse_records(contents) {
        Ok(records) => Ok(startup_marks(records)),
        Err(yach_trace::TraceParseError::TruncatedLine { .. }) => match contents.rfind('\n') {
            Some(index) => parse_startup_trace_marks(&contents[..=index]),
            None => Ok(Vec::new()),
        },
        Err(yach_trace::TraceParseError::Malformed { line_no, message }) => {
            Err(format!("trace line {line_no}: {message}"))
        }
    }
}

fn startup_marks(records: Vec<yach_trace::TraceRecord>) -> Vec<StartupTraceMark> {
    records
        .into_iter()
        .filter(|record| record.scope == "startup")
        .map(|record| StartupTraceMark {
            label: record.label,
            elapsed: Duration::from_micros(record.t_us),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{StartupTraceMark, parse_startup_trace_marks};

    #[test]
    fn parses_jsonl_startup_records() {
        let marks = parse_startup_trace_marks(
            "{\"t_us\":0,\"scope\":\"startup\",\"label\":\"process_main_start\"}\n{\"t_us\":128,\"scope\":\"startup\",\"label\":\"cli_args_parsed\"}\n{\"t_us\":2500,\"scope\":\"startup\",\"label\":\"tui_first_render_end\"}\n",
        );

        assert_eq!(
            marks,
            Ok(vec![
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
            ])
        );
    }

    #[test]
    fn ignores_turn_scope_and_tolerates_truncated_trailing_line() {
        let marks = parse_startup_trace_marks(
            "{\"t_us\":0,\"scope\":\"startup\",\"label\":\"process_main_start\"}\n{\"t_us\":10,\"scope\":\"turn\",\"turn_id\":\"t1\",\"label\":\"prompt_received\"}\n{\"t_us\":128,\"scope\":\"startup\",\"label\":\"cli_args_parsed\"}\n{\"t_us\":2500,\"scope\":\"startup\",\"label\":\"tui_first_render_end",
        );

        assert_eq!(
            marks,
            Ok(vec![
                StartupTraceMark {
                    label: String::from("process_main_start"),
                    elapsed: Duration::from_micros(0),
                },
                StartupTraceMark {
                    label: String::from("cli_args_parsed"),
                    elapsed: Duration::from_micros(128),
                },
            ])
        );
    }

    #[test]
    fn malformed_line_returns_err_with_line_number() {
        let result = parse_startup_trace_marks("not-json\n");
        assert!(result.is_err());
        if let Err(message) = result {
            assert!(message.contains("trace line 1"));
        }
    }
}


