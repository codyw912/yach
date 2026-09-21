use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

const SCHEMA: &str = "yach.review-policy.v1";

/// User-owned durable restriction. Global scope applies to every project;
/// project scope is keyed by the canonical project state key already used
/// for approval-mode persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewRestriction {
    /// Action matching `matcher` must be shown to the user before running.
    AskFirst {
        matcher: RestrictionMatcher,
        note: String,
    },
    /// Yach must hold and hand off; the human performs it outside the agent.
    HumanPerforms {
        matcher: RestrictionMatcher,
        note: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "match_on", rename_all = "snake_case")]
pub enum RestrictionMatcher {
    /// Exact argv-normalized command string prefix, e.g. "sudo", "nixos-rebuild".
    CommandPrefix { prefix: String },
    /// Canonical path prefix outside or inside the project.
    PathPrefix { prefix: String },
    /// Capability class, e.g. persistent install, host activation, publish.
    ActionClass { class: ActionClass },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    PersistentInstall,
    HostActivation,
    ExternalPublish,
    DestructiveDelete,
    SensitiveDisclosure,
}

/// Monotonic revision of the user's restriction set; bumped on every
/// persisted change and copied into every review request/decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct PolicyRevision(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPolicy {
    pub revision: PolicyRevision,
    pub global: Vec<ReviewRestriction>,
    pub project: Vec<ReviewRestriction>,
}

impl ReviewPolicy {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            revision: PolicyRevision(0),
            global: Vec::new(),
            project: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPolicyError {
    Io,
    Malformed,
    HomeUnavailable,
}

impl std::fmt::Display for ReviewPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Io => "review policy storage is unavailable",
            Self::Malformed => "review policy file is malformed",
            Self::HomeUnavailable => "user home directory is unavailable",
        })
    }
}

impl std::error::Error for ReviewPolicyError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredReviewPolicy {
    schema: String,
    revision: PolicyRevision,
    global: Vec<ReviewRestriction>,
    projects: BTreeMap<String, Vec<ReviewRestriction>>,
}

impl StoredReviewPolicy {
    fn empty() -> Self {
        Self {
            schema: String::from(SCHEMA),
            revision: PolicyRevision(0),
            global: Vec::new(),
            projects: BTreeMap::new(),
        }
    }

    fn view(&self, project_key: &str) -> ReviewPolicy {
        ReviewPolicy {
            revision: self.revision,
            global: self.global.clone(),
            project: self.projects.get(project_key).cloned().unwrap_or_default(),
        }
    }
}

pub struct ReviewPolicyStore {
    path: PathBuf,
    cached: Mutex<Option<StoredReviewPolicy>>,
}

impl ReviewPolicyStore {
    pub fn for_current_user() -> Result<Self, ReviewPolicyError> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or(ReviewPolicyError::HomeUnavailable)?;
        Ok(Self::in_home(Path::new(&home)))
    }

    #[must_use]
    pub fn in_home(home: &Path) -> Self {
        Self {
            path: home.join(".yach").join("review-policy.json"),
            cached: Mutex::new(None),
        }
    }

    /// Load global + project restrictions and the current revision.
    /// Missing file → empty policy at revision 0. Malformed → Err, caller
    /// keeps previous policy and warns.
    pub fn load(&self, project_key: &str) -> Result<ReviewPolicy, ReviewPolicyError> {
        let document = match read_document(&self.path)? {
            Some(document) => document,
            None => StoredReviewPolicy::empty(),
        };
        self.store_cache(document.clone());
        Ok(document.view(project_key))
    }

    /// Persist a new restriction set; bumps revision; fsync + rename;
    /// failure leaves previous policy active and returns Err.
    pub fn replace(
        &self,
        project_key: &str,
        policy: &ReviewPolicy,
    ) -> Result<PolicyRevision, ReviewPolicyError> {
        let mut document = self.base_document()?;
        let cached_revision = self.cached_revision();
        let base = document.revision.0.max(cached_revision);
        let next = base.checked_add(1).ok_or(ReviewPolicyError::Io)?;
        document.schema = String::from(SCHEMA);
        document.revision = PolicyRevision(next);
        document.global = policy.global.clone();
        if policy.project.is_empty() {
            document.projects.remove(project_key);
        } else {
            document
                .projects
                .insert(project_key.to_owned(), policy.project.clone());
        }
        write_atomic(&self.path, &document)?;
        self.store_cache(document);
        Ok(PolicyRevision(next))
    }

    fn base_document(&self) -> Result<StoredReviewPolicy, ReviewPolicyError> {
        match read_document(&self.path)? {
            Some(document) => Ok(document),
            None => Ok(self
                .cached_document()
                .unwrap_or_else(StoredReviewPolicy::empty)),
        }
    }

    fn cached_revision(&self) -> u64 {
        self.cached_document()
            .map_or(0, |document| document.revision.0)
    }

    fn cached_document(&self) -> Option<StoredReviewPolicy> {
        self.with_cache(|cached| cached.clone())
    }

    fn store_cache(&self, document: StoredReviewPolicy) {
        self.with_cache(|cached| {
            *cached = Some(document);
        });
    }

    fn with_cache<T>(&self, body: impl FnOnce(&mut Option<StoredReviewPolicy>) -> T) -> T {
        let mut guard = self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        body(&mut guard)
    }
}

