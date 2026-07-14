//! Sensitive-file deny-by-default policy for provider-visible file tools.
//!
//! Design: `docs/superpowers/specs/2026-07-14-sensitive-file-deny-design.md`.
//! One chokepoint decides whether a project-relative path may be read,
//! matched, listed, or edited by provider-visible tools. Deny patterns beat
//! allow patterns; built-in defaults apply unless config disables them;
//! invalid config patterns fail closed to the built-in defaults.

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

/// Built-in default deny patterns. Visible, documented, and overridable via
/// `.yach/config.json`; synthesized from the 2026-07-14 cohort research.
pub const DEFAULT_SENSITIVE_DENY_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.env",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ecdsa*",
    "id_ed25519*",
    "*.keystore",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "**/.aws/credentials",
    "**/.ssh/**",
    "**/.config/gcloud/**",
    "**/.azure/**",
    "secrets/**",
    "credentials.json",
];

/// Built-in default allow carve-outs evaluated after deny patterns.
pub const DEFAULT_SENSITIVE_ALLOW_PATTERNS: &[&str] = &[
    ".env.example",
    ".env.sample",
    ".env.template",
    "*.env.example",
];

/// `files` section of `.yach/config.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NativeFilesConfig {
    pub deny: Vec<String>,
    pub allow: Vec<String>,
    pub use_default_deny: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct NativeConfigFile {
    files: NativeFilesConfig,
}

/// Warning produced while loading sensitive-path config. Fail-closed: any
/// warning means the built-in defaults stayed in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeSensitivePathConfigWarning {
    InvalidConfig { path: String, reason: String },
    InvalidPattern { pattern: String },
}

/// Compiled sensitive-path policy.
#[derive(Debug, Clone)]
pub struct NativeSensitivePathPolicy {
    deny_patterns: Vec<String>,
    allow_patterns: Vec<String>,
    deny_set: GlobSet,
    allow_set: GlobSet,
}

impl PartialEq for NativeSensitivePathPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.deny_patterns == other.deny_patterns && self.allow_patterns == other.allow_patterns
    }
}

impl Eq for NativeSensitivePathPolicy {}

impl Default for NativeSensitivePathPolicy {
    fn default() -> Self {
        Self::from_patterns(
            DEFAULT_SENSITIVE_DENY_PATTERNS
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
            DEFAULT_SENSITIVE_ALLOW_PATTERNS
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
        )
        .unwrap_or_else(|_| Self::deny_nothing())
    }
}

impl NativeSensitivePathPolicy {
    /// Policy that denies nothing. Only for explicit opt-out and internal
    /// fallbacks; the default constructor applies the built-in deny list.
    #[must_use]
    pub fn deny_nothing() -> Self {
        Self {
            deny_patterns: Vec::new(),
            allow_patterns: Vec::new(),
            deny_set: GlobSet::empty(),
            allow_set: GlobSet::empty(),
        }
    }

    fn from_patterns(
        deny_patterns: Vec<String>,
        allow_patterns: Vec<String>,
    ) -> Result<Self, NativeSensitivePathConfigWarning> {
        let deny_set = compile_patterns(&deny_patterns)?;
        let allow_set = compile_patterns(&allow_patterns)?;
        Ok(Self {
            deny_patterns,
            allow_patterns,
            deny_set,
            allow_set,
        })
    }

