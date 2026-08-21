use yach_proto::SessionMessage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTree {
    pub nodes: Vec<SessionTreeNode>,
    pub branches: Vec<BranchSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeNode {
    pub index: usize,
    pub entry_id: Option<String>,
    pub role: String,
    pub preview: String,
    pub is_branch_root: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSummary {
    pub root_entry_id: Option<String>,
    pub title: String,
    pub message_count: usize,
}

#[must_use]
pub fn build_session_tree(messages: &[SessionMessage]) -> SessionTree {
    let mut nodes = Vec::with_capacity(messages.len());
    let mut branches = Vec::new();
    let mut current_branch: Option<BranchSummary> = None;

    for (index, message) in messages.iter().enumerate() {
        // Harness-authored outcome rows (failed/cancelled turns) are
        // transcript display artifacts, not conversation nodes: they must
        // not appear in tree navigation, inflate branch counts, or seed a
        // branch root.
        if message.role == "harness" {
            continue;
        }
        let is_branch_root = message.role == "user" || current_branch.is_none();

        if is_branch_root {
            if let Some(branch) = current_branch.take() {
                branches.push(branch);
            }
            current_branch = Some(BranchSummary {
                root_entry_id: message.entry_id.clone(),
                title: preview(&message.text, 72),
                message_count: 0,
            });
        }

        if let Some(branch) = current_branch.as_mut() {
            branch.message_count += 1;
        }

        nodes.push(SessionTreeNode {
            index,
            entry_id: message.entry_id.clone(),
            role: message.role.clone(),
            preview: preview(&message.text, 96),
            is_branch_root,
        });
    }

    if let Some(branch) = current_branch {
        branches.push(branch);
    }

    SessionTree { nodes, branches }
}

#[must_use]
pub fn branch_summary_line(tree: &SessionTree) -> String {
    let branch_count = tree.branches.len();
    let message_count = tree.nodes.len();
    match (branch_count, message_count) {
        (0, 0) => String::from("session tree: empty"),
        _ => format!("session tree: {branch_count} branches · {message_count} messages"),
    }
}

fn preview(text: &str, limit: usize) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview: String = flattened.chars().take(limit).collect();
    if flattened.chars().count() > limit {
        format!("{preview}...")
    } else if preview.is_empty() {
        String::from("(empty)")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::{branch_summary_line, build_session_tree};
    use yach_proto::SessionMessage;

    #[test]
    fn builds_branch_summaries_from_user_turns() {
        let tree = build_session_tree(&[
            message("user", "u1", "Start here"),
            message("assistant", "a1", "Answer"),
            message("user", "u2", "Try another branch"),
            message("assistant", "a2", "Another answer"),
        ]);

        assert_eq!(tree.nodes.len(), 4);
        assert_eq!(tree.branches.len(), 2);
        assert_eq!(tree.branches[0].message_count, 2);
        assert_eq!(tree.branches[1].root_entry_id.as_deref(), Some("u2"));
        assert_eq!(
            branch_summary_line(&tree),
            "session tree: 2 branches · 4 messages"
        );
    }

    #[test]
    fn harness_outcome_rows_do_not_alter_tree_nodes_counts_or_roots() {
        let with_harness = build_session_tree(&[
            message("harness", "turn-0", "provider_error kind=rate_limited"),
            message("user", "u1", "Start here"),
            message("assistant", "a1", "Answer"),
            message("harness", "turn-1", "cancelled by user"),
            message("user", "u2", "Try another branch"),
            message("assistant", "a2", "Another answer"),
        ]);
        let without_harness = build_session_tree(&[
            message("user", "u1", "Start here"),
            message("assistant", "a1", "Answer"),
            message("user", "u2", "Try another branch"),
            message("assistant", "a2", "Another answer"),
        ]);

        assert_eq!(with_harness.branches, without_harness.branches);
        assert_eq!(with_harness.nodes.len(), without_harness.nodes.len());
        assert!(with_harness.nodes.iter().all(|node| node.role != "harness"));
        assert_eq!(
            branch_summary_line(&with_harness),
            "session tree: 2 branches · 4 messages"
        );
    }

    fn message(role: &str, entry_id: &str, text: &str) -> SessionMessage {
        SessionMessage {
            role: role.to_owned(),
            text: text.to_owned(),
            entry_id: Some(entry_id.to_owned()),
            tool_name: None,
            is_error: None,
            outcome_kind: None,
            tool_result_metadata: None,
            tool_review: None,
        }
    }
}
