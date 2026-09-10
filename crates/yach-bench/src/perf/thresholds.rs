use std::path::Path;

use glob::Pattern;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    pub latency_pct: f64,
    pub memory_pct: f64,
    pub size_pct: f64,
    pub count: u64,
}

#[derive(Debug)]
struct Workload {
    id: String,
    pattern: Pattern,
    latency_pct: Option<f64>,
    memory_pct: Option<f64>,
    size_pct: Option<f64>,
    count: Option<u64>,
}

#[derive(Debug)]
pub struct Thresholds {
    defaults: Budget,
    workloads: Vec<Workload>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    defaults: BudgetFile,
    #[serde(default)]
    workload: Vec<WorkloadFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetFile {
    latency_pct: f64,
    memory_pct: f64,
    size_pct: f64,
    count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkloadFile {
    id: String,
    latency_pct: Option<f64>,
    memory_pct: Option<f64>,
    size_pct: Option<f64>,
    count: Option<u64>,
    #[expect(dead_code, reason = "comments are retained only to validate the file schema")]
    comment: Option<String>,
}

impl From<BudgetFile> for Budget {
    fn from(value: BudgetFile) -> Self {
        Self {
            latency_pct: value.latency_pct,
            memory_pct: value.memory_pct,
            size_pct: value.size_pct,
            count: value.count,
        }
    }
}

impl Thresholds {
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            defaults: Budget {
                latency_pct: 5.0,
                memory_pct: 10.0,
                size_pct: 0.5,
                count: 0,
            },
            workloads: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let file: File = toml::from_str(text).map_err(|error| format!("parse thresholds: {error}"))?;
        let defaults = file.defaults.into();
        let mut workloads = Vec::with_capacity(file.workload.len());
        for row in file.workload {
            let WorkloadFile {
                id,
                latency_pct,
                memory_pct,
                size_pct,
                count,
                comment: _,
            } = row;
            let pattern = Pattern::new(&id)
                .map_err(|error| format!("invalid workload glob {id:?}: {error}"))?;
            workloads.push(Workload {
                id,
                pattern,
                latency_pct,
                memory_pct,
                size_pct,
                count,
            });
        }
        Ok(Self {
            defaults,
            workloads,
        })
    }
    #[must_use]
    pub fn for_id(&self, id: &str) -> Budget {
        let Some((_, row)) = self
            .workloads
            .iter()
            .enumerate()
            .filter(|(_, row)| row.pattern.matches(id))
            .max_by_key(|(_, row)| literal_prefix_len(&row.id))
        else {
            return self.defaults;
        };
        Budget {
            latency_pct: row.latency_pct.unwrap_or(self.defaults.latency_pct),
            memory_pct: row.memory_pct.unwrap_or(self.defaults.memory_pct),
            size_pct: row.size_pct.unwrap_or(self.defaults.size_pct),
            count: row.count.unwrap_or(self.defaults.count),
        }
    }

    #[must_use]
    pub fn unmatched_rows(&self, ids: &[&str]) -> Vec<String> {
        self.workloads
            .iter()
            .filter(|row| !ids.iter().any(|id| row.pattern.matches(id)))
            .map(|row| row.id.clone())
            .collect()
    }
}

fn literal_prefix_len(pattern: &str) -> usize {
    pattern
        .chars()
        .take_while(|character| !matches!(character, '*' | '?' | '['))
        .count()
}

#[cfg(test)]
mod tests {
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }
    use super::Thresholds;

    const TOML: &str = r#"
[defaults]
latency_pct = 5.0
memory_pct = 10.0
size_pct = 0.5
count = 0

[[workload]]
id = "turn/scripted/*"
latency_pct = 8.0
comment = "tokio scheduling"

[[workload]]
id = "turn/scripted/tools_4/*"
latency_pct = 12.0
"#;

    #[test]
    fn most_specific_glob_wins_and_defaults_fill() {
        let t = Thresholds::parse(TOML);
        assert!(t.is_ok(), "{t:?}");
        let Ok(t) = t else { return };
        assert!(
            close(t.for_id("request/assemble/10_turns").latency_pct, 5.0),
            "got {}",
            t.for_id("request/assemble/10_turns").latency_pct
        );
        assert!(
            close(t.for_id("turn/scripted/text_only").latency_pct, 8.0),
            "got {}",
            t.for_id("turn/scripted/text_only").latency_pct
        );
        assert!(
            close(t.for_id("turn/scripted/tools_4/builtin").latency_pct, 12.0),
            "got {}",
            t.for_id("turn/scripted/tools_4/builtin").latency_pct
        );
        assert!(
            close(t.for_id("turn/scripted/tools_4/builtin").memory_pct, 10.0),
            "got {}",
            t.for_id("turn/scripted/tools_4/builtin").memory_pct
        );
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(Thresholds::parse(
            "[defaults]\nlatency_pct = 5.0\nmemory_pct = 1.0\nsize_pct = 1.0\ncount = 0\n[[workload]]\nid = \"x\"\naccept = \"nope\"\n"
        )
        .is_err());
    }

    #[test]
    fn unmatched_rows_are_reported() {
        let t = Thresholds::parse(TOML);
        assert!(t.is_ok(), "{t:?}");
        let Ok(t) = t else { return };
        let unmatched = t.unmatched_rows(&["request/assemble/10_turns", "turn/scripted/text_only"]);
        assert_eq!(unmatched, vec![String::from("turn/scripted/tools_4/*")]);
    }

    #[test]
    fn loads_repository_thresholds() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("perf-thresholds.toml");
        let t = Thresholds::load(&path);
        assert!(t.is_ok(), "{t:?}");
        let Ok(t) = t else { return };
        assert!(
            close(t.for_id("turn/scripted/text_only").latency_pct, 8.0),
            "got {}",
            t.for_id("turn/scripted/text_only").latency_pct
        );
    }
}