    /// Resolve the effective policy from optional user- and project-scope
    /// config sections. Deny patterns from both scopes union with the
    /// built-in defaults (unless `use_default_deny` is `false`, project
    /// winning); allow patterns union with the built-in carve-outs. Invalid
    /// patterns fail closed: the built-in default policy is returned with a
    /// warning.
    #[must_use]
    pub fn resolve(
        user: Option<&NativeFilesConfig>,
        project: Option<&NativeFilesConfig>,
    ) -> (Self, Vec<NativeSensitivePathConfigWarning>) {
        let use_defaults = project
            .and_then(|config| config.use_default_deny)
            .or_else(|| user.and_then(|config| config.use_default_deny))
            .unwrap_or(true);

        let mut deny_patterns = Vec::new();
        let mut allow_patterns = Vec::new();
        if use_defaults {
            deny_patterns.extend(
                DEFAULT_SENSITIVE_DENY_PATTERNS
                    .iter()
                    .map(|pattern| (*pattern).to_owned()),
            );
            allow_patterns.extend(
                DEFAULT_SENSITIVE_ALLOW_PATTERNS
                    .iter()
                    .map(|pattern| (*pattern).to_owned()),
            );
        }
        for config in [user, project].into_iter().flatten() {
            deny_patterns.extend(config.deny.iter().cloned());
            allow_patterns.extend(config.allow.iter().cloned());
        }

        match Self::from_patterns(deny_patterns, allow_patterns) {
            Ok(policy) => (policy, Vec::new()),
            Err(warning) => (Self::default(), vec![warning]),
        }
    }

    /// Load and resolve config from the user scope (`~/.yach/config.json`)
    /// and the project scope (`<project>/.yach/config.json`).
    #[must_use]
    pub fn load_for_project(
        project_root: Option<&Path>,
    ) -> (Self, Vec<NativeSensitivePathConfigWarning>) {
        let mut warnings = Vec::new();
        let user = user_config_path().and_then(|path| load_files_config(&path, &mut warnings));
        let project = project_root
            .map(|root| root.join(".yach").join("config.json"))
            .and_then(|path| load_files_config(&path, &mut warnings));

        let (policy, mut resolve_warnings) = Self::resolve(user.as_ref(), project.as_ref());
        warnings.append(&mut resolve_warnings);
        (policy, warnings)
    }

    /// Whether provider-visible tools must refuse this project-relative
    /// path. Deny patterns beat allow patterns; allow patterns carve
    /// exceptions out of broader denies.
    #[must_use]
    pub fn denies(&self, relative_path: impl AsRef<Path>) -> bool {
        let relative_path = relative_path.as_ref();
        self.deny_set.is_match(relative_path) && !self.allow_set.is_match(relative_path)
    }
}

fn compile_patterns(patterns: &[String]) -> Result<GlobSet, NativeSensitivePathConfigWarning> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        // Basename patterns without a separator match at any depth,
        // mirroring gitignore semantics.
        let expanded = if pattern.contains('/') {
            pattern.clone()
        } else {
            format!("{{{pattern},**/{pattern}}}")
        };
        let glob =
            Glob::new(&expanded).map_err(|_| NativeSensitivePathConfigWarning::InvalidPattern {
                pattern: pattern.clone(),
            })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|_| NativeSensitivePathConfigWarning::InvalidPattern {
            pattern: String::from("<pattern set>"),
        })
}

fn user_config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        std::path::PathBuf::from(home)
            .join(".yach")
            .join("config.json"),
    )
}

