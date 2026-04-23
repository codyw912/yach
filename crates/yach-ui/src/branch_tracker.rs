use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub session_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub entry_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct BranchTracker {
    branches: HashMap<String, BranchInfo>,
    current_session: String,
}

impl BranchTracker {
    pub fn new(initial_session: &str) -> Self {
        let mut branches = HashMap::new();
        branches.insert(
            initial_session.to_string(),
            BranchInfo {
                session_id: initial_session.to_string(),
                parent_id: None,
                title: String::from("initial"),
                entry_count: 0,
            },
        );
        Self {
            branches,
            current_session: initial_session.to_string(),
        }
    }

    pub fn record_fork(&mut self, parent_id: &str, child_id: &str, entry_count: usize) {
        let parent_title = self
            .branches
            .get(parent_id)
            .map_or_else(|| String::from("unknown"), |p| p.title.clone());
        self.branches.insert(
            child_id.to_string(),
            BranchInfo {
                session_id: child_id.to_string(),
                parent_id: Some(parent_id.to_string()),
                title: format!("fork of {parent_title}"),
                entry_count,
            },
        );
    }

    pub fn set_current(&mut self, session_id: &str) {
        self.current_session = session_id.to_string();
        if !self.branches.contains_key(session_id) {
            self.branches.insert(
                session_id.to_string(),
                BranchInfo {
                    session_id: session_id.to_string(),
                    parent_id: None,
                    title: String::from("switched"),
                    entry_count: 0,
                },
            );
        }
    }

    pub fn update_entry_count(&mut self, session_id: &str, count: usize) {
        if let Some(branch) = self.branches.get_mut(session_id) {
            branch.entry_count = count;
        }
    }

    pub fn current(&self) -> &str {
        &self.current_session
    }

    pub fn branch_tree(&self) -> Vec<BranchInfo> {
        let mut result = Vec::new();
        self.collect_branch(&self.current_session, &mut result, 0);
        result
    }

    fn collect_branch(&self, session_id: &str, result: &mut Vec<BranchInfo>, depth: usize) {
        if let Some(branch) = self.branches.get(session_id) {
            let mut b = branch.clone();
            b.title = format!("{}{}", "  ".repeat(depth), b.title);
            result.push(b);
        }
        for branch in self.branches.values() {
            if branch.parent_id.as_deref() == Some(session_id) {
                self.collect_branch(&branch.session_id, result, depth + 1);
            }
        }
    }
}
