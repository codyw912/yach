use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead as _, Write};
use std::path::{Component, Path};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

pub const MANIFEST_JSON: &str = include_str!("../yach.extension.json");

const PROTOCOL: &str = "yach.extension-host.v2";
const EXTENSION_ID: &str = "yach.hashline";
const RESOURCE_MAX_BYTES: u64 = 32 * 1024;
const MAX_FILES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchSection {
    path: String,
    expected_tag: String,
    operations: Vec<PatchOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchOperation {
    PutRange {
        start: usize,
        end: usize,
        lines: Vec<String>,
    },
    PutGap {
        gap: PatchGap,
        lines: Vec<String>,
    },
    Cut {
        start: usize,
        end: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PatchGap {
    Before(usize),
    After(usize),
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchParseError {
    PutBodyPrefix,
    CutTrailingColon,
    Syntax,
}

impl PatchParseError {
    const fn message(self) -> &'static str {
        match self {
            Self::PutBodyPrefix => {
                "malformed hashline patch: every PUT body row must begin with '+'; \
                 example: PUT 3.=3:\n+replacement text"
            }
            Self::CutTrailingColon => {
                "malformed hashline patch: CUT hunks do not take a trailing ':' or body; \
                 use CUT 3.=3"
            }
            Self::Syntax => {
                "malformed hashline patch: expected PUT <locator>: followed by '+' body rows, \
                 or CUT N.=M without ':'"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    path: String,
    normalized_text: String,
    full_sha256: String,
}

#[derive(Debug)]
struct VerifiedSection {
    patch: PatchSection,
    snapshot: Snapshot,
}

#[derive(Debug)]
enum PendingInvocation {
    Read {
        tool_request_id: String,
    },
    Edit {
        tool_request_id: String,
        sections: Vec<VerifiedSection>,
        section_index: usize,
        operations: Vec<Value>,
    },
}

pub fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut pending = BTreeMap::<String, PendingInvocation>::new();
    let mut snapshots = BTreeMap::<String, Vec<Snapshot>>::new();

    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match message.get("type").and_then(Value::as_str) {
            Some("extension.initialize") => send_registration(&mut stdout)?,
            Some("tool.invoke") => {
                handle_tool_invoke(&mut stdout, &mut pending, &snapshots, &message)?;
            }
            Some("resource.result") => {
                handle_resource_result(&mut stdout, &mut pending, &mut snapshots, &message)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn send_registration(output: &mut impl Write) -> io::Result<()> {
    send(
        output,
        &json!({
            "type": "extension.ready",
            "protocol": PROTOCOL,
            "extension_id": EXTENSION_ID
        }),
    )?;
    send(
        output,
        &json!({
            "type": "tool.register",
            "name": "hashline_read",
            "description": "Read a project text file as [path#TAG] followed by one-based numbered lines. TAG is the first 16 hex digits of the whole-file SHA-256.",
            "risk": "reads_local_content",
            "provider_visible": true,
            "input_schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {"path": {"type": "string"}},
                "maxSerializedBytes": 1024
            }
        }),
    )?;
    send(
        output,
        &json!({
            "type": "tool.register",
            "name": "hashline_edit",
            "description": "Apply a line-anchored patch. Each section starts with [path#TAG]. Supported hunks: PUT N.=M:, PUT <N:, PUT >N:, PUT >$:, and CUT N.=M. Every PUT body row MUST begin with '+'; for example: PUT 3.=3:\\n+replacement text. '+' is patch syntax, not file content. Locators address the original snapshot, and every section tag must resolve in this live host.",
            "risk": "mutates_local_state",
            "provider_visible": true,
            "input_schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["input"],
                "properties": {"input": {"type": "string"}},
                "maxSerializedBytes": 49152
            }
        }),
    )
}

fn handle_tool_invoke(
    output: &mut impl Write,
    pending: &mut BTreeMap<String, PendingInvocation>,
    snapshots: &BTreeMap<String, Vec<Snapshot>>,
    message: &Value,
) -> io::Result<()> {
    let Some(request_id) = message.get("request_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(name) = message.get("name").and_then(Value::as_str) else {
        return send_failure(output, request_id, "invalid_request", "missing tool name");
    };
    let Some(arguments) = message.get("arguments") else {
        return send_failure(
            output,
            request_id,
            "invalid_request",
            "missing tool arguments",
        );
    };
    match name {
        "hashline_read" => {
            let Some(path) = arguments.get("path").and_then(Value::as_str) else {
                return send_failure(output, request_id, "invalid_request", "missing path");
            };
            let resource_id = format!("{request_id}:read");
            pending.insert(
                resource_id.clone(),
                PendingInvocation::Read {
                    tool_request_id: request_id.to_owned(),
                },
            );
            send_resource_request(output, &resource_id, path)
        }
        "hashline_edit" => {
            let Some(input) = arguments.get("input").and_then(Value::as_str) else {
                return send_failure(output, request_id, "invalid_request", "missing patch input");
            };
            let parsed_sections = match parse_patch(input) {
                Ok(sections) => sections,
                Err(error) => {
                    return send_failure(output, request_id, "malformed_patch", error.message());
                }
            };
            let Ok(sections) = resolve_sections(parsed_sections, snapshots) else {
                return send_failure(
                    output,
                    request_id,
                    "unknown_snapshot",
                    "unknown, ambiguous, or path-mismatched snapshot tag",
                );
            };
            let resource_id = format!("{request_id}:edit:0");
            let path = sections[0].snapshot.path.clone();
            pending.insert(
                resource_id.clone(),
                PendingInvocation::Edit {
                    tool_request_id: request_id.to_owned(),
                    sections,
                    section_index: 0,
                    operations: Vec::new(),
                },
            );
            send_resource_request(output, &resource_id, &path)
        }
        _ => send_failure(
            output,
            request_id,
            "invalid_request",
            "unknown hashline tool",
        ),
    }
}

fn handle_resource_result(
    output: &mut impl Write,
    pending: &mut BTreeMap<String, PendingInvocation>,
    snapshots: &mut BTreeMap<String, Vec<Snapshot>>,
    message: &Value,
) -> io::Result<()> {
    let Some(resource_id) = message.get("request_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(invocation) = pending.remove(resource_id) else {
        return Ok(());
    };
    let Some(result) = message.get("result") else {
        return fail_pending(
            output,
            &invocation,
            "malformed_resource_result",
            "missing resource result",
        );
    };
    if result.get("status").and_then(Value::as_str) != Some("completed") {
        let reason = result
            .get("reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("resource_read_failed");
        let message = result
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty())
            .unwrap_or("resource read failed");
        return fail_pending(output, &invocation, reason, message);
    }
    let Some(text) = result.get("text").and_then(Value::as_str) else {
        return fail_pending(
            output,
            &invocation,
            "malformed_resource_result",
            "missing resource text",
        );
    };
    let Some(sha256) = result.get("sha256").and_then(Value::as_str) else {
        return fail_pending(
            output,
            &invocation,
            "malformed_resource_result",
            "missing resource hash",
        );
    };
    let Some(canonical_path) = result.get("path").and_then(Value::as_str) else {
        return fail_pending(
            output,
            &invocation,
            "malformed_resource_result",
            "missing resource path",
        );
    };

    let normalized_text = normalize_text(text);
    match invocation {
        PendingInvocation::Read { tool_request_id } => {
            let tag = snapshot_tag(&normalized_text);
            let snapshot = Snapshot {
                path: canonical_path.to_owned(),
                normalized_text: normalized_text.clone(),
                full_sha256: sha256.to_owned(),
            };
            let candidates = snapshots.entry(tag).or_default();
            if !candidates.contains(&snapshot) {
                candidates.push(snapshot);
            }
            send_tool_result(
                output,
                &tool_request_id,
                &render_hashline_file(canonical_path, &normalized_text),
            )
        }
        PendingInvocation::Edit {
            tool_request_id,
            sections,
            section_index,
            mut operations,
        } => {
            let section = &sections[section_index];
            if section.snapshot.path != canonical_path
                || section.snapshot.normalized_text != normalized_text
            {
                return send_failure(
                    output,
                    &tool_request_id,
                    "snapshot_stale",
                    "snapshot is stale",
                );
            }
            let Ok(after_text) = apply_operations(&normalized_text, &section.patch.operations)
            else {
                return send_failure(
                    output,
                    &tool_request_id,
                    "invalid_line_range",
                    "invalid line range",
                );
            };
            if after_text == normalized_text {
                return send_failure(
                    output,
                    &tool_request_id,
                    "no_op",
                    "hashline patch is a no-op",
                );
            }
            operations.push(json!({
                "kind": "modify_text_file",
                "path": section.snapshot.path,
                "expected_sha256": sha256,
                "after_text": after_text
            }));
            let next_index = section_index + 1;
            if next_index == sections.len() {
                return send(
                    output,
                    &json!({
                        "type": "tool.edit_proposal",
                        "request_id": tool_request_id,
                        "summary": format!("Apply hashline patch to {} file(s)", sections.len()),
                        "operations": operations
                    }),
                );
            }
            let resource_id = format!("{tool_request_id}:edit:{next_index}");
            let path = sections[next_index].snapshot.path.clone();
            pending.insert(
                resource_id.clone(),
                PendingInvocation::Edit {
                    tool_request_id,
                    sections,
                    section_index: next_index,
                    operations,
                },
            );
            send_resource_request(output, &resource_id, &path)
        }
    }
}

fn resolve_sections(
    sections: Vec<PatchSection>,
    snapshots: &BTreeMap<String, Vec<Snapshot>>,
) -> Result<Vec<VerifiedSection>, ()> {
    sections
        .into_iter()
        .map(|patch| {
            let candidates = snapshots.get(&patch.expected_tag).ok_or(())?;
            if candidates.len() != 1 || candidates[0].path != patch.path {
                return Err(());
            }
            Ok(VerifiedSection {
                snapshot: candidates[0].clone(),
                patch,
            })
        })
        .collect()
}

fn parse_patch(input: &str) -> Result<Vec<PatchSection>, PatchParseError> {
    let lines = input.lines().collect::<Vec<_>>();
    let mut sections = Vec::new();
    let mut paths = BTreeSet::new();
    let mut index = 0;
    while index < lines.len() {
        let (path, expected_tag) =
            parse_header(lines[index]).map_err(|()| PatchParseError::Syntax)?;
        if !paths.insert(path.clone()) || sections.len() == MAX_FILES {
            return Err(PatchParseError::Syntax);
        }
        index += 1;
        let mut operations = Vec::new();
        while index < lines.len() && !lines[index].starts_with('[') {
            let line = lines[index];
            if let Some(locator) = line
                .strip_prefix("PUT ")
                .and_then(|value| value.strip_suffix(':'))
            {
                index += 1;
                let mut body = Vec::new();
                while index < lines.len() {
                    let Some(content) = lines[index].strip_prefix('+') else {
                        break;
                    };
                    body.push(content.to_owned());
                    index += 1;
                }
                if body.is_empty() {
                    return Err(PatchParseError::PutBodyPrefix);
                }
                if let Some(line) = locator.strip_prefix('<') {
                    operations.push(PatchOperation::PutGap {
                        gap: PatchGap::Before(
                            parse_line(line).map_err(|()| PatchParseError::Syntax)?,
                        ),
                        lines: body,
                    });
                } else if locator == ">$" {
                    operations.push(PatchOperation::PutGap {
                        gap: PatchGap::End,
                        lines: body,
                    });
                } else if let Some(line) = locator.strip_prefix('>') {
                    operations.push(PatchOperation::PutGap {
                        gap: PatchGap::After(
                            parse_line(line).map_err(|()| PatchParseError::Syntax)?,
                        ),
                        lines: body,
                    });
                } else {
                    let (start, end) =
                        parse_range(locator).map_err(|()| PatchParseError::Syntax)?;
                    operations.push(PatchOperation::PutRange {
                        start,
                        end,
                        lines: body,
                    });
                }
            } else if let Some(range) = line.strip_prefix("CUT ") {
                if range.ends_with(':') {
                    return Err(PatchParseError::CutTrailingColon);
                }
                let (start, end) = parse_range(range).map_err(|()| PatchParseError::Syntax)?;
                operations.push(PatchOperation::Cut { start, end });
                index += 1;
            } else {
                return Err(PatchParseError::Syntax);
            }
        }
        if operations.is_empty() {
            return Err(PatchParseError::Syntax);
        }
        sections.push(PatchSection {
            path,
            expected_tag,
            operations,
        });
    }
    if sections.is_empty() {
        Err(PatchParseError::Syntax)
    } else {
        Ok(sections)
    }
}

fn parse_header(line: &str) -> Result<(String, String), ()> {
    let inner = line
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or(())?;
    let (path, tag) = inner.rsplit_once('#').ok_or(())?;
    let valid_path = !path.is_empty()
        && !Path::new(path).is_absolute()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    let valid_tag = tag.len() == 16
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte));
    if !valid_path || !valid_tag {
        return Err(());
    }
    Ok((path.to_owned(), tag.to_owned()))
}

fn parse_line(value: &str) -> Result<usize, ()> {
    let line = value.parse::<usize>().map_err(|_| ())?;
    if line == 0 { Err(()) } else { Ok(line) }
}

fn parse_range(value: &str) -> Result<(usize, usize), ()> {
    let (start, end) = value.split_once(".=").ok_or(())?;
    let start = parse_line(start)?;
    let end = parse_line(end)?;
    if end < start {
        return Err(());
    }
    Ok((start, end))
}

fn apply_operations(text: &str, operations: &[PatchOperation]) -> Result<String, ()> {
    let trailing_newline = text.ends_with('\n');
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if trailing_newline {
        lines.pop();
    }
    let line_count = lines.len();
    let mut ranges = BTreeMap::<usize, (usize, Vec<String>)>::new();
    let mut gaps = BTreeMap::<usize, Vec<String>>::new();
    for operation in operations {
        match operation {
            PatchOperation::PutRange { start, end, lines } => {
                if *end > line_count || ranges.insert(*start, (*end, lines.clone())).is_some() {
                    return Err(());
                }
            }
            PatchOperation::Cut { start, end } => {
                if *end > line_count || ranges.insert(*start, (*end, Vec::new())).is_some() {
                    return Err(());
                }
            }
            PatchOperation::PutGap { gap, lines } => {
                let gap = match gap {
                    PatchGap::Before(line) if *line <= line_count => line - 1,
                    PatchGap::After(line) if *line <= line_count => *line,
                    PatchGap::End => line_count,
                    PatchGap::Before(_) | PatchGap::After(_) => return Err(()),
                };
                if gaps.insert(gap, lines.clone()).is_some() {
                    return Err(());
                }
            }
        }
    }
    let mut previous_end = 0;
    for (&start, &(end, _)) in &ranges {
        if start <= previous_end || gaps.keys().any(|gap| *gap > start - 1 && *gap < end) {
            return Err(());
        }
        previous_end = end;
    }

    let mut output_lines = Vec::new();
    if let Some(inserted) = gaps.get(&0) {
        output_lines.extend(inserted.iter().cloned());
    }
    let mut line = 1;
    while line <= line_count {
        if let Some((end, replacement)) = ranges.get(&line) {
            output_lines.extend(replacement.iter().cloned());
            line = end + 1;
        } else {
            output_lines.push(lines[line - 1].to_owned());
            line += 1;
        }
        if let Some(inserted) = gaps.get(&(line - 1)) {
            output_lines.extend(inserted.iter().cloned());
        }
    }
    let mut output = output_lines.join("\n");
    if trailing_newline && !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn render_hashline_file(path: &str, normalized_text: &str) -> String {
    let mut rendered = format!("[{path}#{}]", snapshot_tag(normalized_text));
    if normalized_text.is_empty() {
        rendered.push_str("\n(empty file)");
        return rendered;
    }
    for (index, line) in normalized_text.lines().enumerate() {
        rendered.push('\n');
        rendered.push_str(&(index + 1).to_string());
        rendered.push(':');
        rendered.push_str(line);
    }
    rendered
}

fn snapshot_tag(normalized_text: &str) -> String {
    let digest = format!("{:X}", Sha256::digest(normalized_text.as_bytes()));
    digest[..16].to_owned()
}

fn send_resource_request(output: &mut impl Write, request_id: &str, path: &str) -> io::Result<()> {
    send(
        output,
        &json!({
            "type": "resource.request",
            "request_id": request_id,
            "operation": {
                "kind": "read_text_file",
                "path": path,
                "max_bytes": RESOURCE_MAX_BYTES
            }
        }),
    )
}

fn fail_pending(
    output: &mut impl Write,
    invocation: &PendingInvocation,
    reason: &str,
    message: &str,
) -> io::Result<()> {
    let request_id = match invocation {
        PendingInvocation::Read {
            tool_request_id, ..
        }
        | PendingInvocation::Edit {
            tool_request_id, ..
        } => tool_request_id,
    };
    send_failure(output, request_id, reason, message)
}

fn send_failure(
    output: &mut impl Write,
    request_id: &str,
    reason: &str,
    message: &str,
) -> io::Result<()> {
    send(
        output,
        &json!({
            "type": "tool.result",
            "request_id": request_id,
            "status": "failed",
            "reason": reason,
            "content": format!("[hashline error: {message}]")
        }),
    )
}

fn send_tool_result(output: &mut impl Write, request_id: &str, content: &str) -> io::Result<()> {
    send(
        output,
        &json!({
            "type": "tool.result",
            "request_id": request_id,
            "status": "completed",
            "content": content
        }),
    )
}

fn send(output: &mut impl Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashline_read_and_edit_share_the_whole_file_tag() {
        let original = "alpha\nbeta\n";
        let tag = snapshot_tag(original);
        assert_eq!(
            render_hashline_file("src/lib.rs", original),
            format!("[src/lib.rs#{tag}]\n1:alpha\n2:beta")
        );
        let patch = parse_patch(&format!("[src/lib.rs#{tag}]\nPUT 2.=2:\n+gamma"));
        assert!(patch.is_ok());
        let Some(section) = patch.ok().and_then(|mut sections| sections.pop()) else {
            return;
        };
        assert_eq!(
            apply_operations(original, &section.operations),
            Ok(String::from("alpha\ngamma\n"))
        );
    }

    #[test]
    fn hashline_edit_guidance_explains_put_body_prefixes() {
        let mut registration = Vec::new();
        assert!(send_registration(&mut registration).is_ok());
        let registration = String::from_utf8_lossy(&registration);
        assert!(registration.contains("Every PUT body row MUST begin with '+'"));

        let request = json!({
            "type": "tool.invoke",
            "request_id": "tool-request-1",
            "name": "hashline_edit",
            "arguments": {
                "input": "[src/lib.rs#8AF354630C30311B]\nPUT 3.=3:\nreplacement text"
            }
        });
        let mut output = Vec::new();
        assert!(
            handle_tool_invoke(
                &mut output,
                &mut BTreeMap::new(),
                &BTreeMap::new(),
                &request,
            )
            .is_ok()
        );
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("every PUT body row must begin with '+'"));
    }

    #[test]
    fn hashline_edit_guidance_distinguishes_cut_trailing_colon() {
        let request = json!({
            "type": "tool.invoke",
            "request_id": "tool-request-1",
            "name": "hashline_edit",
            "arguments": {
                "input": "[src/lib.rs#8AF354630C30311B]\nCUT 3.=3:"
            }
        });
        let mut output = Vec::new();
        assert!(
            handle_tool_invoke(
                &mut output,
                &mut BTreeMap::new(),
                &BTreeMap::new(),
                &request,
            )
            .is_ok()
        );
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("CUT hunks do not take a trailing ':'"));
        assert!(!output.contains("every PUT body row"));
    }

    #[test]
    fn hashline_patch_rejects_overlapping_original_ranges() {
        let tag = snapshot_tag("alpha\nbeta\n");
        let patch = parse_patch(&format!("[src/lib.rs#{tag}]\nPUT 1.=2:\n+gamma\nCUT 2.=2"));
        let result = patch
            .ok()
            .and_then(|mut sections| sections.pop())
            .map(|section| apply_operations("alpha\nbeta\n", &section.operations));
        assert_eq!(result, Some(Err(())));
    }

    #[test]
    fn hashline_patch_supports_multiple_files() {
        let first = snapshot_tag("alpha\n");
        let second = snapshot_tag("beta\n");
        let patch = parse_patch(&format!(
            "[a.txt#{first}]\nPUT 1.=1:\n+one\n[b.txt#{second}]\nCUT 1.=1"
        ));
        assert_eq!(patch.map(|sections| sections.len()), Ok(2));
    }

    #[test]
    fn hashline_patch_supports_original_snapshot_gaps() {
        let tag = snapshot_tag("alpha\nbeta\n");
        let patch = parse_patch(&format!(
            "[src/lib.rs#{tag}]\nPUT <1:\n+before\nPUT >1:\n+middle\nPUT >$:\n+after"
        ));
        let Some(section) = patch.ok().and_then(|mut sections| sections.pop()) else {
            return;
        };
        assert_eq!(
            apply_operations("alpha\nbeta\n", &section.operations),
            Ok(String::from("before\nalpha\nmiddle\nbeta\nafter\n"))
        );
    }

    #[test]
    fn snapshot_resolution_requires_one_matching_live_snapshot() {
        let text = normalize_text("alpha\r\n");
        let tag = snapshot_tag(&text);
        let section = PatchSection {
            path: String::from("src/lib.rs"),
            expected_tag: tag.clone(),
            operations: vec![PatchOperation::Cut { start: 1, end: 1 }],
        };
        let snapshot = Snapshot {
            path: String::from("src/lib.rs"),
            normalized_text: text,
            full_sha256: String::from("full"),
        };
        let mut snapshots = BTreeMap::from([(tag.clone(), vec![snapshot.clone()])]);
        assert!(resolve_sections(vec![section.clone()], &snapshots).is_ok());

        if let Some(candidates) = snapshots.get_mut(&tag) {
            candidates.push(Snapshot {
                path: String::from("other.rs"),
                normalized_text: String::from("other\n"),
                full_sha256: String::from("other"),
            });
        }
        assert!(resolve_sections(vec![section], &snapshots).is_err());
    }

    #[test]
    fn hashline_read_normalizes_crlf_and_marks_empty_files() {
        let normalized = normalize_text("alpha\r\nbeta\r\n");
        assert_eq!(normalized, "alpha\nbeta\n");
        assert_eq!(
            render_hashline_file("empty.txt", ""),
            format!("[empty.txt#{}]\n(empty file)", snapshot_tag(""))
        );
    }
}