fn load_files_config(
    path: &Path,
    warnings: &mut Vec<NativeSensitivePathConfigWarning>,
) -> Option<NativeFilesConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<NativeConfigFile>(&raw) {
        Ok(config) => Some(config.files),
        Err(error) => {
            warnings.push(NativeSensitivePathConfigWarning::InvalidConfig {
                path: path.to_string_lossy().into_owned(),
                reason: error.to_string(),
            });
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_denies_env_and_key_material_at_any_depth() {
        let policy = NativeSensitivePathPolicy::default();

        assert!(policy.denies(".env"));
        assert!(policy.denies(".env.local"));
        assert!(policy.denies("config/production.env"));
        assert!(policy.denies("deploy/cert.pem"));
        assert!(policy.denies("nested/dir/id_rsa"));
        assert!(policy.denies(".npmrc"));
        assert!(policy.denies("ops/.aws/credentials"));
        assert!(policy.denies("home/.ssh/known_hosts"));
        assert!(policy.denies("secrets/api.txt"));
        assert!(policy.denies("credentials.json"));
    }

    #[test]
    fn default_policy_allows_example_files_and_normal_source() {
        let policy = NativeSensitivePathPolicy::default();

        assert!(!policy.denies(".env.example"));
        assert!(!policy.denies(".env.sample"));
        assert!(!policy.denies(".env.template"));
        assert!(!policy.denies("src/environment.rs"));
        assert!(!policy.denies("src/lib.rs"));
        assert!(!policy.denies("docs/env-vars.md"));
        assert!(!policy.denies("keyboard.rs"));
    }

    #[test]
    fn no_substring_matching() {
        let policy = NativeSensitivePathPolicy::default();

        // opencode's `.includes(".env")` bug blocked files like these.
        assert!(!policy.denies("src/environment.ts"));
        assert!(!policy.denies("test/envelope.rs"));
        assert!(!policy.denies("monkey.rs"));
    }

    #[test]
    fn config_deny_and_allow_union_with_defaults() {
        let project = NativeFilesConfig {
            deny: vec![String::from("internal-secrets/**")],
            allow: vec![String::from(".env.ci")],
            use_default_deny: None,
        };

        let (policy, warnings) = NativeSensitivePathPolicy::resolve(None, Some(&project));

        assert!(warnings.is_empty());
        assert!(policy.denies("internal-secrets/token.txt"));
        assert!(!policy.denies(".env.ci"));
        assert!(policy.denies(".env.local"));
    }

    #[test]
    fn use_default_deny_false_disables_builtins_but_keeps_config_patterns() {
        let project = NativeFilesConfig {
            deny: vec![String::from("private/**")],
            allow: Vec::new(),
            use_default_deny: Some(false),
        };

        let (policy, warnings) = NativeSensitivePathPolicy::resolve(None, Some(&project));

        assert!(warnings.is_empty());
        assert!(!policy.denies(".env"));
        assert!(policy.denies("private/notes.txt"));
    }

    #[test]
    fn invalid_pattern_fails_closed_to_defaults() {
        let project = NativeFilesConfig {
            deny: vec![String::from("[invalid")],
            allow: vec![String::from(".env")],
            use_default_deny: None,
        };

        let (policy, warnings) = NativeSensitivePathPolicy::resolve(None, Some(&project));

        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            &warnings[0],
            NativeSensitivePathConfigWarning::InvalidPattern { pattern } if pattern == "[invalid"
        ));
        // Fail closed: defaults still deny, and the config's `.env` allow
        // did not take effect.
        assert!(policy.denies(".env"));
    }

    #[test]
    fn project_scope_wins_for_default_toggle() {
        let user = NativeFilesConfig {
            deny: Vec::new(),
            allow: Vec::new(),
            use_default_deny: Some(false),
        };
        let project = NativeFilesConfig {
            deny: Vec::new(),
            allow: Vec::new(),
            use_default_deny: Some(true),
        };

        let (policy, _) = NativeSensitivePathPolicy::resolve(Some(&user), Some(&project));

        assert!(policy.denies(".env"));
    }

    #[test]
    fn load_for_project_reads_project_config_and_flags_invalid_json() {
        let directory = std::env::temp_dir().join(format!(
            "yach-sensitive-config-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t").len()
        ));
        let yach_dir = directory.join(".yach");
        assert!(std::fs::create_dir_all(&yach_dir).is_ok());
        assert!(
            std::fs::write(
                yach_dir.join("config.json"),
                r#"{"files":{"allow":[".env.ci"]}}"#,
            )
            .is_ok()
        );

        let (policy, warnings) = NativeSensitivePathPolicy::load_for_project(Some(&directory));
        assert!(!policy.denies(".env.ci"));
        assert!(policy.denies(".env"));
        assert!(warnings.is_empty());

        assert!(std::fs::write(yach_dir.join("config.json"), "{not json").is_ok());
        let (policy, warnings) = NativeSensitivePathPolicy::load_for_project(Some(&directory));
        assert!(policy.denies(".env"));
        assert_eq!(warnings.len(), 1);

        assert!(std::fs::remove_dir_all(directory).is_ok());
    }
}