fn read_document(path: &Path) -> Result<Option<StoredReviewPolicy>, ReviewPolicyError> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ReviewPolicyError::Io),
    };
    let document: StoredReviewPolicy =
        serde_json::from_str(&raw).map_err(|_| ReviewPolicyError::Malformed)?;
    if document.schema != SCHEMA {
        return Err(ReviewPolicyError::Malformed);
    }
    Ok(Some(document))
}

fn write_atomic(path: &Path, document: &StoredReviewPolicy) -> Result<(), ReviewPolicyError> {
    let Some(parent) = path.parent() else {
        return Err(ReviewPolicyError::Io);
    };
    create_private_dir(parent)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        serde_json::to_writer(&mut file, document).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result.map_err(|_| ReviewPolicyError::Io)
}

fn create_private_dir(path: &Path) -> Result<(), ReviewPolicyError> {
    if path.exists() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| ReviewPolicyError::Io)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        ActionClass, PolicyRevision, RestrictionMatcher, ReviewPolicyError, ReviewPolicyStore,
        ReviewRestriction,
    };

    struct TempHome {
        path: PathBuf,
    }

    impl TempHome {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "yach-review-policy-{name}-{}-{nanos}",
                std::process::id()
            ));
            let _ = fs::create_dir_all(&path);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let yach = self.path.join(".yach");
                let _ = fs::set_permissions(&yach, Permissions::from_mode(0o700));
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn host_activation() -> ReviewRestriction {
        ReviewRestriction::HumanPerforms {
            matcher: RestrictionMatcher::ActionClass {
                class: ActionClass::HostActivation,
            },
            note: String::from("I run rebuilds"),
        }
    }

    #[test]
    fn missing_policy_file_loads_empty_revision_zero() {
        let home = TempHome::new("missing");
        let store = ReviewPolicyStore::in_home(home.path());
        let policy = store.load("proj-key");
        assert!(policy.is_ok());
        let Ok(policy) = policy else {
            return;
        };
        assert_eq!(policy.revision, PolicyRevision(0));
        assert!(policy.global.is_empty() && policy.project.is_empty());
    }

    #[test]
    fn replace_persists_and_bumps_revision_across_reload() {
        let home = TempHome::new("replace");
        let store = ReviewPolicyStore::in_home(home.path());
        let loaded = store.load("k");
        assert!(loaded.is_ok());
        let Ok(mut policy) = loaded else {
            return;
        };
        policy.global.push(host_activation());
        let rev = store.replace("k", &policy);
        assert!(rev.is_ok());
        let Ok(rev) = rev else {
            return;
        };
        assert_eq!(rev, PolicyRevision(1));
        let reloaded = store.load("k");
        assert!(reloaded.is_ok());
        let Ok(reloaded) = reloaded else {
            return;
        };
        assert_eq!(reloaded.revision, PolicyRevision(1));
        assert!(matches!(
            reloaded.global.first(),
            Some(ReviewRestriction::HumanPerforms { .. })
        ));
    }

    #[test]
    fn replace_keeps_other_project_restrictions_under_one_revision() {
        let home = TempHome::new("projects");
        let store = ReviewPolicyStore::in_home(home.path());
        let first = store.load("alpha");
        assert!(first.is_ok());
        let Ok(mut first) = first else {
            return;
        };
        first.global.push(host_activation());
        first.project.push(ReviewRestriction::AskFirst {
            matcher: RestrictionMatcher::PathPrefix {
                prefix: String::from("secrets"),
            },
            note: String::from("ask before secrets"),
        });
        assert!(store.replace("alpha", &first).is_ok());

        let second = store.load("beta");
        assert!(second.is_ok());
        let Ok(mut second) = second else {
            return;
        };
        assert!(second.project.is_empty());
        assert!(matches!(
            second.global.first(),
            Some(ReviewRestriction::HumanPerforms { .. })
        ));
        second.project.push(ReviewRestriction::AskFirst {
            matcher: RestrictionMatcher::CommandPrefix {
                prefix: String::from("cargo publish"),
            },
            note: String::from("confirm publish"),
        });
        let rev = store.replace("beta", &second);
        assert!(rev.is_ok());
        assert_eq!(rev.ok(), Some(PolicyRevision(2)));

        let alpha = store.load("alpha");
        assert!(alpha.is_ok());
        let Ok(alpha) = alpha else {
            return;
        };
        assert_eq!(alpha.revision, PolicyRevision(2));
        assert!(matches!(
            alpha.project.first(),
            Some(ReviewRestriction::AskFirst {
                matcher: RestrictionMatcher::PathPrefix { prefix },
                ..
            }) if prefix == "secrets"
        ));
        let beta = store.load("beta");
        assert!(beta.is_ok());
        let Ok(beta) = beta else {
            return;
        };
        assert!(matches!(
            beta.project.first(),
            Some(ReviewRestriction::AskFirst {
                matcher: RestrictionMatcher::CommandPrefix { prefix },
                ..
            }) if prefix == "cargo publish"
        ));
    }

    #[test]
    fn malformed_file_returns_err_and_leaves_prior_policy_unchanged() {
        let home = TempHome::new("malformed");
        let store = ReviewPolicyStore::in_home(home.path());
        let loaded = store.load("k");
        assert!(loaded.is_ok());
        let Ok(mut policy) = loaded else {
            return;
        };
        policy.global.push(host_activation());
        assert!(store.replace("k", &policy).is_ok());
        let held = store.load("k");
        assert!(held.is_ok());
        let Ok(held) = held else {
            return;
        };
        assert_eq!(held.revision, PolicyRevision(1));

        let path = home.path().join(".yach").join("review-policy.json");
        let wrong_schema =
            b"{\"schema\":\"not-a-policy\",\"revision\":9,\"global\":[],\"projects\":{}}";
        assert!(fs::write(&path, wrong_schema).is_ok());
        assert!(matches!(store.load("k"), Err(ReviewPolicyError::Malformed)));
        assert_eq!(held.revision, PolicyRevision(1));
        assert!(matches!(
            held.global.first(),
            Some(ReviewRestriction::HumanPerforms { .. })
        ));
        let failed = store.replace("k", &held);
        assert!(failed.is_err());
        let still_corrupt = fs::read(&path);
        assert!(still_corrupt.is_ok());
        assert_eq!(still_corrupt.ok().as_deref(), Some(wrong_schema.as_slice()));
    }

    #[cfg(unix)]
    #[test]
    fn failed_replace_does_not_bump_revision_or_replace_file() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = TempHome::new("replace-fail");
        let store = ReviewPolicyStore::in_home(home.path());
        let loaded = store.load("k");
        assert!(loaded.is_ok());
        let Ok(mut policy) = loaded else {
            return;
        };
        policy.global.push(host_activation());
        assert_eq!(store.replace("k", &policy).ok(), Some(PolicyRevision(1)));
        let path = home.path().join(".yach").join("review-policy.json");
        let before = fs::read(&path);
        assert!(before.is_ok());
        let Ok(before) = before else {
            return;
        };
        let yach = home.path().join(".yach");
        assert!(fs::set_permissions(&yach, Permissions::from_mode(0o500)).is_ok());
        policy.project.push(ReviewRestriction::AskFirst {
            matcher: RestrictionMatcher::CommandPrefix {
                prefix: String::from("sudo"),
            },
            note: String::from("ask for sudo"),
        });
        assert!(store.replace("k", &policy).is_err());
        assert!(fs::set_permissions(&yach, Permissions::from_mode(0o700)).is_ok());
        assert_eq!(fs::read(&path).ok().as_deref(), Some(before.as_slice()));
        let reloaded = store.load("k");
        assert!(reloaded.is_ok());
        let Ok(reloaded) = reloaded else {
            return;
        };
        assert_eq!(reloaded.revision, PolicyRevision(1));
        assert!(reloaded.project.is_empty());
        let next = store.replace("k", &reloaded);
        assert_eq!(next.ok(), Some(PolicyRevision(2)));
    }

    #[cfg(unix)]
    #[test]
    fn persisted_policy_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = TempHome::new("modes");
        let store = ReviewPolicyStore::in_home(home.path());
        let loaded = store.load("k");
        assert!(loaded.is_ok());
        let Ok(policy) = loaded else {
            return;
        };
        assert!(store.replace("k", &policy).is_ok());
        let file = home.path().join(".yach").join("review-policy.json");
        let file_mode = fs::metadata(&file).map(|metadata| metadata.permissions().mode() & 0o777);
        let dir_mode = fs::metadata(file.parent().unwrap_or(home.path()))
            .map(|metadata| metadata.permissions().mode() & 0o777);
        assert_eq!(file_mode.ok(), Some(0o600));
        assert_eq!(dir_mode.ok(), Some(0o700));
    }
}
