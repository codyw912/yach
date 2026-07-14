//! Backend runner groundwork for yach.
//!
//! This crate owns backend-facing concepts that are not specific to the
//! temporary Pi RPC adapter or to the eventual native provider implementation.
//! The public Interface is re-exported here; focused Modules keep the
//! Implementation local to runner, resource, tool, session, and provider concerns.

mod agent_edit_tools;
mod edit;
mod edit_access;
#[cfg_attr(
    all(not(test), not(feature = "bench")),
    expect(dead_code, reason = "backend-local harness until tool integration")
)]
mod edit_harness;
#[cfg(feature = "bench")]
pub mod edit_profile;
mod extension;
mod extension_install;
mod native_runner;
mod permission;
mod provider;
mod resource;
mod runner;
mod session;
mod session_store;
mod static_context;
mod tools;

pub mod rig_adapter;
pub mod rig_diagnostics;

pub use agent_edit_tools::*;
pub use edit::*;
pub use edit_access::*;
pub use extension::*;
pub use extension_install::*;
pub use native_runner::*;
pub use permission::*;
pub use provider::*;
pub use resource::*;
pub use runner::*;
pub use session::*;
pub use session_store::*;
pub use static_context::*;
pub use tools::*;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use rig::streaming::{RawStreamingChoice, RawStreamingToolCall, ToolCallDeltaContent};

    use super::edit::{native_edit_error_label, sha256_hex_for_test};
    use super::edit_harness::{
        NativeEditHarness, NativeEditHarnessContext, native_edit_prepared_evidence_summary,
    };
    use super::{
        BackendCapabilities, BackendKind, BackendMetadata, BoundedProviderStreamBuffer,
        ExtensionHostInvoker, ExtensionHostProtocolError, ExtensionId, ExtensionToolCandidate,
        ExtensionToolContribution, ExtensionToolExecutorRouter, ExtensionToolHandler,
        ExtensionToolRisk, FixtureNativeToolExecutor, NativeAgentEditToolContext,
        NativeAgentEditToolPrepared, NativeEditAccess, NativeEditAccessContext, NativeEditEngine,
        NativeEditError, NativeEditEvidenceOutcome, NativeEditEvidenceSummary, NativeEditHunk,
        NativeEditOperation, NativeEditOperationEvidence, NativeEditPolicy, NativeEditTraceId,
        NativeEditTraceOutcome, NativeEditTracePhase, NativeEditTraceRecord, NativeEditTraceSource,
        NativeEditTransactionId, NativeEditTransactionRequest, NativeEntryId,
        NativeJsonlSessionStore, NativeMetricAttribute, NativePermissionActor,
        NativePermissionCapability, NativePermissionDecisionEngine,
        NativePermissionDecisionOutcome, NativePermissionMode, NativePermissionPolicy,
        NativePermissionRequest, NativePermissionRisk, NativePermissionTargetSummary,
        NativeProviderToolResult, NativeResourceContextError, NativeResourceContextPolicy,
        NativeResourceEntryKind, NativeResourceListPolicy, NativeResourcePathError,
        NativeResourceProviderVisibility, NativeResourceReadError, NativeResourceReadPolicy,
        NativeResourceRoot, NativeResourceRootKind, NativeResourceSearchPolicy, NativeRole,
        NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeStaticContextPolicy,
        NativeToolContinuationContext, NativeToolContinuationError, NativeToolContinuationPolicy,
        NativeToolContinuationWorkflow, NativeToolDefinition, NativeToolError,
        NativeToolExecutionError, NativeToolExecutionResult, NativeToolExecutor,
        NativeToolInputSchema, NativeToolOutcome, NativeToolOwner, NativeToolPayloadSummary,
        NativeToolPermissionPolicy, NativeToolPermissionState, NativeToolProvenance,
        NativeToolRegistrationError, NativeToolRegistry, NativeToolReplacementPolicy,
        NativeToolReplacementRule, NativeToolReplacementSource, NativeToolRequestId,
        NativeToolResolutionError, NativeToolResolutionMode, NativeToolRisk, NativeTurnId,
        NativeTurnOutcome, PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY, PendingAgentEditToolReview,
        PendingNativeToolRequest, ProjectReadOnlyToolExecutor, ProviderContinuationMappingError,
        ProviderContinuationRequest, ProviderContinuationValidationError,
        ProviderContinuationValidationPolicy, ProviderError, ProviderErrorKind, ProviderExtension,
        ProviderFinishReason, ProviderMessage, ProviderMetadata, ProviderModel, ProviderRequest,
        ProviderStreamEvent, ProviderToolAdvertising, ProviderToolAdvertisingError,
        ProviderToolCall, ProviderToolVisibility, ProviderUsage, announce_connected,
        assemble_project_static_context, backend_channels, build_fixture_provider_tool_results,
        build_project_path_info_provider_tool_advertising_extension,
        build_project_readonly_provider_tool_results, build_provider_continuation_submission,
        build_provider_tool_advertising_extension, completed_text_exchange,
        execute_agent_edit_tool_request, normalize_agent_edit_tool_request,
        parse_provider_tool_advertising_extensions, pending_tool_request_from_provider_call,
        prepare_agent_edit_tool_request, record_native_tool_validation,
        reject_agent_edit_tool_review, rig_adapter, start_backend_session,
        strip_provider_tool_advertising_extensions, validate_provider_continuation_request,
    };
    use yach_proto::{BackendEvent, Capability, ClientEvent, Handshake, NegotiatedCapabilities};

    #[derive(Clone)]
    struct RecordingExtensionHostInvoker {
        response: Result<String, ExtensionHostProtocolError>,
        calls: Arc<Mutex<Vec<RecordedExtensionHostInvocation>>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedExtensionHostInvocation {
        request_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        timeout: Duration,
    }

    impl RecordingExtensionHostInvoker {
        fn new(response: Result<String, ExtensionHostProtocolError>) -> Self {
            Self {
                response,
                calls: Arc::default(),
            }
        }

        fn calls(&self) -> Vec<RecordedExtensionHostInvocation> {
            self.calls
                .lock()
                .map_or_else(|_| Vec::new(), |calls| calls.clone())
        }
    }

    impl ExtensionHostInvoker for RecordingExtensionHostInvoker {
        fn invoke(
            &mut self,
            request_id: &str,
            tool_name: &str,
            arguments: serde_json::Value,
            timeout: Duration,
        ) -> Result<String, ExtensionHostProtocolError> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(RecordedExtensionHostInvocation {
                    request_id: request_id.to_owned(),
                    tool_name: tool_name.to_owned(),
                    arguments,
                    timeout,
                });
            }
            self.response.clone()
        }
    }

    #[test]
    fn native_project_resource_root_resolves_in_root_file() {
        let root_path = temp_resource_dir("native-resource-in-root");
        let nested = root_path.join("docs");
        assert!(std::fs::create_dir_all(&nested).is_ok());
        let file = nested.join("plan.md");
        assert!(std::fs::write(&file, "plan").is_ok());

        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let resolved = root
            .as_ref()
            .and_then(|root| root.resolve_file("docs/plan.md").ok());
        let canonical_file = file.canonicalize().ok();

        assert_eq!(
            root.as_ref().map(|root| root.kind),
            Some(NativeResourceRootKind::Project)
        );
        assert_eq!(resolved, canonical_file);
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_root_rejects_parent_traversal() {
        let base_path = temp_resource_dir("native-resource-traversal");
        let root_path = base_path.join("project");
        let outside_path = base_path.join("outside");
        assert!(std::fs::create_dir_all(&root_path).is_ok());
        assert!(std::fs::create_dir_all(&outside_path).is_ok());
        assert!(std::fs::write(outside_path.join("secret.txt"), "secret").is_ok());

        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let error = root
            .as_ref()
            .map(|root| root.resolve_file("../outside/secret.txt"));

        assert_eq!(error, Some(Err(NativeResourcePathError::EscapesRoot)));
        assert!(std::fs::remove_dir_all(base_path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn native_project_resource_root_rejects_symlink_to_outside() {
        let root_path = temp_resource_dir("native-resource-symlink-root");
        let outside_path = temp_resource_dir("native-resource-symlink-outside");
        let outside_file = outside_path.join("secret.txt");
        assert!(std::fs::write(&outside_file, "secret").is_ok());
        assert!(std::os::unix::fs::symlink(&outside_file, root_path.join("secret-link")).is_ok());

        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let error = root.as_ref().map(|root| root.resolve_file("secret-link"));

        assert_eq!(error, Some(Err(NativeResourcePathError::EscapesRoot)));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
        assert!(std::fs::remove_dir_all(outside_path).is_ok());
    }

    #[test]
    fn native_project_resource_root_reports_missing_paths() {
        let root_path = temp_resource_dir("native-resource-missing");
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root.as_ref().map(|root| root.resolve_file("missing.txt"));

        assert_eq!(error, Some(Err(NativeResourcePathError::Missing)));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_path_metadata_returns_normalized_file_and_directory_info() {
        let root_path = temp_resource_dir("native-resource-metadata");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        assert!(std::fs::write(root_path.join("src/lib.rs"), "pub fn demo() {}\n").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let file = root
            .as_ref()
            .and_then(|root| root.path_metadata("src/lib.rs").ok());
        let directory = root
            .as_ref()
            .and_then(|root| root.path_metadata("src").ok());

        assert_eq!(
            file.as_ref()
                .map(|metadata| metadata.relative_path.as_str()),
            Some("src/lib.rs")
        );
        assert_eq!(
            file.as_ref().map(|metadata| metadata.kind),
            Some(NativeResourceEntryKind::File)
        );
        assert_eq!(
            file.as_ref().and_then(|metadata| metadata.byte_size),
            Some(17)
        );
        assert_eq!(
            file.as_ref().map(|metadata| metadata.provider_visibility),
            Some(NativeResourceProviderVisibility::Never)
        );
        assert_eq!(
            directory
                .as_ref()
                .map(|metadata| metadata.relative_path.as_str()),
            Some("src")
        );
        assert_eq!(
            directory.as_ref().map(|metadata| metadata.kind),
            Some(NativeResourceEntryKind::Directory)
        );
        assert_eq!(
            directory.as_ref().and_then(|metadata| metadata.byte_size),
            None
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_path_metadata_reuses_root_escape_policy() {
        let base_path = temp_resource_dir("native-resource-metadata-escape");
        let root_path = base_path.join("project");
        let outside_path = base_path.join("outside");
        assert!(std::fs::create_dir_all(&root_path).is_ok());
        assert!(std::fs::create_dir_all(&outside_path).is_ok());
        assert!(std::fs::write(outside_path.join("secret.txt"), "secret").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root
            .as_ref()
            .map(|root| root.path_metadata("../outside/secret.txt"));

        assert_eq!(error, Some(Err(NativeResourcePathError::EscapesRoot)));
        assert!(std::fs::remove_dir_all(base_path).is_ok());
    }

    #[test]
    fn native_project_list_paths_returns_sorted_bounded_immediate_entries() {
        let root_path = temp_resource_dir("native-resource-list");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        assert!(std::fs::create_dir_all(root_path.join("src/.git")).is_ok());
        assert!(std::fs::write(root_path.join("src/lib.rs"), "lib").is_ok());
        assert!(std::fs::write(root_path.join("src/main.rs"), "main").is_ok());
        assert!(std::fs::write(root_path.join("src/README.md"), "readme").is_ok());
        assert!(std::fs::write(root_path.join("src/.git/generated.rs"), "skip").is_ok());
        let root = NativeResourceRoot::project(&root_path);
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };

        let result = root.list_paths("src", NativeResourceListPolicy { max_entries: 2 });

        assert!(result.is_ok());
        let Some(result) = result.ok() else {
            return;
        };
        assert_eq!(
            result.provider_visibility,
            NativeResourceProviderVisibility::Never
        );
        assert_eq!(result.relative_path, "src");
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].relative_path, "src/README.md");
        assert_eq!(result.entries[0].kind, NativeResourceEntryKind::File);
        assert_eq!(result.entries[0].byte_size, Some(6));
        assert_eq!(result.entries[1].relative_path, "src/lib.rs");
        assert_eq!(result.entries[1].kind, NativeResourceEntryKind::File);
        assert_eq!(result.entries[1].byte_size, Some(3));
        assert!(result.truncated);
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_list_paths_reuses_directory_and_root_escape_policy() {
        let base_path = temp_resource_dir("native-resource-list-policy");
        let root_path = base_path.join("project");
        let outside_path = base_path.join("outside");
        assert!(std::fs::create_dir_all(&root_path).is_ok());
        assert!(std::fs::create_dir_all(&outside_path).is_ok());
        assert!(std::fs::write(root_path.join("file.txt"), "file").is_ok());
        let root = NativeResourceRoot::project(&root_path);
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };

        let file_result = root.list_paths("file.txt", NativeResourceListPolicy { max_entries: 8 });
        let escape_result =
            root.list_paths("../outside", NativeResourceListPolicy { max_entries: 8 });

        assert_eq!(file_result, Err(NativeResourcePathError::ExpectedDirectory));
        assert_eq!(escape_result, Err(NativeResourcePathError::EscapesRoot));
        assert!(std::fs::remove_dir_all(base_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_returns_local_only_text_with_metadata() {
        let root_path = temp_resource_dir("native-resource-read");
        let file = root_path.join("note.txt");
        assert!(std::fs::write(&file, "hello").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let read = root.as_ref().and_then(|root| {
            root.read_text_file("note.txt", NativeResourceReadPolicy::local_only(16))
                .ok()
        });

        assert_eq!(read.as_ref().map(|read| read.text.as_str()), Some("hello"));
        assert_eq!(read.as_ref().map(|read| read.byte_count), Some(5));
        assert_eq!(
            read.as_ref().map(|read| read.provider_visibility),
            Some(NativeResourceProviderVisibility::Never)
        );
        assert_eq!(read.as_ref().map(|read| read.redacted), Some(false));
        assert_eq!(read.as_ref().map(|read| read.truncated), Some(false));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_enforces_size_limit() {
        let root_path = temp_resource_dir("native-resource-read-large");
        assert!(std::fs::write(root_path.join("large.txt"), "123456789").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root
            .as_ref()
            .map(|root| root.read_text_file("large.txt", NativeResourceReadPolicy::local_only(4)));

        assert_eq!(
            error,
            Some(Err(NativeResourceReadError::TooLarge {
                max_bytes: 4,
                actual_bytes: 9,
            }))
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_rejects_non_utf8() {
        let root_path = temp_resource_dir("native-resource-read-non-utf8");
        assert!(std::fs::write(root_path.join("binary.bin"), [0xff, 0xfe]).is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root.as_ref().map(|root| {
            root.read_text_file("binary.bin", NativeResourceReadPolicy::local_only(16))
        });

        assert_eq!(error, Some(Err(NativeResourceReadError::NotUtf8)));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_reuses_path_policy() {
        let base_path = temp_resource_dir("native-resource-read-policy");
        let root_path = base_path.join("project");
        let outside_path = base_path.join("outside");
        assert!(std::fs::create_dir_all(&root_path).is_ok());
        assert!(std::fs::create_dir_all(&outside_path).is_ok());
        assert!(std::fs::write(outside_path.join("secret.txt"), "secret").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root.as_ref().map(|root| {
            root.read_text_file(
                "../outside/secret.txt",
                NativeResourceReadPolicy::local_only(16),
            )
        });

        assert_eq!(
            error,
            Some(Err(NativeResourceReadError::Path(
                NativeResourcePathError::EscapesRoot
            )))
        );
        assert!(std::fs::remove_dir_all(base_path).is_ok());
    }

    #[test]
    fn native_project_context_package_reads_explicit_text_files_local_only() {
        let root_path = temp_resource_dir("native-resource-context");
        assert!(std::fs::create_dir_all(root_path.join("docs")).is_ok());
        assert!(std::fs::write(root_path.join("docs/one.md"), "one").is_ok());
        assert!(std::fs::write(root_path.join("docs/two.md"), "two").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let package = root.as_ref().and_then(|root| {
            root.read_context_package(
                ["docs/one.md", "docs/two.md"],
                NativeResourceContextPolicy {
                    max_file_bytes: 16,
                    max_files: 4,
                },
            )
            .ok()
        });

        assert_eq!(package.as_ref().map(|package| package.items.len()), Some(2));
        assert_eq!(
            package.as_ref().map(|package| package.provider_visibility),
            Some(NativeResourceProviderVisibility::Never)
        );
        assert_eq!(
            package
                .as_ref()
                .map(|package| package.items[0].relative_path.as_str()),
            Some("docs/one.md")
        );
        assert_eq!(
            package
                .as_ref()
                .map(|package| package.items[0].text.as_str()),
            Some("one")
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_context_package_enforces_file_count_limit() {
        let root_path = temp_resource_dir("native-resource-context-limit");
        assert!(std::fs::write(root_path.join("one.txt"), "one").is_ok());
        assert!(std::fs::write(root_path.join("two.txt"), "two").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let result = root.as_ref().map(|root| {
            root.read_context_package(
                ["one.txt", "two.txt"],
                NativeResourceContextPolicy {
                    max_file_bytes: 16,
                    max_files: 1,
                },
            )
        });

        assert_eq!(
            result,
            Some(Err(NativeResourceContextError::TooManyFiles {
                max_files: 1,
                actual_files: 2,
            }))
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_search_returns_bounded_local_only_matches() {
        let root_path = temp_resource_dir("native-resource-search");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        assert!(std::fs::write(root_path.join("src/lib.rs"), "alpha\nneedle one\n").is_ok());
        assert!(std::fs::write(root_path.join("src/main.rs"), "needle two\n").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let results = root.as_ref().and_then(|root| {
            root.search_text("needle", NativeResourceSearchPolicy::small())
                .ok()
        });

        assert_eq!(
            results.as_ref().map(|results| results.matches.len()),
            Some(2)
        );
        assert_eq!(
            results
                .as_ref()
                .map(|results| results.matches[0].relative_path.as_str()),
            Some("src/lib.rs")
        );
        assert_eq!(
            results
                .as_ref()
                .map(|results| results.matches[0].line_number),
            Some(2)
        );
        assert_eq!(
            results
                .as_ref()
                .map(|results| results.matches[0].line.as_str()),
            Some("needle one")
        );
        assert_eq!(
            results.as_ref().map(|results| results.provider_visibility),
            Some(NativeResourceProviderVisibility::Never)
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_search_skips_excluded_and_oversized_files() {
        let root_path = temp_resource_dir("native-resource-search-skip");
        assert!(std::fs::create_dir_all(root_path.join("target")).is_ok());
        assert!(std::fs::write(root_path.join("target/generated.txt"), "needle generated").is_ok());
        assert!(std::fs::write(root_path.join("big.txt"), "needle but too large").is_ok());
        assert!(std::fs::write(root_path.join("ok.txt"), "needle ok").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let results = root.as_ref().and_then(|root| {
            root.search_text(
                "needle",
                NativeResourceSearchPolicy {
                    max_file_bytes: 12,
                    max_files: 64,
                    max_matches: 8,
                },
            )
            .ok()
        });

        assert_eq!(
            results.as_ref().map(|results| results.matches.len()),
            Some(1)
        );
        assert_eq!(
            results
                .as_ref()
                .map(|results| results.matches[0].relative_path.as_str()),
            Some("ok.txt")
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_search_returns_matches_in_stable_path_order() {
        let root_path = temp_resource_dir("native-resource-search-order");
        assert!(std::fs::create_dir_all(root_path.join("b")).is_ok());
        assert!(std::fs::create_dir_all(root_path.join("a")).is_ok());
        assert!(std::fs::write(root_path.join("b/two.txt"), "needle two").is_ok());
        assert!(std::fs::write(root_path.join("a/one.txt"), "needle one").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let results = root.as_ref().and_then(|root| {
            root.search_text(
                "needle",
                NativeResourceSearchPolicy {
                    max_file_bytes: 16,
                    max_files: 16,
                    max_matches: 1,
                },
            )
            .ok()
        });

        assert_eq!(
            results
                .as_ref()
                .map(|results| results.matches[0].relative_path.as_str()),
            Some("a/one.txt")
        );
        assert_eq!(
            results
                .as_ref()
                .map(|results| results.matches[0].line.as_str()),
            Some("needle one")
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_search_counts_non_utf8_files_toward_file_bound() {
        let root_path = temp_resource_dir("native-resource-search-non-utf8-bound");
        assert!(std::fs::write(root_path.join("a.bin"), [0xff, 0xfe]).is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let results = root.as_ref().and_then(|root| {
            root.search_text(
                "needle",
                NativeResourceSearchPolicy {
                    max_file_bytes: 16,
                    max_files: 1,
                    max_matches: 8,
                },
            )
            .ok()
        });

        assert_eq!(
            results.as_ref().map(|results| results.matches.len()),
            Some(0)
        );
        assert_eq!(
            results.as_ref().map(|results| results.searched_files),
            Some(1)
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_root_distinguishes_files_and_directories() {
        let root_path = temp_resource_dir("native-resource-kind");
        let directory = root_path.join("directory");
        assert!(std::fs::create_dir_all(&directory).is_ok());
        let file = root_path.join("file.txt");
        assert!(std::fs::write(&file, "file").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let canonical_directory = directory.canonicalize().ok();

        assert_eq!(
            root.as_ref().map(|root| root.resolve_file("directory")),
            Some(Err(NativeResourcePathError::ExpectedFile))
        );
        assert_eq!(
            root.as_ref().map(|root| root.resolve_directory("file.txt")),
            Some(Err(NativeResourcePathError::ExpectedDirectory))
        );
        assert_eq!(
            root.as_ref()
                .and_then(|root| root.resolve_directory("directory").ok()),
            canonical_directory
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn provider_continuation_request_validates_and_preserves_metadata() {
        let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            Some("provider-call-1"),
            "redacted result",
        )]);

        let result = validate_provider_continuation_request(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(64),
        );

        assert_eq!(result, Ok(()));
        assert_eq!(request.turn_id, NativeTurnId(String::from("turn-1")));
        assert_eq!(request.model.provider, "fixture-provider");
        assert_eq!(
            request.tool_results[0].provider_call_id,
            Some(String::from("provider-call-1"))
        );
    }

    #[test]
    fn build_provider_continuation_submission_preserves_tool_result_metadata() {
        let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            Some("provider-call-1"),
            "{\"relative_path\":\"Cargo.toml\",\"kind\":\"file\",\"byte_size\":10,\"provider_visibility\":\"never\"}",
        )]);

        let submission = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(256),
        );

        assert!(submission.as_ref().is_ok_and(|submission| {
            submission.turn_id == NativeTurnId(String::from("turn-1"))
                && submission.model.provider == "fixture-provider"
                && submission.prior_messages.len() == 1
                && submission.extensions.len() == 1
                && submission.tool_results.len() == 1
        }));
        let result = submission
            .ok()
            .and_then(|submission| submission.tool_results.into_iter().next());
        assert_eq!(
            result
                .as_ref()
                .map(|result| result.tool_request_id.as_str()),
            Some("tool-request-1")
        );
        assert_eq!(
            result
                .as_ref()
                .map(|result| result.provider_call_id.as_str()),
            Some("provider-call-1")
        );
        assert_eq!(
            result.as_ref().map(|result| result.status),
            Some(NativeToolOutcome::Completed)
        );
        assert!(
            result
                .as_ref()
                .is_some_and(|result| result.content.contains("\"provider_visibility\":\"never\""))
        );
    }

    #[test]
    fn provider_continuation_accepts_agent_edit_rejection_as_completed_transport_result() {
        let content = serde_json::json!({
            "outcome": "rejected",
            "tool_request_id": "tool-request-1",
            "path": "notes.txt"
        })
        .to_string();
        let request = ProviderContinuationRequest {
            turn_id: NativeTurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            },
            prior_messages: Vec::new(),
            tool_results: vec![NativeProviderToolResult {
                tool_request_id: String::from("tool-request-1"),
                provider_call_id: Some(String::from("call-edit-1")),
                status: NativeToolOutcome::Completed,
                byte_count: content.len(),
                content,
                redacted: true,
                truncated: false,
                reason: Some(String::from("user_rejected")),
            }],
            extensions: Vec::new(),
        };

        let submission = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(512),
        );

        assert!(submission.is_ok());
        let Some(result) = submission
            .ok()
            .and_then(|submission| submission.tool_results.into_iter().next())
        else {
            return;
        };
        assert_eq!(result.status, NativeToolOutcome::Completed);
        assert!(result.content.contains("\"outcome\":\"rejected\""));
    }

    #[test]
    fn build_provider_continuation_submission_rejects_empty_results() {
        let request = fixture_provider_continuation_request(Vec::new());

        let result = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(256),
        );

        assert_eq!(
            result,
            Err(ProviderContinuationMappingError::EmptyToolResults)
        );
    }

    #[test]
    fn build_provider_continuation_submission_rejects_non_completed_results() {
        let mut failed_result =
            fixture_provider_tool_result("tool-request-1", Some("provider-call-1"), "tool failed");
        failed_result.status = NativeToolOutcome::Failed;
        failed_result.reason = Some(String::from("resource_path_missing"));
        let request = fixture_provider_continuation_request(vec![failed_result]);

        let result = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(256),
        );

        assert_eq!(
            result,
            Err(
                ProviderContinuationMappingError::UnsupportedToolResultStatus {
                    tool_request_id: String::from("tool-request-1"),
                    status: NativeToolOutcome::Failed,
                }
            )
        );
    }

    #[test]
    fn build_provider_continuation_submission_allows_failed_results_for_agent_policy() {
        let mut failed_result =
            fixture_provider_tool_result("tool-request-1", Some("provider-call-1"), "tool failed");
        failed_result.status = NativeToolOutcome::Failed;
        failed_result.reason = Some(String::from("target_exists"));
        let request = fixture_provider_continuation_request(vec![failed_result]);

        let result = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::agent_tool_results(256),
        );

        assert!(result.is_ok());
        let Ok(submission) = result else {
            return;
        };
        assert_eq!(submission.tool_results.len(), 1);
        assert_eq!(submission.tool_results[0].status, NativeToolOutcome::Failed);
        assert_eq!(
            submission.tool_results[0].reason.as_deref(),
            Some("target_exists")
        );
    }

    #[test]
    fn build_provider_continuation_submission_rejects_denied_results_for_agent_policy() {
        let mut denied_result =
            fixture_provider_tool_result("tool-request-1", Some("provider-call-1"), "tool denied");
        denied_result.status = NativeToolOutcome::Denied;
        let request = fixture_provider_continuation_request(vec![denied_result]);

        let result = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::agent_tool_results(256),
        );

        assert_eq!(
            result,
            Err(
                ProviderContinuationMappingError::UnsupportedToolResultStatus {
                    tool_request_id: String::from("tool-request-1"),
                    status: NativeToolOutcome::Denied,
                }
            )
        );
    }

    #[test]
    fn build_provider_continuation_submission_wraps_validation_errors() {
        let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            None,
            "redacted result",
        )]);

        let result = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(256),
        );

        assert_eq!(
            result,
            Err(ProviderContinuationMappingError::Validation(
                ProviderContinuationValidationError::MissingProviderCallId {
                    tool_request_id: String::from("tool-request-1"),
                },
            ))
        );
    }

    #[test]
    fn provider_tool_advertising_builder_emits_project_path_info_schema() {
        let extension =
            build_provider_tool_advertising_extension(&[NativeToolDefinition::project_path_info()]);

        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        assert_eq!(extension.key, PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY);
        let advertising = parse_provider_tool_advertising_extensions(&[extension]);
        assert!(advertising.is_ok());
        let Ok(Some(advertising)) = advertising else {
            return;
        };
        assert_eq!(advertising.tools.len(), 1);
        let tool = &advertising.tools[0];
        assert_eq!(tool.name, "project_path_info");
        assert_eq!(
            tool.description,
            "Return local-only project path metadata without reading file contents."
        );
        assert_eq!(
            tool.parameters
                .get("type")
                .and_then(serde_json::Value::as_str),
            Some("object")
        );
        let properties = tool
            .parameters
            .get("properties")
            .and_then(serde_json::Value::as_object);
        assert!(properties.is_some());
        let Some(properties) = properties else {
            return;
        };
        assert_eq!(properties.len(), 1);
        let path = properties.get("path");
        assert_eq!(
            path.and_then(|path| path.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("string")
        );
        assert_eq!(
            path.and_then(|path| path.get("description"))
                .and_then(serde_json::Value::as_str),
            Some("Project-relative path to inspect.")
        );
        assert_eq!(
            tool.parameters.get("required"),
            Some(&serde_json::json!(["path"]))
        );
        assert_eq!(
            tool.parameters.get("additionalProperties"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn provider_tool_advertising_builder_emits_approved_extension_schema() {
        let tool = NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Visible,
        );

        let extension = build_provider_tool_advertising_extension(&[tool]);
        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        let advertising = parse_provider_tool_advertising_extensions(&[extension]);
        assert!(advertising.is_ok());
        let Ok(Some(advertising)) = advertising else {
            return;
        };

        assert_eq!(advertising.tools[0].name, "toy_tool");
        assert_eq!(
            advertising.tools[0].parameters["required"],
            serde_json::json!(["label"])
        );
        assert_eq!(
            advertising.tools[0].parameters["properties"]["label"]["type"],
            "string"
        );
    }

    #[test]
    fn provider_tool_advertising_builder_emits_canonical_agent_edit_schemas() {
        let extension = build_provider_tool_advertising_extension(&[
            NativeToolDefinition::edit_text_file(),
            NativeToolDefinition::create_text_file(),
        ]);
        assert!(extension.is_ok());
        let Ok(extension) = extension else {
            return;
        };
        let advertising = parse_provider_tool_advertising_extensions(&[extension]);
        assert!(advertising.is_ok());
        let Ok(Some(advertising)) = advertising else {
            return;
        };

        let names = advertising
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["edit_text_file", "create_text_file"]);

        let edit = &advertising.tools[0];
        assert_eq!(
            edit.parameters["required"],
            serde_json::json!(["find", "path", "replace"])
        );
        assert!(
            edit.parameters["properties"]
                .get("expected_sha256")
                .is_none()
        );
    }

    #[test]
    fn provider_tool_advertising_builder_emits_canonical_content_schemas() {
        let extension = build_provider_tool_advertising_extension(&[
            NativeToolDefinition::read_text_file(),
            NativeToolDefinition::search_project(),
            NativeToolDefinition::list_project_paths(),
        ]);

        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        let advertising = serde_json::from_value::<ProviderToolAdvertising>(extension.value);
        assert!(advertising.is_ok());
        let Some(advertising) = advertising.ok() else {
            return;
        };
        let names = advertising
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec!["read_text_file", "search_project", "list_project_paths"]
        );
        for tool in &advertising.tools {
            assert_eq!(tool.parameters["type"], "object");
            assert_eq!(tool.parameters["additionalProperties"], false);
        }
        assert!(advertising.tools.iter().any(|tool| {
            tool.name == "read_text_file"
                && tool.parameters["properties"]["path"]["type"] == "string"
                && tool.parameters["required"] == serde_json::json!(["path"])
        }));
        assert!(advertising.tools.iter().any(|tool| {
            tool.name == "search_project"
                && tool.parameters["properties"]["query"]["type"] == "string"
                && tool.parameters["required"] == serde_json::json!(["query"])
        }));
        assert!(advertising.tools.iter().any(|tool| {
            tool.name == "list_project_paths"
                && tool.parameters["properties"]["path"]["type"] == "string"
                && tool.parameters["required"] == serde_json::json!(["path"])
        }));
    }

    #[test]
    fn provider_tool_advertising_rejects_mutated_builtin_project_path_info() {
        let mut mutated_schema = NativeToolDefinition::project_path_info();
        mutated_schema.input_schema =
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512);
        assert_eq!(
            build_provider_tool_advertising_extension(&[mutated_schema]),
            Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from("project_path_info")
            })
        );

        let mut mutated_description = NativeToolDefinition::project_path_info();
        mutated_description.description = String::from("Different description.");
        assert_eq!(
            build_provider_tool_advertising_extension(&[mutated_description]),
            Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from("project_path_info")
            })
        );
    }

    #[test]
    fn provider_tool_advertising_rejects_mutated_builtin_content_tool() {
        let mut tool = NativeToolDefinition::read_text_file();
        tool.description = String::from("changed");

        assert_eq!(
            build_provider_tool_advertising_extension(&[tool]),
            Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from("read_text_file")
            })
        );
    }

    #[test]
    fn provider_tool_advertising_rejects_noncanonical_mutation_tool() {
        let mut tool = NativeToolDefinition::edit_text_file();
        tool.name = String::from("write_text_file");

        assert_eq!(
            build_provider_tool_advertising_extension(&[tool]).err(),
            Some(ProviderToolAdvertisingError::UnsupportedTool {
                name: String::from("write_text_file")
            })
        );
    }

    #[test]
    fn provider_tool_advertising_rejects_unsupported_tools_and_risks() {
        let fixture = NativeToolDefinition::fixture_echo_metadata();
        let unsupported_tool = build_provider_tool_advertising_extension(&[fixture]);

        assert_eq!(
            unsupported_tool,
            Err(ProviderToolAdvertisingError::UnsupportedTool {
                name: String::from("fixture_echo_metadata")
            })
        );

        let mut content_risk = NativeToolDefinition::project_path_info();
        content_risk.risk = NativeToolRisk::ReadsLocalContent;
        assert_eq!(
            build_provider_tool_advertising_extension(&[content_risk]),
            Err(ProviderToolAdvertisingError::UnsupportedRisk {
                name: String::from("project_path_info"),
                risk: NativeToolRisk::ReadsLocalContent,
            })
        );

        let mut hidden = NativeToolDefinition::project_path_info();
        hidden.provider_visibility = ProviderToolVisibility::Hidden;
        assert_eq!(
            build_provider_tool_advertising_extension(&[hidden]),
            Err(ProviderToolAdvertisingError::UnsupportedTool {
                name: String::from("project_path_info")
            })
        );
    }

    #[test]
    fn provider_tool_advertising_parser_fails_closed_for_malformed_known_data() {
        let malformed = ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({"tools": true}),
        };
        assert_eq!(
            parse_provider_tool_advertising_extensions(&[malformed]),
            Err(ProviderToolAdvertisingError::Malformed)
        );

        let empty = ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({"tools": []}),
        };
        assert_eq!(
            parse_provider_tool_advertising_extensions(&[empty]),
            Err(ProviderToolAdvertisingError::EmptyTools)
        );

        let canonical = build_project_path_info_provider_tool_advertising_extension();
        assert!(canonical.is_ok());
        let Some(canonical) = canonical.ok() else {
            return;
        };
        let canonical_tool = canonical.value["tools"][0].clone();
        let duplicate_names = ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({"tools": [canonical_tool.clone(), canonical_tool]}),
        };
        assert_eq!(
            parse_provider_tool_advertising_extensions(&[duplicate_names]),
            Err(ProviderToolAdvertisingError::DuplicateToolName {
                name: String::from("project_path_info")
            })
        );

        assert_eq!(
            parse_provider_tool_advertising_extensions(&[canonical.clone(), canonical.clone()]),
            Err(ProviderToolAdvertisingError::DuplicateExtension)
        );

        let missing_required_property = ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({
                "tools": [{
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "label argument for toy_tool."
                            }
                        },
                        "required": ["missing"],
                        "additionalProperties": false
                    }
                }]
            }),
        };
        assert_eq!(
            parse_provider_tool_advertising_extensions(&[missing_required_property]),
            Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from("toy_tool")
            })
        );

        let extra_root_key = ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({
                "tools": [{
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "label argument for toy_tool."
                            }
                        },
                        "required": ["label"],
                        "additionalProperties": false,
                        "extra": true
                    }
                }]
            }),
        };
        assert_eq!(
            parse_provider_tool_advertising_extensions(&[extra_root_key]),
            Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from("toy_tool")
            })
        );

        let duplicate_required = ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({
                "tools": [{
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "label argument for toy_tool."
                            }
                        },
                        "required": ["label", "label"],
                        "additionalProperties": false
                    }
                }]
            }),
        };
        assert_eq!(
            parse_provider_tool_advertising_extensions(&[duplicate_required]),
            Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from("toy_tool")
            })
        );

        let unsupported_schema = ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({
                "tools": [{
                    "name": "project_path_info",
                    "description": "Return local-only project path metadata without reading file contents.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Project-relative path to inspect."
                            }
                        },
                        "required": ["path"],
                        "additionalProperties": true
                    }
                }]
            }),
        };
        assert_eq!(
            parse_provider_tool_advertising_extensions(&[unsupported_schema]),
            Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from("project_path_info")
            })
        );
    }

    #[test]
    fn provider_tool_advertising_parser_ignores_unrelated_extensions() {
        let unrelated = ProviderExtension {
            key: String::from("fixture"),
            value: serde_json::json!(true),
        };

        assert_eq!(
            parse_provider_tool_advertising_extensions(&[unrelated]),
            Ok(None)
        );
    }

    #[test]
    fn provider_tool_advertising_strip_removes_only_known_extension() {
        let before = ProviderExtension {
            key: String::from("before"),
            value: serde_json::json!({"keep": 1}),
        };
        let advertising = build_project_path_info_provider_tool_advertising_extension();
        assert!(advertising.is_ok());
        let Some(advertising) = advertising.ok() else {
            return;
        };
        let after = ProviderExtension {
            key: String::from("after"),
            value: serde_json::json!({"keep": 2}),
        };

        let stripped = strip_provider_tool_advertising_extensions(vec![
            before.clone(),
            advertising,
            after.clone(),
        ]);

        assert_eq!(stripped, vec![before, after]);
    }

    #[test]
    fn rig_continuation_projection_appends_ordered_tool_messages() {
        let request = fixture_provider_continuation_request(vec![
            fixture_provider_tool_result(
                "tool-request-1",
                Some("provider-call-1"),
                "{\"one\":true}",
            ),
            fixture_provider_tool_result(
                "tool-request-2",
                Some("provider-call-2"),
                "{\"two\":true}",
            ),
        ]);
        let submission = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(256),
        )
        .ok();
        assert!(submission.is_some());

        let Some(submission) = submission else {
            return;
        };
        let projected = rig_adapter::project_provider_continuation_request(submission);

        assert_eq!(projected.turn_id, NativeTurnId(String::from("turn-1")));
        assert_eq!(projected.model.provider, "fixture-provider");
        assert_eq!(projected.extensions.len(), 1);
        assert_eq!(projected.messages.len(), 4);
        assert_eq!(projected.messages[0].role, NativeRole::User);
        assert_eq!(projected.messages[1].role, NativeRole::System);
        assert!(
            projected.messages[1]
                .content
                .contains("You may call more advertised tools")
        );
        assert!(
            !projected.messages[1]
                .content
                .contains("No additional tools are available")
        );
        assert_eq!(projected.messages[2].role, NativeRole::Tool);
        assert_eq!(projected.messages[3].role, NativeRole::Tool);

        let first_tool =
            serde_json::from_str::<serde_json::Value>(&projected.messages[2].content).ok();
        let second_tool =
            serde_json::from_str::<serde_json::Value>(&projected.messages[3].content).ok();
        assert_eq!(
            first_tool
                .as_ref()
                .and_then(|tool| tool.get("provider_call_id"))
                .and_then(serde_json::Value::as_str),
            Some("provider-call-1")
        );
        assert_eq!(
            first_tool
                .as_ref()
                .and_then(|tool| tool.get("status"))
                .and_then(serde_json::Value::as_str),
            Some("completed")
        );
        assert_eq!(
            first_tool
                .as_ref()
                .and_then(|tool| tool.get("content"))
                .and_then(serde_json::Value::as_str),
            Some("{\"one\":true}")
        );
        assert_eq!(
            second_tool
                .as_ref()
                .and_then(|tool| tool.get("provider_call_id"))
                .and_then(serde_json::Value::as_str),
            Some("provider-call-2")
        );
        assert_eq!(
            second_tool
                .as_ref()
                .and_then(|tool| tool.get("status"))
                .and_then(serde_json::Value::as_str),
            Some("completed")
        );
        assert_eq!(
            second_tool
                .as_ref()
                .and_then(|tool| tool.get("content"))
                .and_then(serde_json::Value::as_str),
            Some("{\"two\":true}")
        );
    }

    #[test]
    fn rig_continuation_guard_allows_more_advertised_tools() {
        let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            Some("provider-call-1"),
            "{\"one\":true}",
        )]);
        let submission = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(256),
        )
        .ok();
        assert!(submission.is_some());

        let Some(submission) = submission else {
            return;
        };
        let projected = rig_adapter::project_provider_continuation_request(submission);
        let guard = projected
            .messages
            .iter()
            .find(|message| message.role == NativeRole::System);

        assert!(guard.is_some());
        let Some(guard) = guard else {
            return;
        };
        assert!(guard.content.contains("You may call more advertised tools"));
        assert!(!guard.content.contains("No additional tools are available"));
    }

    #[test]
    fn rig_continuation_projection_excludes_raw_arguments() {
        let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            Some("provider-call-1"),
            "{\"relative_path\":\"Cargo.toml\",\"provider_visibility\":\"never\"}",
        )]);
        let submission = build_provider_continuation_submission(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(256),
        )
        .ok();
        assert!(submission.is_some());

        let Some(submission) = submission else {
            return;
        };
        let projected = rig_adapter::project_provider_continuation_request(submission);
        let tool_message = projected
            .messages
            .iter()
            .find(|message| message.role == NativeRole::Tool);

        assert!(tool_message.is_some());
        let Some(tool_message) = tool_message else {
            return;
        };
        let tool_json = serde_json::from_str::<serde_json::Value>(&tool_message.content).ok();
        assert_eq!(
            tool_json
                .as_ref()
                .and_then(|tool| tool.get("content"))
                .and_then(serde_json::Value::as_str),
            Some("{\"relative_path\":\"Cargo.toml\",\"provider_visibility\":\"never\"}")
        );
        assert!(
            tool_json
                .as_ref()
                .is_some_and(|tool| tool.get("arguments_json").is_none())
        );
        assert!(
            tool_json
                .as_ref()
                .is_some_and(|tool| tool.get("path").is_none())
        );
        assert!(
            tool_json
                .as_ref()
                .is_some_and(|tool| tool.get("tool_request_id").is_none())
        );
    }

    #[test]
    fn provider_continuation_request_rejects_missing_provider_call_id() {
        let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            None,
            "redacted result",
        )]);

        let result = validate_provider_continuation_request(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(64),
        );

        assert_eq!(
            result,
            Err(ProviderContinuationValidationError::MissingProviderCallId {
                tool_request_id: String::from("tool-request-1"),
            })
        );
    }

    #[test]
    fn provider_continuation_request_rejects_oversized_content() {
        let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            Some("provider-call-1"),
            "0123456789",
        )]);

        let result = validate_provider_continuation_request(
            &request,
            ProviderContinuationValidationPolicy::strict_tool_results(4),
        );

        assert_eq!(
            result,
            Err(ProviderContinuationValidationError::ResultContentTooLarge {
                tool_request_id: String::from("tool-request-1"),
                max_bytes: 4,
                actual_bytes: 10,
            })
        );
    }

    #[test]
    fn provider_continuation_request_respects_redaction_and_truncation_policy() {
        let redacted = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
            "tool-request-1",
            Some("provider-call-1"),
            "redacted result",
        )]);
        let mut truncated_result = fixture_provider_tool_result(
            "tool-request-2",
            Some("provider-call-2"),
            "truncated result",
        );
        truncated_result.truncated = true;
        let truncated = fixture_provider_continuation_request(vec![truncated_result]);

        assert_eq!(
            validate_provider_continuation_request(
                &redacted,
                ProviderContinuationValidationPolicy {
                    require_provider_call_id: true,
                    max_result_content_bytes: 64,
                    allow_redacted_results: false,
                    allow_truncated_results: false,
                    allow_failed_results: false,
                },
            ),
            Err(
                ProviderContinuationValidationError::RedactedResultRejected {
                    tool_request_id: String::from("tool-request-1"),
                }
            )
        );
        assert_eq!(
            validate_provider_continuation_request(
                &truncated,
                ProviderContinuationValidationPolicy::strict_tool_results(64),
            ),
            Err(
                ProviderContinuationValidationError::TruncatedResultRejected {
                    tool_request_id: String::from("tool-request-2"),
                }
            )
        );
    }

    #[test]
    fn fixture_provider_tool_results_execute_and_record_success() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"secret-label"}),
        }];

        let results = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            results,
            Ok(vec![NativeProviderToolResult {
                tool_request_id: String::from("tool-request-1"),
                provider_call_id: Some(String::from("provider-call-1")),
                status: NativeToolOutcome::Completed,
                content: String::from("fixture tool executed with redacted arguments"),
                byte_count: 24,
                redacted: true,
                truncated: false,
                reason: None,
            }])
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                result_summary: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn project_readonly_provider_tool_results_execute_metadata_and_record_success() {
        let root_path = temp_resource_dir("native-readonly-tool-loop-success");
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("project_path_info"),
            arguments_json: serde_json::json!({"path":"Cargo.toml"}),
        }];
        let mut log = NativeSessionLog::default();

        let Some(root) = root else {
            return;
        };
        let results = build_project_readonly_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            root,
            &NativeToolRegistry::with_project_read_only_tools(),
            &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert!(results.as_ref().is_ok_and(|results| results.len() == 1));
        let result = results.ok().and_then(|mut results| results.pop());
        assert_eq!(
            result
                .as_ref()
                .and_then(|result| result.provider_call_id.as_deref()),
            Some("provider-call-1")
        );
        assert_eq!(
            result.as_ref().map(|result| result.status),
            Some(NativeToolOutcome::Completed)
        );
        assert!(
            result
                .as_ref()
                .is_some_and(|result| result.content.contains("\"relative_path\":\"Cargo.toml\""))
        );
        assert!(
            result
                .as_ref()
                .is_some_and(|result| result.content.contains("\"provider_visibility\":\"never\""))
        );
        assert!(
            result
                .as_ref()
                .is_some_and(|result| !result.content.contains("[package]"))
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.first(),
            Some(NativeSessionEvent::ToolRequestRecorded {
                tool_name,
                permission: NativeToolPermissionState::Allowed,
                ..
            }) if tool_name == "project_path_info"
        ));
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                result_summary: Some(summary),
                ..
            }) if summary.summary.contains("\"relative_path\":\"Cargo.toml\"")
                && !summary.summary.contains("[package]")
        ));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_read_text_file_returns_content_with_persisted_evidence()
     {
        let root_path = temp_resource_dir("provider-read-text-file");
        assert!(std::fs::write(root_path.join("notes.txt"), "alpha\nbeta\n").is_ok());
        let root = NativeResourceRoot::project(&root_path);
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info"],
                ["read_text_file"],
                std::iter::empty::<&str>(),
            );
        let mut log = NativeSessionLog::default();
        let context = NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
        };

        let results = build_project_readonly_provider_tool_results(
            &mut log,
            &context,
            vec![ProviderToolCall {
                call_id: String::from("call-read-1"),
                name: String::from("read_text_file"),
                arguments_json: serde_json::json!({"path": "notes.txt"}),
            }],
            root,
            &registry,
            &policy,
            NativeToolContinuationPolicy {
                max_tool_calls: 4,
                max_result_bytes: 64 * 1024,
            },
        );

        assert!(results.is_ok());
        let Some(results) = results.ok() else {
            return;
        };
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider_call_id.as_deref(), Some("call-read-1"));
        assert!(results[0].content.contains("\"text\":\"alpha\\nbeta\\n\""));
        assert!(!results[0].redacted);
        let raw_log = serde_json::to_string(&log.events);
        assert!(raw_log.is_ok());
        let Some(raw_log) = raw_log.ok() else {
            return;
        };
        assert!(raw_log.contains("read_text_file result redacted"));
        assert!(raw_log.contains("result_content"));
        assert!(raw_log.contains("alpha"));
        assert!(raw_log.contains("notes.txt"));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_search_project_returns_bounded_matches_with_persisted_evidence()
     {
        let root_path = temp_resource_dir("provider-search-project");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        assert!(
            std::fs::write(
                root_path.join("src/lib.rs"),
                "needle one\nnone\nneedle two\n"
            )
            .is_ok()
        );
        let root = NativeResourceRoot::project(&root_path);
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info"],
                ["search_project"],
                std::iter::empty::<&str>(),
            );
        let mut log = NativeSessionLog::default();
        let context = NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
        };

        let results = build_project_readonly_provider_tool_results(
            &mut log,
            &context,
            vec![ProviderToolCall {
                call_id: String::from("call-search-1"),
                name: String::from("search_project"),
                arguments_json: serde_json::json!({"query": "needle"}),
            }],
            root,
            &registry,
            &policy,
            NativeToolContinuationPolicy {
                max_tool_calls: 4,
                max_result_bytes: 64 * 1024,
            },
        );

        assert!(results.is_ok());
        let Some(results) = results.ok() else {
            return;
        };
        assert!(results[0].content.contains("\"outcome\":\"search\""));
        assert!(results[0].content.contains("\"line_number\":1"));
        assert!(results[0].content.contains("needle one"));
        assert!(!results[0].content.contains("\"query\""));
        let raw_log = serde_json::to_string(&log.events);
        assert!(raw_log.is_ok());
        let Some(raw_log) = raw_log.ok() else {
            return;
        };
        assert!(raw_log.contains("search_project matches=2 truncated=false"));
        assert!(raw_log.contains("needle one"));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_list_project_paths_returns_entries_with_persisted_evidence()
     {
        let root_path = temp_resource_dir("provider-list-project-paths");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        assert!(std::fs::write(root_path.join("src/lib.rs"), "lib").is_ok());
        assert!(std::fs::write(root_path.join("src/main.rs"), "main").is_ok());
        let root = NativeResourceRoot::project(&root_path);
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info"],
                ["list_project_paths"],
                std::iter::empty::<&str>(),
            );
        let mut log = NativeSessionLog::default();
        let context = NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
        };

        let results = build_project_readonly_provider_tool_results(
            &mut log,
            &context,
            vec![ProviderToolCall {
                call_id: String::from("call-list-1"),
                name: String::from("list_project_paths"),
                arguments_json: serde_json::json!({"path": "src"}),
            }],
            root,
            &registry,
            &policy,
            NativeToolContinuationPolicy {
                max_tool_calls: 4,
                max_result_bytes: 64 * 1024,
            },
        );

        assert!(results.is_ok());
        let Some(results) = results.ok() else {
            return;
        };
        assert!(results[0].content.contains("\"outcome\":\"list\""));
        assert!(results[0].content.contains("\"path\":\"src/lib.rs\""));
        let raw_log = serde_json::to_string(&log.events);
        assert!(raw_log.is_ok());
        let Some(raw_log) = raw_log.ok() else {
            return;
        };
        assert!(raw_log.contains("list_project_paths entries=2 truncated=false"));
        assert!(raw_log.contains("src/lib.rs"));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_content_requires_content_policy() {
        let root_path = temp_resource_dir("provider-content-policy");
        assert!(std::fs::write(root_path.join("notes.txt"), "secret").is_ok());
        let root = NativeResourceRoot::project(&root_path);
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let mut log = NativeSessionLog::default();
        let context = NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
        };

        let result = build_project_readonly_provider_tool_results(
            &mut log,
            &context,
            vec![ProviderToolCall {
                call_id: String::from("call-read-1"),
                name: String::from("read_text_file"),
                arguments_json: serde_json::json!({"path": "notes.txt"}),
            }],
            root,
            &registry,
            &policy,
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::PermissionDenied
            ))
        );
        let raw_log = serde_json::to_string(&log.events);
        assert!(raw_log.is_ok());
        let Some(raw_log) = raw_log.ok() else {
            return;
        };
        assert!(!raw_log.contains("secret"));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn extension_executor_routes_through_native_tool_workflow_and_records_evidence() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let extension_tool = NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Hidden,
        );
        assert_eq!(registry.register_extension_tool(extension_tool), Ok(()));
        let router = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata(
                "example.toy-tools",
                "{\"kind\":\"toy\",\"visibility\":\"local\"}",
            ),
        )]);
        let workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool("toy_tool"),
            executor: &router,
            continuation_policy: NativeToolContinuationPolicy::fixture_default(),
        };
        let mut log = NativeSessionLog::default();

        let results = workflow.build_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );

        assert_eq!(
            results,
            Ok(vec![NativeProviderToolResult {
                tool_request_id: String::from("tool-request-1"),
                provider_call_id: Some(String::from("provider-call-1")),
                status: NativeToolOutcome::Completed,
                content: String::from("{\"kind\":\"toy\",\"visibility\":\"local\"}"),
                byte_count: 35,
                redacted: false,
                truncated: false,
                reason: None,
            }])
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.first(),
            Some(NativeSessionEvent::ToolRequestRecorded {
                tool_name,
                permission: NativeToolPermissionState::Allowed,
                argument_summary,
                ..
            }) if tool_name == "toy_tool"
                && argument_summary.summary == "tool payload redacted"
        ));
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                reason: None,
                result_summary: Some(summary),
                ..
            }) if summary.summary == "{\"kind\":\"toy\",\"visibility\":\"local\"}"
                && summary.byte_count == 35
                && !summary.redacted
                && !summary.truncated
        ));
    }

    #[test]
    fn extension_executor_invokes_metadata_tool_through_host_session() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Hidden,
            )),
            Ok(())
        );
        let invoker = RecordingExtensionHostInvoker::new(Ok(String::from(
            "{\"kind\":\"toy\",\"label\":\"fixture\"}",
        )));
        let router = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::host_metadata(
                "example.toy-tools",
                invoker.clone(),
                Duration::from_secs(2),
            ),
        )]);
        let workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool("toy_tool"),
            executor: &router,
            continuation_policy: NativeToolContinuationPolicy::fixture_default(),
        };
        let mut log = NativeSessionLog::default();

        let results = workflow.build_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );

        assert_eq!(
            results,
            Ok(vec![NativeProviderToolResult {
                tool_request_id: String::from("tool-request-1"),
                provider_call_id: Some(String::from("provider-call-1")),
                status: NativeToolOutcome::Completed,
                content: String::from("{\"kind\":\"toy\",\"label\":\"fixture\"}"),
                byte_count: 32,
                redacted: false,
                truncated: false,
                reason: None,
            }])
        );
        assert_eq!(
            invoker.calls(),
            vec![RecordedExtensionHostInvocation {
                request_id: String::from("tool-request-1"),
                tool_name: String::from("toy_tool"),
                arguments: serde_json::json!({"label":"fixture"}),
                timeout: Duration::from_secs(2),
            }]
        );
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                reason: None,
                result_summary: Some(summary),
                ..
            }) if summary.summary == "{\"kind\":\"toy\",\"label\":\"fixture\"}"
                && summary.byte_count == 32
        ));
    }

    #[test]
    fn extension_executor_host_failures_are_categorized() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Hidden,
            )),
            Ok(())
        );
        let router = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::host_metadata(
                "example.toy-tools",
                RecordingExtensionHostInvoker::new(Err(ExtensionHostProtocolError::TimedOut)),
                Duration::from_millis(1),
            ),
        )]);
        let workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool("toy_tool"),
            executor: &router,
            continuation_policy: NativeToolContinuationPolicy::fixture_default(),
        };
        let mut log = NativeSessionLog::default();

        let result = workflow.build_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Execution(
                NativeToolExecutionError::ExtensionHost {
                    error: ExtensionHostProtocolError::TimedOut
                }
            ))
        );
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "extension_host_timed_out"
        ));
    }

    #[test]
    fn extension_executor_failure_modes_are_categorized() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Hidden,
            )),
            Ok(())
        );
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "invalid_json_tool",
                "Return invalid static fixture metadata.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Hidden,
            )),
            Ok(())
        );
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "mismatched_owner_tool",
                "Return metadata from a mismatched handler owner.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Hidden,
            )),
            Ok(())
        );
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "large_tool",
                "Return larger static fixture metadata.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Hidden,
            )),
            Ok(())
        );
        let malformed_router = ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::malformed_result("example.toy-tools"),
        )]);
        let denied_workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::deny_all(),
            executor: &malformed_router,
            continuation_policy: NativeToolContinuationPolicy::fixture_default(),
        };
        let malformed_workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool("toy_tool"),
            executor: &malformed_router,
            continuation_policy: NativeToolContinuationPolicy::fixture_default(),
        };
        let large_router = ExtensionToolExecutorRouter::from_handlers([(
            "large_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "{\"kind\":\"toy\"}"),
        )]);
        let oversized_workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool(
                "large_tool",
            ),
            executor: &large_router,
            continuation_policy: NativeToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes: 4,
            },
        };
        let invalid_json_router = ExtensionToolExecutorRouter::from_handlers([(
            "invalid_json_tool",
            ExtensionToolHandler::static_metadata("example.toy-tools", "not-json"),
        )]);
        let invalid_json_workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool(
                "invalid_json_tool",
            ),
            executor: &invalid_json_router,
            continuation_policy: NativeToolContinuationPolicy::fixture_default(),
        };
        let owner_mismatch_router = ExtensionToolExecutorRouter::from_handlers([(
            "mismatched_owner_tool",
            ExtensionToolHandler::static_metadata("example.other-tools", "{\"kind\":\"toy\"}"),
        )]);
        let owner_mismatch_workflow = NativeToolContinuationWorkflow {
            registry: &registry,
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool(
                "mismatched_owner_tool",
            ),
            executor: &owner_mismatch_router,
            continuation_policy: NativeToolContinuationPolicy::fixture_default(),
        };
        let mut denied_log = NativeSessionLog::default();
        let denied = denied_workflow.build_provider_tool_results(
            &mut denied_log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );
        let mut malformed_log = NativeSessionLog::default();
        let malformed = malformed_workflow.build_provider_tool_results(
            &mut malformed_log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );
        let mut oversized_log = NativeSessionLog::default();
        let oversized = oversized_workflow.build_provider_tool_results(
            &mut oversized_log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "large_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );
        let mut invalid_json_log = NativeSessionLog::default();
        let invalid_json = invalid_json_workflow.build_provider_tool_results(
            &mut invalid_json_log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "invalid_json_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );
        let mut owner_mismatch_log = NativeSessionLog::default();
        let owner_mismatch = owner_mismatch_workflow.build_provider_tool_results(
            &mut owner_mismatch_log,
            &fixture_continuation_context(),
            vec![provider_tool_call(
                "provider-call-1",
                "mismatched_owner_tool",
                serde_json::json!({"label":"fixture"}),
            )],
        );

        assert_eq!(
            denied,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::PermissionDenied
            ))
        );
        assert!(matches!(
            denied_log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Denied,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "permission_denied"
        ));
        assert_eq!(
            malformed,
            Err(NativeToolContinuationError::Execution(
                NativeToolExecutionError::MalformedResult
            ))
        );
        assert!(matches!(
            malformed_log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "malformed_result"
        ));
        assert!(matches!(
            oversized,
            Err(NativeToolContinuationError::ResultTooLarge {
                ref tool_call_id,
                max_bytes,
                actual_bytes,
            }) if tool_call_id == "provider-call-1" && max_bytes == 4 && actual_bytes > max_bytes
        ));
        assert!(matches!(
            oversized_log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "result_too_large"
        ));
        assert_eq!(
            invalid_json,
            Err(NativeToolContinuationError::Execution(
                NativeToolExecutionError::MalformedResult
            ))
        );
        assert!(matches!(
            invalid_json_log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "malformed_result"
        ));
        assert_eq!(
            owner_mismatch,
            Err(NativeToolContinuationError::Execution(
                NativeToolExecutionError::UnsupportedTool
            ))
        );
        assert!(matches!(
            owner_mismatch_log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "unsupported_tool"
        ));
    }

    #[test]
    fn project_readonly_provider_tool_results_deny_without_execution() {
        let root_path = temp_resource_dir("native-readonly-tool-loop-denied");
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("project_path_info"),
            arguments_json: serde_json::json!({"path":"Cargo.toml"}),
        }];
        let mut log = NativeSessionLog::default();

        let Some(root) = root else {
            return;
        };
        let result = build_project_readonly_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            root,
            &NativeToolRegistry::with_project_read_only_tools(),
            &NativeToolPermissionPolicy::deny_all(),
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::PermissionDenied
            ))
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Denied,
                result_summary: None,
                ..
            })
        ));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_reject_unknown_tool_without_execution() {
        let root_path = temp_resource_dir("native-readonly-tool-loop-unknown");
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("read"),
            arguments_json: serde_json::json!({"path":"Cargo.toml"}),
        }];
        let mut log = NativeSessionLog::default();

        let Some(root) = root else {
            return;
        };
        let result = build_project_readonly_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            root,
            &NativeToolRegistry::with_project_read_only_tools(),
            &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::UnknownTool
            ))
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::ValidationFailed,
                result_summary: None,
                ..
            })
        ));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_record_resource_path_failure() {
        let root_path = temp_resource_dir("native-readonly-tool-loop-missing-path");
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("project_path_info"),
            arguments_json: serde_json::json!({"path":"missing.txt"}),
        }];
        let mut log = NativeSessionLog::default();

        let Some(root) = root else {
            return;
        };
        let result = build_project_readonly_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            root,
            &NativeToolRegistry::with_project_read_only_tools(),
            &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Execution(
                NativeToolExecutionError::ResourcePath {
                    error: NativeResourcePathError::Missing
                }
            ))
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.first(),
            Some(NativeSessionEvent::ToolRequestRecorded {
                tool_name,
                permission: NativeToolPermissionState::Allowed,
                ..
            }) if tool_name == "project_path_info"
        ));
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "resource_path_missing"
                && !reason.contains(std::path::MAIN_SEPARATOR)
        ));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_enforce_tool_call_limit_before_execution() {
        let root_path = temp_resource_dir("native-readonly-tool-loop-call-limit");
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        assert!(std::fs::write(root_path.join("README.md"), "# project\n").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let calls = vec![
            ProviderToolCall {
                call_id: String::from("provider-call-1"),
                name: String::from("project_path_info"),
                arguments_json: serde_json::json!({"path":"Cargo.toml"}),
            },
            ProviderToolCall {
                call_id: String::from("provider-call-2"),
                name: String::from("project_path_info"),
                arguments_json: serde_json::json!({"path":"README.md"}),
            },
        ];
        let mut log = NativeSessionLog::default();

        let Some(root) = root else {
            return;
        };
        let result = build_project_readonly_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            root,
            &NativeToolRegistry::with_project_read_only_tools(),
            &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
            NativeToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes: 256,
            },
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::TooManyToolCalls { max: 1, actual: 2 })
        );
        assert!(log.events.is_empty());
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn project_readonly_provider_tool_results_enforce_result_size_limit() {
        let root_path = temp_resource_dir("native-readonly-tool-loop-result-limit");
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("project_path_info"),
            arguments_json: serde_json::json!({"path":"Cargo.toml"}),
        }];
        let mut log = NativeSessionLog::default();
        let max_result_bytes = 1;

        let Some(root) = root else {
            return;
        };
        let result = build_project_readonly_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            root,
            &NativeToolRegistry::with_project_read_only_tools(),
            &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
            NativeToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes,
            },
        );

        assert!(matches!(
            result,
            Err(NativeToolContinuationError::ResultTooLarge {
                ref tool_call_id,
                max_bytes,
                actual_bytes,
            }) if tool_call_id == "provider-call-1"
                && max_bytes == max_result_bytes
                && actual_bytes > max_bytes
        ));
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.first(),
            Some(NativeSessionEvent::ToolRequestRecorded {
                tool_name,
                permission: NativeToolPermissionState::Allowed,
                ..
            }) if tool_name == "project_path_info"
        ));
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                result_summary: None,
                ..
            }) if reason == "result_too_large"
        ));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn fixture_provider_tool_results_stop_on_validation_failure() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"note":"missing label"}),
        }];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::MissingRequiredField {
                    field: String::from("label")
                }
            ))
        );
        assert_eq!(log.events.len(), 2);
    }

    #[test]
    fn fixture_provider_tool_results_stop_on_permission_denial() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"ok"}),
        }];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::deny_all(),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::PermissionDenied
            ))
        );
        assert_eq!(log.events.len(), 2);
    }

    #[test]
    fn fixture_provider_tool_results_enforce_result_size_limit() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"secret-label"}),
        }];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes: 1,
            },
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::ResultTooLarge {
                tool_call_id: String::from("provider-call-1"),
                max_bytes: 1,
                actual_bytes: 24,
            })
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                ..
            }) if reason == "result_too_large"
        ));
    }

    #[test]
    fn fixture_provider_tool_results_enforce_tool_call_limit() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![
            ProviderToolCall {
                call_id: String::from("provider-call-1"),
                name: String::from("fixture_echo_metadata"),
                arguments_json: serde_json::json!({"label":"one"}),
            },
            ProviderToolCall {
                call_id: String::from("provider-call-2"),
                name: String::from("fixture_echo_metadata"),
                arguments_json: serde_json::json!({"label":"two"}),
            },
        ];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes: 256,
            },
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::TooManyToolCalls { max: 1, actual: 2 })
        );
        assert!(log.events.is_empty());
    }

    #[test]
    fn provider_tool_call_maps_to_pending_native_tool_request() {
        let tool_call = ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"ok"}),
        };

        let request = pending_tool_request_from_provider_call(
            "tool-request-1",
            NativeTurnId(String::from("turn-1")),
            tool_call,
        );

        assert_eq!(
            request,
            PendingNativeToolRequest {
                request_id: String::from("tool-request-1"),
                turn_id: NativeTurnId(String::from("turn-1")),
                tool_name: String::from("fixture_echo_metadata"),
                provider_call_id: Some(String::from("provider-call-1")),
                arguments: serde_json::json!({"label":"ok"}),
            }
        );
    }

    #[test]
    fn provider_tool_call_validation_persists_validated_argument_content() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let policy = NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata");
        let tool_call = ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"persisted-label"}),
        };
        let request = pending_tool_request_from_provider_call(
            "tool-request-1",
            NativeTurnId(String::from("turn-1")),
            tool_call,
        );
        let mut log = NativeSessionLog::default();

        let validation = record_native_tool_validation(
            &mut log,
            NativeSessionId(String::from("session-1")),
            &request,
            &registry,
            &policy,
        );

        assert!(validation.is_ok());
        assert_eq!(log.events.len(), 1);
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolRequestRecorded {
                argument_content: Some(content),
                ..
            } if content.contains("persisted-label")
        )));
        let path = temp_log_path("native-provider-tool-validation");
        assert!(log.write_to_file(&path).is_ok());
        let raw = std::fs::read_to_string(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());
        assert!(raw.is_some_and(|raw| raw.contains("persisted-label")));
    }

    #[test]
    fn provider_tool_call_validation_records_rejection_without_execution() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = pending_tool_request_from_provider_call(
            "tool-request-1",
            NativeTurnId(String::from("turn-1")),
            ProviderToolCall {
                call_id: String::from("provider-call-1"),
                name: String::from("fixture_echo_metadata"),
                arguments_json: serde_json::json!({"note":"missing label"}),
            },
        );
        let mut log = NativeSessionLog::default();

        let validation = record_native_tool_validation(
            &mut log,
            NativeSessionId(String::from("session-1")),
            &request,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(
            validation,
            Err(NativeToolError::MissingRequiredField {
                field: String::from("label")
            })
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::ValidationFailed,
                result_summary: None,
                ..
            })
        ));
    }

    #[test]
    fn fixture_native_tool_executor_runs_only_validated_fixture_tool() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let policy = NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata");
        let request = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"secret-label"}),
        );
        let validation = registry.validate_request(&request, &policy).ok();
        assert!(validation.is_some());

        let result = validation
            .as_ref()
            .map(|validation| FixtureNativeToolExecutor.execute(&registry, &request, validation));

        assert_eq!(
            result,
            Some(Ok(NativeToolExecutionResult {
                request_id: String::from("tool-request-1"),
                summary: String::from("fixture tool executed with redacted arguments"),
                byte_count: 24,
                redacted: true,
                truncated: false,
            }))
        );
    }

    #[test]
    fn fixture_native_tool_executor_rejects_unvalidated_permission() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"label":"ok"}));
        let validation = super::NativeToolValidation {
            request_id: String::from("tool-request-1"),
            tool_name: String::from("fixture_echo_metadata"),
            permission: NativeToolPermissionState::Denied,
        };

        let result = FixtureNativeToolExecutor.execute(&registry, &request, &validation);

        assert_eq!(result, Err(NativeToolExecutionError::PermissionDenied));
    }

    #[test]
    fn native_tool_registry_rejects_unknown_tool() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = fixture_tool_request("missing_tool", serde_json::json!({"label":"ok"}));

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("missing_tool"),
        );

        assert_eq!(result, Err(NativeToolError::UnknownTool));
    }

    #[test]
    fn native_tool_registry_rejects_malformed_args() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!("not-object"));

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(result, Err(NativeToolError::MalformedArguments));
    }

    #[test]
    fn native_tool_registry_rejects_schema_mismatch() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let missing =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"note":"only"}));
        let wrong_type =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"label": 42}));
        let unexpected = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"ok","extra":"nope"}),
        );
        let policy = NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata");

        assert_eq!(
            registry.validate_request(&missing, &policy),
            Err(NativeToolError::MissingRequiredField {
                field: String::from("label")
            })
        );
        assert_eq!(
            registry.validate_request(&wrong_type, &policy),
            Err(NativeToolError::InvalidFieldType {
                field: String::from("label")
            })
        );
        assert_eq!(
            registry.validate_request(&unexpected, &policy),
            Err(NativeToolError::UnexpectedField {
                field: String::from("extra")
            })
        );
    }

    #[test]
    fn native_tool_registry_rejects_oversized_args() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"x".repeat(2048)}),
        );

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(result, Err(NativeToolError::ArgumentsTooLarge));
    }

    #[test]
    fn native_tool_registry_denies_by_default() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"label":"ok"}));

        let result = registry.validate_request(&request, &NativeToolPermissionPolicy::deny_all());

        assert_eq!(result, Err(NativeToolError::PermissionDenied));
    }

    #[test]
    fn native_tool_registry_allows_explicit_fixture_policy() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"ok","note":"fixture only"}),
        );

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(
            result,
            Ok(super::NativeToolValidation {
                request_id: String::from("tool-request-1"),
                tool_name: String::from("fixture_echo_metadata"),
                permission: NativeToolPermissionState::Allowed,
            })
        );
    }

    #[test]
    fn native_tool_registry_exposes_canonical_agent_edit_tools() {
        let registry = NativeToolRegistry::with_agent_edit_tools();

        let edit = registry.get("edit_text_file");
        assert!(edit.is_some());
        let Some(edit) = edit else {
            return;
        };
        assert_eq!(edit.risk, NativeToolRisk::MutatesLocalState);
        assert_eq!(edit.owner, NativeToolOwner::BuiltIn);
        assert_eq!(edit.provider_visibility, ProviderToolVisibility::Visible);

        let create = registry.get("create_text_file");
        assert!(create.is_some());
        let Some(create) = create else {
            return;
        };
        assert_eq!(create.risk, NativeToolRisk::MutatesLocalState);
        assert_eq!(create.owner, NativeToolOwner::BuiltIn);
        assert_eq!(create.provider_visibility, ProviderToolVisibility::Visible);
    }

    #[test]
    fn agent_edit_tool_schema_rejects_expected_sha256_from_provider() {
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "expected_sha256": "provider-must-not-supply-this",
                "find": "old",
                "replace": "new"
            }),
        };

        assert_eq!(
            registry.validate_request_schema_only(&request).err(),
            Some(NativeToolError::UnexpectedField {
                field: String::from("expected_sha256")
            })
        );
    }

    #[test]
    fn agent_edit_text_file_normalization_computes_expected_hash() {
        let root_guard = temp_native_edit_root("agent-edit-normalize-modify");
        root_guard.write("notes.txt", "alpha\n");
        let resource_root = NativeResourceRoot::project(root_guard.root()).ok();
        assert!(resource_root.is_some());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let request = fixture_tool_request(
            "edit_text_file",
            serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        );

        let normalized = resource_root.as_ref().and_then(|resource_root| {
            normalize_agent_edit_tool_request(
                &registry,
                resource_root,
                &request,
                NativeEditPolicy::test(),
            )
            .ok()
        });

        assert_eq!(
            normalized
                .as_ref()
                .map(|normalized| normalized.path.as_str()),
            Some("notes.txt")
        );
        assert_eq!(
            normalized
                .as_ref()
                .map(|normalized| normalized.operation.as_str()),
            Some("edit_text_file")
        );
        let operations = normalized
            .as_ref()
            .map(|normalized| normalized.transaction.operations.as_slice());
        assert_eq!(operations.map(<[NativeEditOperation]>::len), Some(1));
        let modify = operations.and_then(|operations| match operations {
            [
                NativeEditOperation::ModifyTextFile {
                    path,
                    expected_sha256,
                    hunks,
                },
            ] => Some((path, expected_sha256, hunks)),
            _ => None,
        });
        assert!(modify.is_some());
        let Some((path, expected_sha256, hunks)) = modify else {
            return;
        };
        assert_eq!(path, "notes.txt");
        assert_eq!(expected_sha256.as_str(), sha256_hex_for_test("alpha\n"));
        assert_eq!(
            hunks.as_slice(),
            [NativeEditHunk {
                find: String::from("alpha"),
                replace: String::from("beta"),
            }]
        );
    }

    #[test]
    fn agent_create_text_file_normalization_builds_create_transaction() {
        let root_guard = temp_native_edit_root("agent-edit-normalize-create");
        let resource_root = NativeResourceRoot::project(root_guard.root()).ok();
        assert!(resource_root.is_some());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let request = fixture_tool_request(
            "create_text_file",
            serde_json::json!({
                "path": "new.txt",
                "content": "created\n"
            }),
        );

        let normalized = resource_root.as_ref().and_then(|resource_root| {
            normalize_agent_edit_tool_request(
                &registry,
                resource_root,
                &request,
                NativeEditPolicy::test(),
            )
            .ok()
        });

        assert_eq!(
            normalized
                .as_ref()
                .map(|normalized| normalized.path.as_str()),
            Some("new.txt")
        );
        assert_eq!(
            normalized
                .as_ref()
                .map(|normalized| normalized.operation.as_str()),
            Some("create_text_file")
        );
        let operations = normalized
            .as_ref()
            .map(|normalized| normalized.transaction.operations.as_slice());
        assert_eq!(operations.map(<[NativeEditOperation]>::len), Some(1));
        let create = operations.and_then(|operations| match operations {
            [NativeEditOperation::CreateTextFile { path, content }] => Some((path, content)),
            _ => None,
        });
        assert!(create.is_some());
        let Some((path, content)) = create else {
            return;
        };
        assert_eq!(path, "new.txt");
        assert_eq!(content, "created\n");
    }

    #[test]
    fn agent_edit_text_file_normalization_rejects_metadata_path() {
        let root_guard = temp_native_edit_root("agent-edit-normalize-metadata");
        root_guard.write(".git/config", "protected\n");
        let resource_root = NativeResourceRoot::project(root_guard.root()).ok();
        assert!(resource_root.is_some());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let request = fixture_tool_request(
            "edit_text_file",
            serde_json::json!({
                "path": ".git/config",
                "find": "protected",
                "replace": "changed"
            }),
        );

        let normalized = resource_root.as_ref().map(|resource_root| {
            normalize_agent_edit_tool_request(
                &registry,
                resource_root,
                &request,
                NativeEditPolicy::test(),
            )
        });

        assert_eq!(normalized, Some(Err(NativeToolError::MalformedArguments)));
    }

    #[test]
    fn agent_edit_tool_allow_mode_applies_and_preserves_provider_call_id() {
        let root_guard = temp_native_edit_root("agent-edit-allow");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store_path = root_guard.root().join("session.jsonl");
        let store = NativeJsonlSessionStore::new(store_path.clone());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };

        let result = execute_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::for_edit_mode(
                    NativePermissionMode::Allow,
                ),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        assert!(result.is_ok());
        let Some(result) = result.ok() else {
            return;
        };
        assert_eq!(result.provider_call_id.as_deref(), Some("call-edit-1"));
        assert_eq!(result.status, NativeToolOutcome::Completed);
        assert_eq!(
            std::fs::read_to_string(root_guard.root().join("notes.txt")).ok(),
            Some(String::from("beta\n"))
        );

        let log = NativeJsonlSessionStore::new(store_path).load();
        assert!(log.is_ok());
        assert!(
            log.as_ref()
                .is_ok_and(|log| events_are_ordered_before_completed_apply(&log.events))
        );
    }

    #[test]
    fn agent_edit_tool_ask_mode_returns_review_without_applying() {
        let root_guard = temp_native_edit_root("agent-edit-ask");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store = NativeJsonlSessionStore::new(root_guard.root().join("session.jsonl"));
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };

        let outcome = prepare_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::default_local_edit(),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        assert!(matches!(
            outcome,
            Ok(NativeAgentEditToolPrepared::NeedsUserReview { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(root_guard.root().join("notes.txt")).ok(),
            Some(String::from("alpha\n"))
        );
    }

    #[test]
    fn agent_edit_tool_prepare_review_carries_trace_identity() {
        let root_guard = temp_native_edit_root("agent-edit-trace-review");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store = NativeJsonlSessionStore::new(root_guard.root().join("session.jsonl"));
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };

        let prepared = prepare_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::default_local_edit(),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        let Ok(NativeAgentEditToolPrepared::NeedsUserReview { trace_id, .. }) = prepared else {
            assert!(matches!(
                prepared,
                Ok(NativeAgentEditToolPrepared::NeedsUserReview { .. })
            ));
            return;
        };
        assert!(trace_id.0.starts_with("edit-trace-"));
    }

    #[test]
    fn agent_edit_tool_reject_review_returns_completed_rejection_result() {
        let root_guard = temp_native_edit_root("agent-edit-reject");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store = NativeJsonlSessionStore::new(root_guard.root().join("session.jsonl"));
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };

        let prepared = prepare_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::default_local_edit(),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );
        let Ok(NativeAgentEditToolPrepared::NeedsUserReview {
            trace_id,
            request_id,
            provider_call_id,
            preview,
            path,
            operation,
        }) = prepared
        else {
            assert!(matches!(
                prepared,
                Ok(NativeAgentEditToolPrepared::NeedsUserReview { .. })
            ));
            return;
        };
        let preview_id = preview.preview_id.clone();

        let result = reject_agent_edit_tool_review(
            &mut access,
            &store,
            PendingAgentEditToolReview {
                trace_id,
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                request_id,
                provider_call_id,
                preview_id: preview.preview_id,
                permission_decision_id: preview.permission_decision_id,
                path,
                operation,
            },
        );

        assert!(result.is_ok());
        let Some(result) = result.ok() else {
            return;
        };
        assert_eq!(result.provider_call_id.as_deref(), Some("call-edit-1"));
        assert_eq!(result.status, NativeToolOutcome::Completed);
        assert_eq!(result.reason.as_deref(), Some("user_rejected"));
        assert!(result.content.contains("\"outcome\":\"rejected\""));
        assert!(!access.has_pending_preview(&preview_id));
        assert_eq!(
            std::fs::read_to_string(root_guard.root().join("notes.txt")).ok(),
            Some(String::from("alpha\n"))
        );
    }

    #[test]
    fn agent_edit_tool_duplicate_create_returns_failed_result_with_guidance() {
        let root_guard = temp_native_edit_root("agent-edit-duplicate-create");
        root_guard.write("dogfood.txt", "existing content\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store_path = root_guard.root().join("session.jsonl");
        let store = NativeJsonlSessionStore::new(store_path.clone());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("create_text_file"),
            provider_call_id: Some(String::from("call-create-1")),
            arguments: serde_json::json!({
                "path": "dogfood.txt",
                "content": "hello"
            }),
        };

        let prepared = prepare_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::default_local_edit(),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        assert!(prepared.is_ok());
        let Ok(NativeAgentEditToolPrepared::Failed { result, .. }) = prepared else {
            unreachable!("duplicate create should prepare a failed tool result");
        };
        assert_eq!(result.status, NativeToolOutcome::Failed);
        assert_eq!(result.provider_call_id.as_deref(), Some("call-create-1"));
        assert_eq!(result.reason.as_deref(), Some("target_exists"));
        assert!(result.content.contains("\"outcome\":\"failed\""));
        assert!(result.content.contains("target_exists"));
        assert!(result.content.contains("read_text_file"));
        assert_eq!(
            std::fs::read_to_string(root_guard.root().join("dogfood.txt")).ok(),
            Some(String::from("existing content\n"))
        );
        let log = NativeJsonlSessionStore::new(store_path).load();
        assert!(log.is_ok());
        let Some(log) = log.ok() else {
            return;
        };
        assert!(log.events.iter().any(|event| {
            matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::Failed,
                    reason: Some(reason),
                    ..
                } if reason == "target_exists"
            )
        }));
    }

    #[test]
    fn agent_edit_tool_missing_provider_call_id_records_validation_failure() {
        let root_guard = temp_native_edit_root("agent-edit-missing-provider-call");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store_path = root_guard.root().join("session.jsonl");
        let store = NativeJsonlSessionStore::new(store_path.clone());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: None,
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };

        let result = prepare_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::default_local_edit(),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::MalformedArguments
            ))
        );
        let log = NativeJsonlSessionStore::new(store_path).load();
        assert!(log.is_ok());
        let Some(log) = log.ok() else {
            return;
        };
        assert!(log.events.iter().any(|event| {
            matches!(
                event,
                NativeSessionEvent::ToolRequestRecorded {
                    validation: Err(NativeToolError::MalformedArguments),
                    provider_call_id: None,
                    ..
                }
            )
        }));
        assert!(log.events.iter().any(|event| {
            matches!(
                event,
                NativeSessionEvent::ToolExecutionFinished {
                    outcome: NativeToolOutcome::ValidationFailed,
                    reason: Some(reason),
                    ..
                } if reason == "missing_provider_call_id"
            )
        }));
    }

    fn edit_trace_records(log: &NativeSessionLog) -> Vec<NativeEditTraceRecord> {
        log.events
            .iter()
            .filter_map(|event| match event {
                NativeSessionEvent::EditTraceRecorded { trace, .. } => Some(trace.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn agent_edit_tool_allow_mode_records_correlated_trace_phases() {
        let root_guard = temp_native_edit_root("agent-edit-trace-allow");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store_path = root_guard.root().join("session.jsonl");
        let store = NativeJsonlSessionStore::new(store_path.clone());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };

        let result = execute_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::for_edit_mode(
                    NativePermissionMode::Allow,
                ),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        assert!(result.is_ok());
        let log = NativeJsonlSessionStore::new(store_path).load();
        assert!(log.is_ok());
        let Some(log) = log.ok() else {
            return;
        };
        let traces = edit_trace_records(&log);
        let trace_id = traces.first().map(|trace| trace.trace_id.clone());
        assert!(trace_id.is_some());
        let Some(trace_id) = trace_id else {
            return;
        };
        for phase in [
            NativeEditTracePhase::ToolValidation,
            NativeEditTracePhase::ArgumentNormalization,
            NativeEditTracePhase::PermissionDecision,
            NativeEditTracePhase::Preview,
            NativeEditTracePhase::Apply,
            NativeEditTracePhase::ResultShaping,
        ] {
            assert!(traces.iter().any(|trace| {
                trace.trace_id == trace_id
                    && trace.phase == phase
                    && trace.outcome == NativeEditTraceOutcome::Completed
                    && trace.tool_request_id.as_ref().map(|id| id.0.as_str())
                        == Some("tool-request-1")
                    && trace.provider_call_id.as_deref() == Some("call-edit-1")
            }));
        }
    }

    #[test]
    fn agent_edit_tool_reject_review_records_rejected_trace_phase() {
        let root_guard = temp_native_edit_root("agent-edit-trace-reject");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store_path = root_guard.root().join("session.jsonl");
        let store = NativeJsonlSessionStore::new(store_path.clone());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some(String::from("call-edit-1")),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };
        let prepared = prepare_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::default_local_edit(),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );
        let Ok(NativeAgentEditToolPrepared::NeedsUserReview {
            trace_id,
            request_id,
            provider_call_id,
            preview,
            path,
            operation,
        }) = prepared
        else {
            assert!(matches!(
                prepared,
                Ok(NativeAgentEditToolPrepared::NeedsUserReview { .. })
            ));
            return;
        };

        let result = reject_agent_edit_tool_review(
            &mut access,
            &store,
            PendingAgentEditToolReview {
                trace_id: trace_id.clone(),
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                request_id,
                provider_call_id,
                preview_id: preview.preview_id,
                permission_decision_id: preview.permission_decision_id,
                path,
                operation,
            },
        );

        assert!(result.is_ok());
        let log = NativeJsonlSessionStore::new(store_path).load();
        assert!(log.is_ok());
        let Some(log) = log.ok() else {
            return;
        };
        let traces = edit_trace_records(&log);
        assert!(traces.iter().any(|trace| {
            trace.trace_id == trace_id
                && trace.phase == NativeEditTracePhase::Reject
                && trace.outcome == NativeEditTraceOutcome::Rejected
                && trace.reason_label.as_deref() == Some("user_rejected")
        }));
    }

    #[test]
    fn agent_edit_tool_missing_provider_call_id_records_validation_trace_without_transaction() {
        let root_guard = temp_native_edit_root("agent-edit-trace-missing-provider-call");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store_path = root_guard.root().join("session.jsonl");
        let store = NativeJsonlSessionStore::new(store_path.clone());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: None,
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": "beta"
            }),
        };

        let result = prepare_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::default_local_edit(),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::MalformedArguments
            ))
        );
        let log = NativeJsonlSessionStore::new(store_path).load();
        assert!(log.is_ok());
        let Some(log) = log.ok() else {
            return;
        };
        let traces = edit_trace_records(&log);
        assert!(traces.iter().any(|trace| {
            trace.phase == NativeEditTracePhase::ToolValidation
                && trace.outcome == NativeEditTraceOutcome::Failed
                && trace.reason_label.as_deref() == Some("missing_provider_call_id")
                && trace.transaction_id.is_none()
        }));
    }

    #[test]
    fn agent_edit_trace_records_are_bounded_and_do_not_include_raw_arguments() {
        let root_guard = temp_native_edit_root("agent-edit-trace-bounds");
        root_guard.write("notes.txt", "alpha\n");
        let root = NativeResourceRoot::project(root_guard.root());
        assert!(root.is_ok());
        let Ok(root) = root else {
            unreachable!("asserted root creation succeeds");
        };
        let store_path = root_guard.root().join("session.jsonl");
        let store = NativeJsonlSessionStore::new(store_path.clone());
        let registry = NativeToolRegistry::with_agent_edit_tools();
        let mut access = NativeEditAccess::default();
        let sentinel = "RAW_ARGUMENT_SENTINEL_DO_NOT_PERSIST";
        let request = PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: Some("call-".repeat(80)),
            arguments: serde_json::json!({
                "path": "notes.txt",
                "find": "alpha",
                "replace": sentinel
            }),
        };

        let result = execute_agent_edit_tool_request(
            &registry,
            &root,
            &mut access,
            &store,
            NativeAgentEditToolContext {
                session_id: NativeSessionId(String::from("default")),
                turn_id: NativeTurnId(String::from("turn-1")),
                permission_policy: NativePermissionPolicy::for_edit_mode(
                    NativePermissionMode::Allow,
                ),
                edit_policy: NativeEditPolicy::test(),
            },
            request,
        );

        assert!(result.is_ok());
        let raw = std::fs::read_to_string(&store_path);
        assert!(raw.is_ok());
        let Some(raw) = raw.ok() else {
            return;
        };
        assert!(raw.contains("edit_trace_recorded"));
        let log = NativeJsonlSessionStore::new(store_path).load();
        assert!(log.is_ok());
        let Some(log) = log.ok() else {
            return;
        };
        // Raw arguments persist once as tool request content, never inside
        // diagnostic trace records.
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolRequestRecorded {
                argument_content: Some(content),
                ..
            } if content.contains(sentinel)
        )));
        assert!(log.events.iter().all(|event| {
            !matches!(event, NativeSessionEvent::EditTraceRecorded { .. })
                || !serde_json::to_string(event).is_ok_and(|json| json.contains(sentinel))
        }));
        let traces = edit_trace_records(&log);
        assert!(traces.iter().all(|trace| {
            trace
                .provider_call_id
                .as_ref()
                .is_none_or(|provider_call_id| provider_call_id.len() <= 256)
        }));
    }

    #[test]
    fn native_tool_registry_registers_extension_owned_metadata_tool() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let candidate = ExtensionToolCandidate {
            extension_id: ExtensionId(String::from("example.toy-tools")),
            extension_version: String::from("0.1.0"),
            tool: ExtensionToolContribution {
                name: String::from("toy_tool"),
                description: String::from("Return static fixture metadata."),
                risk: ExtensionToolRisk::ReadsLocalMetadata,
                provider_visible: false,
            },
        };

        let registration = registry.register_extension_tool(candidate.to_native_definition());
        let definition = registry.get("toy_tool");
        let request = fixture_tool_request("toy_tool", serde_json::json!({"label":"fixture"}));
        let validation = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_project_metadata_tools([
                "project_path_info",
                "toy_tool",
            ]),
        );

        assert_eq!(registration, Ok(()));
        assert_eq!(
            definition.map(|definition| &definition.owner),
            Some(&NativeToolOwner::Extension {
                extension_id: String::from("example.toy-tools"),
                extension_version: Some(String::from("0.1.0")),
            })
        );
        assert_eq!(
            definition.map(|definition| definition.provider_visibility),
            Some(ProviderToolVisibility::Hidden)
        );
        assert_eq!(
            validation,
            Ok(super::NativeToolValidation {
                request_id: String::from("tool-request-1"),
                tool_name: String::from("toy_tool"),
                permission: NativeToolPermissionState::Allowed,
            })
        );
    }

    #[test]
    fn native_tool_registry_rejects_extension_tool_collisions() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let colliding = NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "project_path_info",
            "Collides with the built-in path metadata tool.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Hidden,
        );
        let unsupported = NativeToolDefinition {
            name: String::from("process_tool"),
            description: String::from("Attempts to run a process."),
            input_schema: NativeToolInputSchema::string_object(
                ["label"],
                std::iter::empty::<&str>(),
                512,
            ),
            risk: NativeToolRisk::RunsProcess,
            owner: NativeToolOwner::Extension {
                extension_id: String::from("example.toy-tools"),
                extension_version: None,
            },
            provider_visibility: ProviderToolVisibility::Hidden,
        };

        assert_eq!(
            registry.register_extension_tool(colliding),
            Err(NativeToolRegistrationError::DuplicateToolName {
                name: String::from("project_path_info")
            })
        );
        assert_eq!(
            registry.register_extension_tool(unsupported),
            Err(NativeToolRegistrationError::UnsupportedRisk {
                name: String::from("process_tool"),
                risk: NativeToolRisk::RunsProcess,
            })
        );
    }

    #[test]
    fn native_tool_registry_rejects_extension_registration_with_builtin_owner() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let builtin = NativeToolDefinition {
            name: String::from("unique_builtin_metadata"),
            description: String::from("A built-in-shaped metadata tool."),
            input_schema: NativeToolInputSchema::string_object(
                ["label"],
                std::iter::empty::<&str>(),
                512,
            ),
            risk: NativeToolRisk::ReadsLocalMetadata,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Hidden,
        };

        assert_eq!(
            registry.register_extension_tool(builtin),
            Err(NativeToolRegistrationError::UnsupportedOwner {
                name: String::from("unique_builtin_metadata")
            })
        );
        assert!(registry.get("unique_builtin_metadata").is_none());
    }

    #[test]
    fn extension_mutation_tool_registration_still_rejected() {
        let mut registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        let mut extension_tool = NativeToolDefinition::extension_metadata_tool(
            "example.extension",
            "extension_edit_text_file",
            "tries to edit files",
            NativeToolInputSchema::string_object(["path"], std::iter::empty::<&str>(), 1024),
            ProviderToolVisibility::Visible,
        );
        extension_tool.risk = NativeToolRisk::MutatesLocalState;

        assert_eq!(
            registry.register_extension_tool(extension_tool).err(),
            Some(NativeToolRegistrationError::UnsupportedRisk {
                name: String::from("extension_edit_text_file"),
                risk: NativeToolRisk::MutatesLocalState,
            })
        );
        assert!(registry.get("extension_edit_text_file").is_none());
    }

    #[test]
    fn provider_advertising_candidates_include_only_visible_allowed_routable_tools() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let toy_tool = NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Visible extension metadata tool.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Visible,
        );
        let hidden = NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "hidden_tool",
            "Hidden extension metadata tool.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Hidden,
        );

        assert_eq!(registry.register_extension_tool(toy_tool), Ok(()));
        assert_eq!(registry.register_extension_tool(hidden), Ok(()));

        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_tool",
            "hidden_tool",
        ]);
        let candidates = registry.provider_advertising_candidates(&policy, ["toy_tool"]);
        let names = candidates
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["toy_tool"]);
    }

    #[test]
    fn provider_advertising_candidates_require_explicit_agent_edit_policy() {
        let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        let no_edit_policy =
            NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let routable = ["project_path_info", "edit_text_file", "create_text_file"];

        let without_edits = registry.provider_advertising_candidates(&no_edit_policy, routable);
        assert_eq!(
            without_edits
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["project_path_info"]
        );

        let edit_policy = NativeToolPermissionPolicy::allow_project_metadata_and_agent_edit_tools(
            ["project_path_info"],
            ["edit_text_file", "create_text_file"],
        );
        let with_edits = registry.provider_advertising_candidates(&edit_policy, routable);
        assert_eq!(
            with_edits
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["project_path_info", "edit_text_file", "create_text_file"]
        );
    }

    #[test]
    fn provider_advertising_candidates_require_explicit_content_policy() {
        let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        let metadata_only = NativeToolPermissionPolicy::allow_project_metadata_and_agent_edit_tools(
            ["project_path_info"],
            ["edit_text_file", "create_text_file"],
        );
        let content_policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info"],
                ["read_text_file", "search_project", "list_project_paths"],
                ["edit_text_file", "create_text_file"],
            );
        let routable = [
            "project_path_info",
            "read_text_file",
            "search_project",
            "list_project_paths",
            "edit_text_file",
            "create_text_file",
        ];

        let metadata_only_names = registry
            .provider_advertising_candidates(&metadata_only, routable)
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        let content_names = registry
            .provider_advertising_candidates(&content_policy, routable)
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert_eq!(
            metadata_only_names,
            vec!["project_path_info", "edit_text_file", "create_text_file"]
        );
        assert_eq!(
            content_names,
            vec![
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
                "edit_text_file",
                "create_text_file",
            ]
        );
    }

    #[test]
    fn provider_turn_resolved_catalog_preserves_builtin_advertising() {
        let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
        let policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info"],
                ["read_text_file", "search_project", "list_project_paths"],
                ["edit_text_file", "create_text_file"],
            );
        let catalog = registry.resolve_provider_turn_catalog(
            &policy,
            [
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
                "edit_text_file",
                "create_text_file",
            ],
        );

        let names = catalog
            .tools()
            .iter()
            .map(|tool| tool.provider_name.as_str())
            .collect::<Vec<_>>();
        let implementation_names = catalog
            .tools()
            .iter()
            .map(|tool| tool.implementation_name.as_str())
            .collect::<Vec<_>>();
        let provenances = catalog
            .tools()
            .iter()
            .map(|tool| &tool.provenance)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
                "edit_text_file",
                "create_text_file",
            ]
        );
        assert_eq!(implementation_names, names);
        assert!(
            provenances
                .iter()
                .all(|provenance| **provenance == NativeToolProvenance::BuiltIn)
        );
    }

    #[test]
    fn provider_turn_resolved_catalog_includes_only_active_visible_extension_tools() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Visible extension metadata tool.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "inactive_tool",
                "Visible but not executable this turn.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "hidden_tool",
                "Executable but provider-hidden.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Hidden,
            )),
            Ok(())
        );
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_tool",
            "inactive_tool",
            "hidden_tool",
        ]);

        let catalog =
            registry.resolve_provider_turn_catalog(&policy, ["project_path_info", "toy_tool"]);
        let names = catalog
            .tools()
            .iter()
            .map(|tool| tool.provider_name.as_str())
            .collect::<Vec<_>>();
        let toy = catalog
            .tools()
            .iter()
            .find(|tool| tool.provider_name == "toy_tool");

        assert_eq!(names, vec!["project_path_info", "toy_tool"]);
        assert_eq!(
            toy.map(|tool| &tool.provenance),
            Some(&NativeToolProvenance::Extension {
                extension_id: String::from("example.toy-tools"),
                extension_version: String::from("unknown"),
            })
        );
    }

    #[test]
    fn provider_turn_resolved_catalog_advertises_schema_only_definitions() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Visible extension metadata tool.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_tool",
        ]);
        let catalog =
            registry.resolve_provider_turn_catalog(&policy, ["project_path_info", "toy_tool"]);

        let extension = build_provider_tool_advertising_extension(&catalog.provider_definitions());
        assert!(extension.is_ok());
        let Ok(extension) = extension else {
            return;
        };
        let parsed = parse_provider_tool_advertising_extensions(&[extension]);
        assert!(parsed.is_ok());
        let Some(advertising) = parsed.ok().flatten() else {
            return;
        };

        assert_eq!(
            advertising
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["project_path_info", "toy_tool"]
        );
        assert!(advertising.tools.iter().all(|tool| {
            tool.parameters
                .get("additionalProperties")
                .is_some_and(|value| value == false)
        }));
    }

    #[test]
    fn provider_turn_resolved_catalog_is_turn_snapshot() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_tool",
        ]);
        let before_activation =
            registry.resolve_provider_turn_catalog(&policy, ["project_path_info"]);

        assert_eq!(
            registry.register_extension_tool(NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Visible extension metadata tool.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Visible,
            )),
            Ok(())
        );
        let after_activation =
            registry.resolve_provider_turn_catalog(&policy, ["project_path_info", "toy_tool"]);

        assert!(
            before_activation
                .tools()
                .iter()
                .all(|tool| tool.provider_name != "toy_tool")
        );
        assert!(
            after_activation
                .tools()
                .iter()
                .any(|tool| tool.provider_name == "toy_tool")
        );
    }

    fn replacement_project_path_info_tool() -> NativeToolDefinition {
        NativeToolDefinition::extension_metadata_tool_with_version(
            "example.toy-tools",
            Some("1.2.3"),
            "toy_path_info",
            "Replacement path metadata implementation.",
            NativeToolDefinition::project_path_info().input_schema,
            ProviderToolVisibility::Visible,
        )
    }

    fn replacement_rule(
        mode: NativeToolResolutionMode,
        source: NativeToolReplacementSource,
    ) -> NativeToolReplacementRule {
        NativeToolReplacementRule {
            builtin_name: String::from("project_path_info"),
            extension_id: String::from("example.toy-tools"),
            extension_tool: String::from("toy_path_info"),
            mode,
            source,
        }
    }

    #[test]
    fn native_tool_replacement_accidental_builtin_collision_fails_closed() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let colliding = NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "project_path_info",
            "Accidental collision with built-in metadata.",
            NativeToolDefinition::project_path_info().input_schema,
            ProviderToolVisibility::Visible,
        );

        assert_eq!(
            registry.register_extension_tool(colliding),
            Err(NativeToolRegistrationError::DuplicateToolName {
                name: String::from("project_path_info")
            })
        );
    }

    #[test]
    fn native_tool_replacement_alias_only_exposes_extension_name() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(replacement_project_path_info_tool()),
            Ok(())
        );
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_path_info",
        ]);
        let replacement_policy = NativeToolReplacementPolicy::from_rules([replacement_rule(
            NativeToolResolutionMode::AliasOnly,
            NativeToolReplacementSource::Profile,
        )]);

        let catalog = registry.resolve_provider_turn_catalog_with_replacements(
            &policy,
            ["project_path_info", "toy_path_info"],
            &replacement_policy,
        );

        assert!(catalog.is_ok());
        let Some(catalog) = catalog.ok() else {
            return;
        };
        assert_eq!(
            catalog
                .tools()
                .iter()
                .map(|tool| (
                    tool.provider_name.as_str(),
                    tool.implementation_name.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("project_path_info", "project_path_info"),
                ("toy_path_info", "toy_path_info"),
            ]
        );
    }

    #[test]
    fn native_tool_replacement_replace_builtin_routes_provider_name_to_extension() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(replacement_project_path_info_tool()),
            Ok(())
        );
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_path_info",
        ]);
        let replacement_policy = NativeToolReplacementPolicy::from_rules([replacement_rule(
            NativeToolResolutionMode::ReplaceBuiltin,
            NativeToolReplacementSource::Profile,
        )]);

        let catalog = registry.resolve_provider_turn_catalog_with_replacements(
            &policy,
            ["project_path_info", "toy_path_info"],
            &replacement_policy,
        );

        assert!(catalog.is_ok());
        let Some(catalog) = catalog.ok() else {
            return;
        };
        assert_eq!(catalog.tools().len(), 1);
        assert_eq!(
            catalog.implementation_name_for_provider_tool("project_path_info"),
            Some("toy_path_info")
        );
        let replacement = catalog.resolved_tool("project_path_info");
        assert!(replacement.is_some());
        let Some(replacement) = replacement else {
            return;
        };
        assert_eq!(
            replacement.provenance,
            NativeToolProvenance::ExtensionReplacement {
                extension_id: String::from("example.toy-tools"),
                extension_version: String::from("1.2.3"),
                replaced_builtin: String::from("project_path_info"),
                replacement_source: String::from("profile"),
            }
        );
        assert_eq!(
            catalog.provider_definitions(),
            vec![NativeToolDefinition::project_path_info()]
        );
    }

    #[test]
    fn native_tool_replacement_disable_builtin_removes_it_from_catalog() {
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
        let replacement_policy =
            NativeToolReplacementPolicy::from_rules([NativeToolReplacementRule {
                builtin_name: String::from("project_path_info"),
                extension_id: String::new(),
                extension_tool: String::new(),
                mode: NativeToolResolutionMode::DisableBuiltin,
                source: NativeToolReplacementSource::User,
            }]);

        let catalog = registry.resolve_provider_turn_catalog_with_replacements(
            &policy,
            ["project_path_info"],
            &replacement_policy,
        );

        assert_eq!(
            catalog.map(|catalog| catalog.tools().to_vec()),
            Ok(Vec::new())
        );
    }

    #[test]
    fn native_tool_replacement_deny_omits_extension_alias() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(replacement_project_path_info_tool()),
            Ok(())
        );
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_path_info",
        ]);
        let replacement_policy = NativeToolReplacementPolicy::from_rules([replacement_rule(
            NativeToolResolutionMode::Deny,
            NativeToolReplacementSource::User,
        )]);

        let catalog = registry.resolve_provider_turn_catalog_with_replacements(
            &policy,
            ["project_path_info", "toy_path_info"],
            &replacement_policy,
        );

        assert!(catalog.is_ok());
        let Some(catalog) = catalog.ok() else {
            return;
        };
        assert_eq!(
            catalog
                .tools()
                .iter()
                .map(|tool| tool.provider_name.as_str())
                .collect::<Vec<_>>(),
            vec!["project_path_info"]
        );
    }

    #[test]
    fn native_tool_replacement_cannot_lower_builtin_risk() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(replacement_project_path_info_tool()),
            Ok(())
        );
        let policy =
            NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
                ["project_path_info", "toy_path_info"],
                ["search_project"],
                std::iter::empty::<&str>(),
            );
        let replacement_policy =
            NativeToolReplacementPolicy::from_rules([NativeToolReplacementRule {
                builtin_name: String::from("search_project"),
                extension_id: String::from("example.toy-tools"),
                extension_tool: String::from("toy_path_info"),
                mode: NativeToolResolutionMode::ReplaceBuiltin,
                source: NativeToolReplacementSource::Profile,
            }]);

        let catalog = registry.resolve_provider_turn_catalog_with_replacements(
            &policy,
            ["search_project", "toy_path_info"],
            &replacement_policy,
        );

        assert_eq!(
            catalog,
            Err(NativeToolResolutionError::ReplacementLowersRisk {
                builtin_name: String::from("search_project"),
                builtin_risk: NativeToolRisk::ReadsLocalContent,
                extension_tool: String::from("toy_path_info"),
                extension_risk: NativeToolRisk::ReadsLocalMetadata,
            })
        );
    }

    #[test]
    fn native_tool_replacement_blocks_untrusted_project_policy() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        assert_eq!(
            registry.register_extension_tool(replacement_project_path_info_tool()),
            Ok(())
        );
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
            "project_path_info",
            "toy_path_info",
        ]);
        let replacement_policy = NativeToolReplacementPolicy::from_rules([replacement_rule(
            NativeToolResolutionMode::ReplaceBuiltin,
            NativeToolReplacementSource::Project { trusted: false },
        )]);

        let catalog = registry.resolve_provider_turn_catalog_with_replacements(
            &policy,
            ["project_path_info", "toy_path_info"],
            &replacement_policy,
        );

        assert_eq!(
            catalog,
            Err(NativeToolResolutionError::UntrustedProjectReplacement {
                builtin_name: String::from("project_path_info"),
            })
        );
    }

    #[test]
    fn native_edit_harness_does_not_register_or_advertise_mutation_tools() {
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let definitions = registry.definitions();
        let registered_names = definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();
        let metadata_names = definitions
            .iter()
            .filter(|definition| definition.risk == NativeToolRisk::ReadsLocalMetadata)
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();
        let policy = NativeToolPermissionPolicy::allow_project_metadata_tools(metadata_names);
        let candidates =
            registry.provider_advertising_candidates(&policy, registered_names.iter().copied());

        assert_eq!(
            registered_names,
            vec![
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
            ]
        );
        assert!(
            definitions
                .iter()
                .all(|definition| definition.risk != NativeToolRisk::MutatesLocalState)
        );
        assert_eq!(registry.get("edit"), None);
        assert_eq!(registry.get("write"), None);
        assert_eq!(registry.get("native_edit"), None);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "project_path_info");
    }

    #[test]
    fn native_tool_registry_exposes_provider_content_tools() {
        let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();

        assert_eq!(
            registry
                .get("read_text_file")
                .map(|definition| definition.risk),
            Some(NativeToolRisk::ReadsLocalContent)
        );
        assert_eq!(
            registry
                .get("search_project")
                .map(|definition| definition.risk),
            Some(NativeToolRisk::ReadsLocalContent)
        );
        assert_eq!(
            registry
                .get("list_project_paths")
                .map(|definition| definition.risk),
            Some(NativeToolRisk::ReadsLocalContent)
        );
    }

    #[test]
    fn project_path_info_tool_requires_explicit_metadata_policy() {
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let request = fixture_tool_request(
            "project_path_info",
            serde_json::json!({"path":"Cargo.toml"}),
        );

        let denied = registry.validate_request(&request, &NativeToolPermissionPolicy::deny_all());
        let allowed = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        );

        assert_eq!(denied, Err(NativeToolError::PermissionDenied));
        assert_eq!(
            allowed,
            Ok(super::NativeToolValidation {
                request_id: String::from("tool-request-1"),
                tool_name: String::from("project_path_info"),
                permission: NativeToolPermissionState::Allowed,
            })
        );
    }

    #[test]
    fn project_path_info_tool_executes_metadata_without_file_content() {
        let root_path = temp_resource_dir("native-project-path-info-tool");
        assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let registry = NativeToolRegistry::with_project_read_only_tools();
        let request = fixture_tool_request(
            "project_path_info",
            serde_json::json!({"path":"Cargo.toml"}),
        );
        let validation = registry
            .validate_request(
                &request,
                &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
            )
            .ok();
        assert!(validation.is_some());

        let Some(root) = root else {
            return;
        };
        let executor = ProjectReadOnlyToolExecutor::new(root);
        let result = validation
            .as_ref()
            .map(|validation| executor.execute(&registry, &request, validation));

        assert_eq!(
            result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|result| result.redacted),
            Some(false)
        );
        assert!(
            result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .is_some_and(|result| result.summary.contains("\"relative_path\":\"Cargo.toml\""))
        );
        assert!(
            result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .is_some_and(|result| !result.summary.contains("[package]"))
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn pi_rpc_metadata_identifies_compatibility_runner() {
        let metadata = BackendMetadata::pi_rpc();

        assert_eq!(metadata.kind, BackendKind::PiRpc);
        assert_eq!(metadata.label, "pi rpc");
        assert_eq!(
            metadata.capabilities,
            BackendCapabilities::pi_rpc_compatibility()
        );
        assert!(metadata.capabilities.prompt_streaming);
        assert!(!metadata.capabilities.file_first_sessions);
        assert!(!metadata.capabilities.tool_execution);
    }

    #[test]
    fn native_dogfood_metadata_identifies_file_first_runner() {
        let metadata = BackendMetadata::native_dogfood();

        assert_eq!(metadata.kind, BackendKind::Native);
        assert_eq!(metadata.label, "native dogfood");
        assert_eq!(metadata.capabilities, BackendCapabilities::native_dogfood());
        assert!(metadata.capabilities.prompt_streaming);
        assert!(metadata.capabilities.file_first_sessions);
        assert!(!metadata.capabilities.tool_execution);
    }

    #[test]
    fn metadata_has_debug_and_equality_behavior() {
        let left = BackendMetadata::native_dogfood();
        let right = BackendMetadata::native_dogfood();

        assert_eq!(left, right);
        assert_eq!(format!("{left:?}"), format!("{right:?}"));
    }

    #[test]
    fn backend_channels_connect_ui_sender_to_runner_receiver() {
        let (channels, mut endpoints) = backend_channels();

        assert!(
            channels
                .client_tx
                .send(ClientEvent::RecentSessionsRequested)
                .is_ok()
        );

        assert_eq!(
            endpoints.client_rx.blocking_recv(),
            Some(ClientEvent::RecentSessionsRequested)
        );
    }

    #[test]
    fn connected_announcement_reaches_ui_receiver() {
        let (mut channels, endpoints) = backend_channels();
        let negotiated = negotiated_prompt_streaming();

        assert!(announce_connected(
            &endpoints.backend_tx,
            negotiated.clone()
        ));

        assert_eq!(
            channels.backend_rx.blocking_recv(),
            Some(BackendEvent::Connected { negotiated })
        );
    }

    #[test]
    fn backend_session_carries_metadata_and_announces_connection() {
        let negotiated = negotiated_prompt_streaming();
        let mut session = start_backend_session(BackendMetadata::pi_rpc(), negotiated.clone());

        assert_eq!(session.metadata, BackendMetadata::pi_rpc());
        assert_eq!(
            session.channels.backend_rx.blocking_recv(),
            Some(BackendEvent::Connected { negotiated })
        );
    }

    #[test]
    fn native_session_log_preserves_tool_records_jsonl() {
        let session_id = NativeSessionId(String::from("session-tools"));
        let turn_id = NativeTurnId(String::from("turn-tools"));
        let tool_request_id = NativeToolRequestId(String::from("tool-request-1"));
        let argument_summary = NativeToolPayloadSummary {
            summary: String::from("label=<redacted>"),
            byte_count: 21,
            redacted: true,
            truncated: false,
        };
        let result_summary = NativeToolPayloadSummary {
            summary: String::from("fixture metadata ok"),
            byte_count: 19,
            redacted: false,
            truncated: false,
        };
        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("fixture_echo_metadata"),
            provider_call_id: Some(String::from("provider-call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary,
            argument_content: None,
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id,
            turn_id,
            tool_request_id,
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(result_summary),
            result_content: None,
        });
        let path = temp_log_path("native-session-tool-records");

        assert!(log.write_to_file(&path).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_loads_pre_persistence_tool_events_without_content_fields() {
        let old_request_line = r#"{"type":"tool_request_recorded","session_id":"session-1","turn_id":"turn-1","tool_request_id":"tool-request-1","tool_name":"read_text_file","provider_call_id":"provider-call-1","validation":{"Ok":null},"permission":"allowed","argument_summary":{"summary":"tool payload redacted","byte_count":21,"redacted":true,"truncated":false}}"#;
        let old_finished_line = r#"{"type":"tool_execution_finished","session_id":"session-1","turn_id":"turn-1","tool_request_id":"tool-request-1","outcome":"completed","reason":null,"result_summary":{"summary":"read_text_file result redacted","byte_count":56,"redacted":true,"truncated":false}}"#;

        let request = serde_json::from_str::<NativeSessionEvent>(old_request_line);
        assert!(
            matches!(
                &request,
                Ok(NativeSessionEvent::ToolRequestRecorded {
                    argument_content: None,
                    ..
                })
            ),
            "old tool request line should load with no argument content: {request:?}"
        );
        let finished = serde_json::from_str::<NativeSessionEvent>(old_finished_line);
        assert!(
            matches!(
                &finished,
                Ok(NativeSessionEvent::ToolExecutionFinished {
                    result_content: None,
                    ..
                })
            ),
            "old tool finished line should load with no result content: {finished:?}"
        );
    }

    #[test]
    fn native_session_log_round_trips_tool_events_with_content_fields() {
        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: NativeSessionId(String::from("session-content")),
            turn_id: NativeTurnId(String::from("turn-content")),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("read_text_file"),
            provider_call_id: Some(String::from("provider-call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 21,
                redacted: true,
                truncated: false,
            },
            argument_content: Some(String::from(r#"{"path":"notes.txt"}"#)),
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: NativeSessionId(String::from("session-content")),
            turn_id: NativeTurnId(String::from("turn-content")),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(NativeToolPayloadSummary {
                summary: String::from("read_text_file result redacted"),
                byte_count: 56,
                redacted: true,
                truncated: false,
            }),
            result_content: Some(String::from(
                r#"{"path":"notes.txt","text":"alpha\n","truncated":false}"#,
            )),
        });
        let path = temp_log_path("native-session-tool-content-records");

        assert!(log.write_to_file(&path).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_preserves_tool_validation_failures_without_raw_args() {
        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: NativeSessionId(String::from("session-tools")),
            turn_id: NativeTurnId(String::from("turn-tools")),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("fixture_echo_metadata"),
            provider_call_id: Some(String::from("provider-call-1")),
            validation: Err(NativeToolError::MissingRequiredField {
                field: String::from("label"),
            }),
            permission: NativeToolPermissionState::Denied,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("validation failed before persistence"),
                byte_count: 15,
                redacted: true,
                truncated: false,
            },
            argument_content: None,
        });
        let path = temp_log_path("native-session-tool-validation");

        assert!(log.write_to_file(&path).is_ok());
        let raw = std::fs::read_to_string(&path).ok();
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
        assert!(raw.is_some_and(|raw| !raw.contains("raw_secret_argument")));
    }

    #[test]
    fn native_session_log_starts_empty() {
        let log = NativeSessionLog::default();

        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn completed_exchange_has_stable_parent_links() {
        let log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );

        assert_eq!(log.len(), 3);
        assert_eq!(
            log.events.first(),
            Some(&NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("session-1")),
                entry_id: NativeEntryId(String::from("entry-user")),
                parent_entry_id: None,
                turn_id: NativeTurnId(String::from("turn-1")),
                role: NativeRole::User,
                text: String::from("hello"),
                provider: None,
            })
        );
        assert_eq!(
            log.events.get(1),
            Some(&NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("session-1")),
                entry_id: NativeEntryId(String::from("entry-assistant")),
                parent_entry_id: Some(NativeEntryId(String::from("entry-user"))),
                turn_id: NativeTurnId(String::from("turn-1")),
                role: NativeRole::Assistant,
                text: String::from("hi"),
                provider: None,
            })
        );
        assert_eq!(
            log.events.get(2),
            Some(&NativeSessionEvent::TurnFinished {
                session_id: NativeSessionId(String::from("session-1")),
                turn_id: NativeTurnId(String::from("turn-1")),
                outcome: NativeTurnOutcome::Completed,
                reason: None,
            })
        );
    }

    #[test]
    fn cancelled_or_failed_turns_are_distinct_from_completed_turns() {
        let cancelled = NativeSessionEvent::TurnFinished {
            session_id: NativeSessionId(String::from("session-1")),
            turn_id: NativeTurnId(String::from("turn-1")),
            outcome: NativeTurnOutcome::Cancelled,
            reason: Some(String::from("user cancelled")),
        };
        let failed = NativeSessionEvent::TurnFinished {
            session_id: NativeSessionId(String::from("session-1")),
            turn_id: NativeTurnId(String::from("turn-1")),
            outcome: NativeTurnOutcome::Failed,
            reason: Some(String::from("provider error")),
        };

        assert_ne!(cancelled, failed);
    }

    #[test]
    fn provider_request_keeps_common_shape_provider_free() {
        let request = ProviderRequest {
            turn_id: NativeTurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("openai"),
                model: String::from("gpt-test"),
            },
            messages: vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("hello"),
            }],
            extensions: vec![ProviderExtension {
                key: String::from("temperature"),
                value: serde_json::json!(0.2),
            }],
        };

        assert_eq!(request.messages.len(), 1);
        assert_eq!(
            request
                .extensions
                .first()
                .map(|extension| extension.key.as_str()),
            Some("temperature")
        );
    }

    #[test]
    fn provider_stream_events_preserve_turn_identity() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let event = ProviderStreamEvent::TextDelta {
            turn_id: turn_id.clone(),
            delta: String::from("hello"),
        };

        assert_eq!(event.turn_id(), &turn_id);
    }

    #[test]
    fn plain_streaming_text_fixture_has_ordered_lifecycle_events() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let events = [
            ProviderStreamEvent::Started {
                turn_id: turn_id.clone(),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("text-stream"),
                },
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("hel"),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("lo"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn_id.clone(),
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: Some(ProviderUsage {
                    input_tokens: Some(3),
                    output_tokens: Some(2),
                    total_tokens: Some(5),
                }),
                provider_response_id: Some(String::from("resp_fixture_1")),
            },
        ];

        assert!(events.iter().all(|event| event.turn_id() == &turn_id));
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    ProviderStreamEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "hello"
        );
        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::Completed { .. })
        ));
    }

    #[test]
    fn streamed_tool_call_fixture_preserves_call_id_and_json_arguments() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let tool_call = ProviderToolCall {
            call_id: String::from("call_1"),
            name: String::from("read_file"),
            arguments_json: serde_json::json!({ "path": "Cargo.toml" }),
        };
        let events = [
            ProviderStreamEvent::ToolCallStarted {
                turn_id: turn_id.clone(),
                call_id: String::from("call_1"),
                name: String::from("read_file"),
            },
            ProviderStreamEvent::ToolCallDelta {
                turn_id: turn_id.clone(),
                call_id: String::from("call_1"),
                arguments_delta: String::from("{\"path\":"),
            },
            ProviderStreamEvent::ToolCallDelta {
                turn_id: turn_id.clone(),
                call_id: String::from("call_1"),
                arguments_delta: String::from("\"Cargo.toml\"}"),
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id,
                tool_call: tool_call.clone(),
            },
        ];

        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::ToolCallCompleted { tool_call: completed, .. })
                if completed == &tool_call
        ));
    }

    #[test]
    fn provider_stream_error_fixtures_cover_normalized_categories() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let fixtures = [
            (ProviderErrorKind::Authentication, "auth failed"),
            (ProviderErrorKind::RateLimited, "rate limited"),
            (ProviderErrorKind::InvalidRequest, "invalid request"),
            (ProviderErrorKind::ContextLength, "context length"),
            (ProviderErrorKind::UnavailableModel, "model unavailable"),
            (ProviderErrorKind::SafetyRefusal, "safety refusal"),
            (ProviderErrorKind::MalformedStream, "malformed stream"),
            (ProviderErrorKind::Backpressure, "backpressure"),
        ];

        let events = fixtures.map(|(kind, message)| ProviderStreamEvent::Failed {
            turn_id: turn_id.clone(),
            error: ProviderError {
                kind,
                message: String::from(message),
                redacted_debug: Some(String::from("authorization=<redacted>")),
            },
        });

        assert!(events.iter().all(|event| event.turn_id() == &turn_id));
        assert!(events.iter().all(|event| matches!(event, ProviderStreamEvent::Failed { error, .. } if error.redacted_debug.as_deref() == Some("authorization=<redacted>"))));
    }

    #[test]
    fn cancellation_fixture_does_not_mark_turn_completed() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let event = ProviderStreamEvent::Cancelled {
            turn_id: turn_id.clone(),
            reason: Some(String::from("ui dropped receiver")),
        };

        assert_eq!(event.turn_id(), &turn_id);
        assert!(!matches!(event, ProviderStreamEvent::Completed { .. }));
    }

    #[test]
    fn bounded_provider_stream_buffer_coalesces_text_when_full() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut buffer = BoundedProviderStreamBuffer::new(1);

        assert!(
            buffer
                .push(ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("hel"),
                })
                .is_ok()
        );
        assert!(
            buffer
                .push(ProviderStreamEvent::TextDelta {
                    turn_id,
                    delta: String::from("lo"),
                })
                .is_ok()
        );

        assert_eq!(buffer.len(), 1);
        assert!(matches!(
            buffer.pop_front(),
            Some(ProviderStreamEvent::TextDelta { delta, .. }) if delta == "hello"
        ));
    }

    #[test]
    fn bounded_provider_stream_buffer_preserves_lifecycle_by_dropping_text() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut buffer = BoundedProviderStreamBuffer::new(2);

        assert!(
            buffer
                .push(ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("text-stream"),
                    },
                })
                .is_ok()
        );
        assert!(
            buffer
                .push(ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("drop me if needed"),
                })
                .is_ok()
        );
        assert!(
            buffer
                .push(ProviderStreamEvent::Completed {
                    turn_id,
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: None,
                })
                .is_ok()
        );

        assert_eq!(buffer.len(), 2);
        assert!(matches!(
            buffer.pop_front(),
            Some(ProviderStreamEvent::Started { .. })
        ));
        assert!(matches!(
            buffer.pop_front(),
            Some(ProviderStreamEvent::Completed { .. })
        ));
    }

    #[test]
    fn bounded_provider_stream_buffer_returns_backpressure_error_when_full() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut buffer = BoundedProviderStreamBuffer::new(1);

        assert!(
            buffer
                .push(ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("text-stream"),
                    },
                })
                .is_ok()
        );
        let result = buffer.push(ProviderStreamEvent::ToolCallStarted {
            turn_id,
            call_id: String::from("call-1"),
            name: String::from("read_file"),
        });

        assert!(matches!(
            result,
            Err(ProviderStreamEvent::Failed { error, .. })
                if error.message == "Native backend fell behind this stream."
        ));
    }

    #[test]
    fn rig_adapter_maps_text_and_final_stream_choices() {
        let turn_id = NativeTurnId(String::from("turn-1"));

        let text = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::Message(String::from("hello")),
        );
        let final_event =
            rig_adapter::map_raw_streaming_choice(&turn_id, RawStreamingChoice::FinalResponse(()));

        assert!(matches!(
            text,
            Some(ProviderStreamEvent::TextDelta { delta, .. }) if delta == "hello"
        ));
        assert!(matches!(
            final_event,
            Some(ProviderStreamEvent::Completed {
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
                ..
            })
        ));
    }

    #[test]
    fn rig_adapter_preserves_tool_call_identity_and_arguments() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let tool_call = RawStreamingToolCall::new(
            String::from("provider-call-1"),
            String::from("read_file"),
            serde_json::json!({ "path": "Cargo.toml" }),
        )
        .with_call_id(String::from("call-1"));

        let event = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCall(tool_call),
        );

        assert!(matches!(
            event,
            Some(ProviderStreamEvent::ToolCallCompleted { tool_call, .. })
                if tool_call.call_id == "call-1"
                    && tool_call.name == "read_file"
                    && tool_call.arguments_json == serde_json::json!({ "path": "Cargo.toml" })
        ));
    }

    #[test]
    fn rig_adapter_maps_tool_call_deltas_without_tool_execution() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let started = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-1"),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Name(String::from("read_file")),
            },
        );
        let delta = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-1"),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Delta(String::from("{\"path\":")),
            },
        );

        assert!(matches!(
            started,
            Some(ProviderStreamEvent::ToolCallStarted { call_id, name, .. })
                if call_id == "call-1" && name == "read_file"
        ));
        assert!(matches!(
            delta,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, arguments_delta, .. })
                if call_id == "call-1" && arguments_delta == "{\"path\":"
        ));
    }

    #[test]
    fn rig_adapter_accumulates_message_id_into_completion_metadata() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut mapper = rig_adapter::RigStreamMapper::new(turn_id);

        let message_id =
            mapper.map_choice::<()>(RawStreamingChoice::MessageId(String::from("msg_1")));
        let completed = mapper.map_choice(RawStreamingChoice::FinalResponse(()));

        assert!(message_id.is_none());
        assert_eq!(mapper.provider_response_id(), Some("msg_1"));
        assert!(matches!(
            completed,
            Some(ProviderStreamEvent::Completed {
                provider_response_id: Some(id),
                usage: None,
                ..
            }) if id == "msg_1"
        ));
    }

    #[test]
    fn rig_adapter_preserves_parallel_tool_call_ids() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let first = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-1"),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Delta(String::from("{\"path\":")),
            },
        );
        let second = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-2"),
                internal_call_id: String::from("rig-internal-2"),
                content: ToolCallDeltaContent::Delta(String::from("{\"cmd\":")),
            },
        );

        assert!(matches!(
            first,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, .. }) if call_id == "call-1"
        ));
        assert!(matches!(
            second,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, .. }) if call_id == "call-2"
        ));
    }

    #[test]
    fn rig_adapter_uses_internal_tool_call_id_when_provider_id_is_missing() {
        let turn_id = NativeTurnId(String::from("turn-1"));

        let event = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::new(),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Delta(String::from("{}")),
            },
        );

        assert!(matches!(
            event,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, .. }) if call_id == "rig-internal-1"
        ));
    }

    #[test]
    fn rig_adapter_maps_cancellation_without_completion() {
        let turn_id = NativeTurnId(String::from("turn-1"));

        let event = rig_adapter::map_cancelled(turn_id, "stream aborted");

        assert!(matches!(
            event,
            ProviderStreamEvent::Cancelled { reason: Some(ref reason), .. } if reason == "stream aborted"
        ));
        assert!(!matches!(event, ProviderStreamEvent::Completed { .. }));
    }

    #[test]
    fn provider_errors_carry_normalized_redacted_debug_details() {
        let error = ProviderError {
            kind: ProviderErrorKind::RateLimited,
            message: String::from("Provider limit reached. Try later or switch model."),
            redacted_debug: Some(String::from("status=429 authorization=<redacted>")),
        };

        assert_eq!(error.kind, ProviderErrorKind::RateLimited);
        assert!(!error.redacted_debug.unwrap_or_default().contains("sk-"));
    }

    #[test]
    fn rig_provider_error_classification_covers_dogfood_failures() {
        assert_eq!(
            rig_adapter::classify_provider_error_debug("401 unauthorized invalid api key"),
            ProviderErrorKind::Authentication
        );
        assert_eq!(
            rig_adapter::classify_provider_error_debug("not_found_error model: yach-bad-model"),
            ProviderErrorKind::UnavailableModel
        );
        assert_eq!(
            rig_adapter::classify_provider_error_debug("request timed out while streaming"),
            ProviderErrorKind::Timeout
        );
        assert_eq!(
            rig_adapter::classify_provider_error_debug("network connect error"),
            ProviderErrorKind::Network
        );
    }

    #[test]
    fn rig_secret_redaction_handles_common_key_shapes() {
        let redacted = rig_adapter::redact_secrets(
            "authorization=Bearer sk-test api-key=sk-other apikey=sk-third harmless",
        );

        assert!(!redacted.contains("sk-test"));
        assert!(!redacted.contains("sk-other"));
        assert!(!redacted.contains("sk-third"));
        assert!(redacted.contains("harmless"));
    }

    #[test]
    fn rig_adapter_schema_tool_definition_is_not_executable_rig_tool() {
        let extension = build_project_path_info_provider_tool_advertising_extension();
        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        let request = ProviderRequest {
            turn_id: NativeTurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("fixture-provider"),
                model: String::from("fixture-model"),
            },
            messages: vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("inspect cargo"),
            }],
            extensions: vec![extension],
        };

        let tools = rig_adapter::rig_tool_definitions_from_request(&request);
        assert!(tools.is_ok());
        let Some(tools) = tools.ok() else {
            return;
        };

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "project_path_info");
    }

    #[test]
    fn fixture_error_constructors_cover_native_dogfood_failures() {
        let fixture_failure = ProviderError::fixture_failure();
        let malformed = ProviderError::malformed_stream("fixture stream ended mid-event");
        let backpressure = ProviderError::backpressure();
        let cancelled = ProviderError::cancelled("native dogfood fixture cancellation");

        assert_eq!(fixture_failure.kind, ProviderErrorKind::ProviderInternal);
        assert_eq!(malformed.kind, ProviderErrorKind::MalformedStream);
        assert_eq!(backpressure.kind, ProviderErrorKind::Backpressure);
        assert_eq!(cancelled.kind, ProviderErrorKind::Cancelled);
        assert!(cancelled.redacted_debug.is_none());
    }

    #[test]
    fn native_session_log_writes_and_reloads_jsonl() {
        let path = temp_log_path("native-session-log");
        let log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );

        assert!(log.write_to_file(&path).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_preserves_provider_metadata_jsonl() {
        let path = temp_log_path("native-session-log-provider");
        let mut log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );
        if let Some(NativeSessionEvent::EntryAppended { provider, .. }) = log.events.get_mut(1) {
            *provider = Some(ProviderMetadata {
                provider: String::from("chatgpt-subscription"),
                model: String::from("gpt-5.3-codex-spark"),
                response_id: None,
            });
        }

        assert!(log.write_to_file(&path).is_ok());
        let persisted = std::fs::read_to_string(&path).unwrap_or_default();
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert!(persisted.contains("chatgpt-subscription"));
        assert!(persisted.contains("gpt-5.3-codex-spark"));
        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_preserves_static_context_evidence_without_content_body() {
        let root_path = temp_resource_dir("native-session-log-static-context");
        let agents_path = root_path.join("AGENTS.md");
        let full_body = "root static context body should stay out of the session log";
        assert!(std::fs::write(&agents_path, full_body).is_ok());

        let assembly = assemble_project_static_context(
            &root_path,
            &root_path,
            NativeStaticContextPolicy::test(),
        );
        let mut log = NativeSessionLog::default();
        log.record_static_context_included(
            NativeSessionId(String::from("session-static-context")),
            NativeTurnId(String::from("turn-static-context")),
            assembly.bundle.summary(),
            assembly.omissions,
        );
        let path = temp_log_path("native-session-static-context");

        assert!(log.write_to_file(&path).is_ok());
        let raw = std::fs::read_to_string(&path).ok();
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());
        assert!(std::fs::remove_dir_all(root_path).is_ok());

        match raw.as_deref() {
            Some(raw) => {
                assert!(raw.contains("static_context_included"));
                assert!(raw.contains("AGENTS.md"));
                assert!(!raw.contains(full_body));
            }
            None => assert!(raw.is_some()),
        }
        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_preserves_edit_transaction_evidence_jsonl() {
        let log_path = temp_resource_dir("native-edit-evidence-jsonl").join("session.jsonl");
        let mut log = NativeSessionLog::default();
        let summary = NativeEditEvidenceSummary {
            operation_count: 1,
            operations: vec![NativeEditOperationEvidence::ModifyTextFile {
                relative_path: String::from("src/lib.rs"),
                before_sha256: String::from("before"),
                after_sha256: String::from("after"),
                before_bytes: 12,
                after_bytes: 13,
                hunk_count: 1,
                bytes_written: None,
            }],
            diff_summary: NativeToolPayloadSummary {
                summary: String::from("--- src/lib.rs\n+++ src/lib.rs\n-red\n+green\n"),
                byte_count: 43,
                redacted: false,
                truncated: false,
            },
        };

        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id: NativeSessionId(String::from("session-edit")),
            turn_id: NativeTurnId(String::from("turn-7")),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
            transaction_id: NativeEditTransactionId(String::from("edit-7")),
            summary: summary.clone(),
        });
        let finished_summary = NativeEditEvidenceSummary {
            operation_count: 1,
            operations: vec![NativeEditOperationEvidence::ModifyTextFile {
                relative_path: String::from("src/lib.rs"),
                before_sha256: String::from("before"),
                after_sha256: String::from("after"),
                before_bytes: 12,
                after_bytes: 13,
                hunk_count: 1,
                bytes_written: Some(13),
            }],
            diff_summary: summary.diff_summary.clone(),
        };

        log.push(NativeSessionEvent::EditTransactionFinished {
            session_id: NativeSessionId(String::from("session-edit")),
            turn_id: NativeTurnId(String::from("turn-7")),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
            transaction_id: Some(NativeEditTransactionId(String::from("edit-7"))),
            outcome: NativeEditEvidenceOutcome::Completed,
            reason: None,
            summary: Some(finished_summary),
        });

        assert!(log.write_to_file(&log_path).is_ok());
        let raw = std::fs::read_to_string(&log_path).ok();
        let loaded = NativeSessionLog::load_from_file(&log_path);

        match raw.as_deref() {
            Some(raw) => {
                assert!(raw.contains("edit_transaction_prepared"));
                assert!(raw.contains("edit_transaction_finished"));
                assert!(raw.contains("modify_text_file"));
                assert!(raw.contains("\"outcome\":\"completed\""));
            }
            None => assert!(raw.is_some()),
        }
        assert_eq!(loaded.ok(), Some(log));
        let Some(parent) = log_path.parent() else {
            assert!(log_path.parent().is_some());
            return;
        };
        assert!(std::fs::remove_dir_all(parent).is_ok());
    }

    #[test]
    fn native_session_log_preserves_edit_trace_records_jsonl() {
        let path = temp_resource_dir("native-edit-trace-jsonl").join("session.jsonl");
        let mut log = NativeSessionLog::default();
        log.record_edit_trace(
            NativeSessionId(String::from("default")),
            NativeTurnId(String::from("turn-7")),
            NativeEditTraceRecord {
                trace_id: NativeEditTraceId(String::from("edit-trace-1")),
                phase: NativeEditTracePhase::Preview,
                source: NativeEditTraceSource::ProviderTool,
                tool_name: Some(String::from("edit_text_file")),
                tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
                provider_call_id: Some(String::from("call-edit-1")),
                preview_id: Some(super::NativeEditPreviewId(String::from("edit-preview-1"))),
                permission_decision_id: Some(super::NativePermissionDecisionId(String::from(
                    "permission-decision-1",
                ))),
                transaction_id: Some(NativeEditTransactionId(String::from("edit-1"))),
                outcome: NativeEditTraceOutcome::Completed,
                duration_ms: 3,
                reason_label: None,
                attributes: vec![NativeMetricAttribute {
                    key: String::from("operation"),
                    value: String::from("edit_text_file"),
                }],
            },
        );

        assert!(log.write_to_file(&path).is_ok());
        let raw = std::fs::read_to_string(&path).ok();
        let loaded = NativeSessionLog::load_from_file(&path).ok();

        assert!(raw.as_deref().is_some_and(|raw| {
            raw.contains("edit_trace_recorded")
                && raw.contains("\"phase\":\"preview\"")
                && raw.contains("\"trace_id\":\"edit-trace-1\"")
        }));
        assert_eq!(
            loaded.as_ref().map(NativeSessionLog::next_turn_index),
            Some(8)
        );
        assert_eq!(loaded, Some(log));
        if let Some(parent) = path.parent() {
            assert!(std::fs::remove_dir_all(parent).is_ok());
        }
    }

    #[test]
    fn native_session_permission_evidence_is_not_provider_transcript() {
        let mut log = completed_text_exchange(
            NativeSessionId(String::from("default")),
            NativeEntryId(String::from("entry-1-user")),
            NativeEntryId(String::from("entry-1-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("world"),
        );
        let request = NativePermissionRequest {
            request_id: String::from("perm-1"),
            actor: NativePermissionActor::UserLocalUi,
            capability: NativePermissionCapability::EditTransaction,
            target: NativePermissionTargetSummary {
                operation: String::from("modify_text_file"),
                resource: String::from("src/lib.rs"),
            },
            risk: NativePermissionRisk::WorkspaceWrite,
            requested_reviewer: None,
        };
        let decision = NativePermissionDecisionEngine::decide(
            &request,
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
        );

        log.record_permission_decision(
            NativeSessionId(String::from("default")),
            NativeTurnId(String::from("turn-1")),
            decision.summary(&request, false),
        );

        let messages = log.transcript_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "hello");
        assert_eq!(messages[1].text, "world");
    }

    #[test]
    fn native_session_edit_trace_is_not_provider_transcript() {
        let mut log = completed_text_exchange(
            NativeSessionId(String::from("default")),
            NativeEntryId(String::from("entry-1-user")),
            NativeEntryId(String::from("entry-1-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("world"),
        );
        log.record_edit_trace(
            NativeSessionId(String::from("default")),
            NativeTurnId(String::from("turn-1")),
            NativeEditTraceRecord::test_record(
                NativeEditTraceId(String::from("edit-trace-1")),
                NativeEditTracePhase::Preview,
            ),
        );

        let messages = log.transcript_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "hello");
        assert_eq!(messages[1].text, "world");
    }

    #[test]
    fn native_session_permission_evidence_round_trips_jsonl() {
        let path =
            temp_resource_dir("native-session-permission-evidence-jsonl").join("session.jsonl");
        let mut log = NativeSessionLog::default();
        let request = NativePermissionRequest {
            request_id: String::from("perm-1"),
            actor: NativePermissionActor::UserLocalUi,
            capability: NativePermissionCapability::EditTransaction,
            target: NativePermissionTargetSummary {
                operation: String::from("create_text_file"),
                resource: String::from("notes.txt"),
            },
            risk: NativePermissionRisk::WorkspaceWrite,
            requested_reviewer: None,
        };
        let decision = NativePermissionDecisionEngine::decide(
            &request,
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Ask),
        );
        log.record_permission_decision(
            NativeSessionId(String::from("default")),
            NativeTurnId(String::from("turn-7")),
            decision.summary(&request, false),
        );

        assert!(log.write_to_file(&path).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();

        assert_eq!(
            loaded.as_ref().map(|loaded| &loaded.events),
            Some(&log.events)
        );
        assert_eq!(
            loaded.as_ref().map(NativeSessionLog::next_turn_index),
            Some(8)
        );
        assert!(loaded.as_ref().is_some_and(|loaded| matches!(
            loaded.events.as_slice(),
            [NativeSessionEvent::PermissionDecisionRecorded { summary, .. }]
                if summary.outcome == NativePermissionDecisionOutcome::NeedsUserReview
                    && summary.reason == "permission_mode_ask"
                    && summary.target.resource == "notes.txt"
        )));
        if let Some(parent) = path.parent() {
            assert!(std::fs::remove_dir_all(parent).is_ok());
        } else {
            assert!(path.parent().is_some());
        }
    }

    #[test]
    fn native_edit_harness_summarizes_preview_without_file_bodies() {
        let root_path = temp_resource_dir("native-edit-harness-preview-summary");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        let Some(root) = root else {
            return;
        };

        let preview = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("secret body\n"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert!(preview.is_ok());
        let summary = preview
            .as_ref()
            .map(native_edit_prepared_evidence_summary)
            .ok();

        assert!(matches!(
            summary.as_ref().map(|summary| summary.operations.as_slice()),
            Some([NativeEditOperationEvidence::CreateTextFile {
                relative_path,
                after_bytes: 12,
                bytes_written: None,
                ..
            }]) if relative_path == "src/new.rs"
        ));
        assert!(
            summary
                .as_ref()
                .is_some_and(|summary| !summary.diff_summary.summary.contains("secret body"))
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_edit_access_records_tool_request_id_on_prepare_apply() {
        let root_guard = temp_native_edit_root("edit-access-tool-request-id");
        root_guard.write("notes.txt", "alpha\n");
        let resource_root = NativeResourceRoot::project(root_guard.root()).ok();
        let Some(resource_root) = resource_root else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let context = NativeEditAccessContext {
            session_id: NativeSessionId(String::from("session-1")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
            edit_policy: NativeEditPolicy::test(),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
        };

        let preview = access.prepare(
            &resource_root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("notes.txt"),
                    expected_sha256: sha256_hex_for_test("alpha\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("alpha"),
                        replace: String::from("beta"),
                    }],
                }],
            },
            context,
            &mut log,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };
        let applied = access.apply(
            &preview.preview_id,
            &preview.permission_decision_id,
            &mut log,
        );
        assert!(applied.is_ok());

        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::EditTransactionPrepared {
                tool_request_id: Some(NativeToolRequestId(id)),
                ..
            } if id == "tool-request-1"
        )));
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::EditTransactionFinished {
                tool_request_id: Some(NativeToolRequestId(id)),
                outcome: NativeEditEvidenceOutcome::Completed,
                ..
            } if id == "tool-request-1"
        )));
    }

    #[test]
    fn native_edit_harness_records_prepare_and_complete_events() {
        let root_path = temp_resource_dir("native-edit-harness-success");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        let Some(root) = root else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let context = NativeEditHarnessContext {
            session_id: NativeSessionId(String::from("session-edit")),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_request_id: None,
        };

        let result = NativeEditHarness::preview_and_apply(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("created\n"),
                }],
            },
            NativeEditPolicy::test(),
            &mut log,
            context,
        );

        assert!(result.is_ok());
        assert_eq!(
            std::fs::read_to_string(root_path.join("src/new.rs")).ok(),
            Some(String::from("created\n"))
        );
        assert_eq!(log.events.len(), 2);

        let prepared_transaction_id = match &log.events[0] {
            NativeSessionEvent::EditTransactionPrepared {
                session_id,
                turn_id,
                tool_request_id,
                transaction_id,
                summary,
            } => {
                assert_eq!(session_id.0, "session-edit");
                assert_eq!(turn_id.0, "turn-1");
                assert_eq!(tool_request_id, &None);
                assert_eq!(summary.operation_count, 1);
                transaction_id.clone()
            }
            event => {
                assert!(
                    matches!(event, NativeSessionEvent::EditTransactionPrepared { .. }),
                    "expected prepared event, got {event:?}"
                );
                return;
            }
        };

        match &log.events[1] {
            NativeSessionEvent::EditTransactionFinished {
                session_id,
                turn_id,
                tool_request_id,
                transaction_id,
                outcome,
                reason,
                summary,
            } => {
                assert_eq!(session_id.0, "session-edit");
                assert_eq!(turn_id.0, "turn-1");
                assert_eq!(tool_request_id, &None);
                assert_eq!(transaction_id, &Some(prepared_transaction_id));
                assert_eq!(outcome, &NativeEditEvidenceOutcome::Completed);
                assert_eq!(reason, &None);
                assert!(matches!(
                    summary.as_ref().map(|summary| summary.operations.as_slice()),
                    Some([NativeEditOperationEvidence::CreateTextFile {
                        relative_path,
                        after_bytes: 8,
                        bytes_written: Some(8),
                        ..
                    }]) if relative_path == "src/new.rs"
                ));
            }
            event => {
                assert!(
                    matches!(event, NativeSessionEvent::EditTransactionFinished { .. }),
                    "expected finished event, got {event:?}"
                );
                return;
            }
        }

        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_edit_harness_records_validation_failure_without_raw_payload() {
        let root_path = temp_resource_dir("native-edit-harness-validation-failure");
        let root = NativeResourceRoot::project(&root_path).ok();
        let Some(root) = root else {
            return;
        };
        let mut log = NativeSessionLog::default();

        let result = NativeEditHarness::preview_and_apply(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("../outside.rs"),
                    content: String::from("secret payload\n"),
                }],
            },
            NativeEditPolicy::test(),
            &mut log,
            NativeEditHarnessContext {
                session_id: NativeSessionId(String::from("session-edit")),
                turn_id: NativeTurnId(String::from("turn-1")),
                tool_request_id: None,
            },
        );

        assert!(matches!(result, Err(NativeEditError::PathTraversal { .. })));
        assert_eq!(log.events.len(), 1);
        assert!(matches!(
            &log.events[0],
            NativeSessionEvent::EditTransactionFinished {
                transaction_id: None,
                outcome: NativeEditEvidenceOutcome::ValidationFailed,
                reason: Some(reason),
                summary: None,
                ..
            } if reason == "path_traversal"
        ));
        let serialized = serde_json::to_string(&log.events).ok();
        assert!(
            serialized
                .as_ref()
                .is_some_and(|events| !events.contains("secret payload"))
        );

        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_edit_harness_records_apply_failure_after_prepare() {
        let root_path = temp_resource_dir("native-edit-harness-apply-failure");
        assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        let Some(root) = root else {
            return;
        };
        let mut log = NativeSessionLog::default();
        let preview_policy = NativeEditPolicy::test();
        let apply_policy = NativeEditPolicy {
            allow_create: false,
            ..NativeEditPolicy::test()
        };
        let tool_request_id = NativeToolRequestId(String::from("tool-request-local-edit"));

        let result = NativeEditHarness::preview_and_apply_with_apply_policy(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("created\n"),
                }],
            },
            preview_policy,
            apply_policy,
            &mut log,
            NativeEditHarnessContext {
                session_id: NativeSessionId(String::from("session-edit")),
                turn_id: NativeTurnId(String::from("turn-1")),
                tool_request_id: Some(tool_request_id.clone()),
            },
        );

        assert!(matches!(result, Err(NativeEditError::CreateDisabled)));
        assert!(!root_path.join("src/new.rs").exists());
        assert_eq!(log.events.len(), 2);

        let prepared_transaction_id = match &log.events[0] {
            NativeSessionEvent::EditTransactionPrepared {
                tool_request_id: event_tool_request_id,
                transaction_id,
                summary,
                ..
            } => {
                assert_eq!(event_tool_request_id, &Some(tool_request_id.clone()));
                assert_eq!(summary.operation_count, 1);
                transaction_id.clone()
            }
            event => {
                assert!(
                    matches!(event, NativeSessionEvent::EditTransactionPrepared { .. }),
                    "expected prepared event, got {event:?}"
                );
                return;
            }
        };

        match &log.events[1] {
            NativeSessionEvent::EditTransactionFinished {
                tool_request_id: event_tool_request_id,
                transaction_id,
                outcome,
                reason,
                summary,
                ..
            } => {
                assert_eq!(event_tool_request_id, &Some(tool_request_id));
                assert_eq!(transaction_id, &Some(prepared_transaction_id));
                assert_eq!(outcome, &NativeEditEvidenceOutcome::Failed);
                assert_eq!(reason.as_deref(), Some("create_disabled"));
                assert_eq!(
                    summary.as_ref().map(|summary| summary.operation_count),
                    Some(1)
                );
            }
            event => {
                assert!(
                    matches!(event, NativeSessionEvent::EditTransactionFinished { .. }),
                    "expected failed event, got {event:?}"
                );
                return;
            }
        }

        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_edit_error_labels_are_categorical() {
        assert_eq!(
            native_edit_error_label(&NativeEditError::TargetExists {
                path: String::from("src/lib.rs")
            }),
            "target_exists"
        );
        assert_eq!(
            native_edit_error_label(&NativeEditError::HashMismatch {
                path: String::from("src/lib.rs"),
                expected_sha256: String::from("expected"),
                actual_sha256: String::from("actual"),
            }),
            "hash_mismatch"
        );
    }

    #[test]
    fn native_session_log_ignores_blank_jsonl_lines() {
        let path = temp_log_path("native-session-log-blanks");
        let log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );
        let lines = log
            .events
            .iter()
            .filter_map(|event| serde_json::to_string(event).ok())
            .collect::<Vec<_>>()
            .join("\n\n");

        assert!(std::fs::write(&path, format!("\n{lines}\n\n")).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    fn fixture_provider_continuation_request(
        tool_results: Vec<NativeProviderToolResult>,
    ) -> ProviderContinuationRequest {
        ProviderContinuationRequest {
            turn_id: NativeTurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("fixture-provider"),
                model: String::from("fixture-model"),
            },
            prior_messages: vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("use a tool"),
            }],
            tool_results,
            extensions: vec![ProviderExtension {
                key: String::from("fixture"),
                value: serde_json::json!(true),
            }],
        }
    }

    fn fixture_provider_tool_result(
        tool_request_id: &str,
        provider_call_id: Option<&str>,
        content: &str,
    ) -> NativeProviderToolResult {
        NativeProviderToolResult {
            tool_request_id: String::from(tool_request_id),
            provider_call_id: provider_call_id.map(String::from),
            status: NativeToolOutcome::Completed,
            content: String::from(content),
            byte_count: content.len(),
            redacted: true,
            truncated: false,
            reason: None,
        }
    }

    fn fixture_continuation_context() -> NativeToolContinuationContext {
        NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("session-1")),
            turn_id: NativeTurnId(String::from("turn-1")),
        }
    }

    fn fixture_tool_request(
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> PendingNativeToolRequest {
        PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from(tool_name),
            provider_call_id: Some(String::from("provider-call-1")),
            arguments,
        }
    }

    fn events_are_ordered_before_completed_apply(events: &[NativeSessionEvent]) -> bool {
        let tool_request = events
            .iter()
            .position(|event| matches!(event, NativeSessionEvent::ToolRequestRecorded { .. }));
        let permission = events.iter().position(|event| {
            matches!(event, NativeSessionEvent::PermissionDecisionRecorded { .. })
        });
        let prepared = events
            .iter()
            .position(|event| matches!(event, NativeSessionEvent::EditTransactionPrepared { .. }));
        let apply_started = events.iter().position(|event| {
            matches!(
                event,
                NativeSessionEvent::EditTransactionFinished {
                    outcome: NativeEditEvidenceOutcome::ApplyStarted,
                    ..
                }
            )
        });
        let completed = events.iter().position(|event| {
            matches!(
                event,
                NativeSessionEvent::EditTransactionFinished {
                    outcome: NativeEditEvidenceOutcome::Completed,
                    ..
                }
            )
        });
        matches!(
            (tool_request, permission, prepared, apply_started, completed),
            (
                Some(tool_request_index),
                Some(permission_index),
                Some(prepared_index),
                Some(apply_started_index),
                Some(completed_index),
            ) if tool_request_index < permission_index
                && permission_index < prepared_index
                && prepared_index < apply_started_index
                && apply_started_index < completed_index
        )
    }

    fn provider_tool_call(
        call_id: &str,
        name: &str,
        arguments_json: serde_json::Value,
    ) -> ProviderToolCall {
        ProviderToolCall {
            call_id: String::from(call_id),
            name: String::from(name),
            arguments_json,
        }
    }

    fn negotiated_prompt_streaming() -> NegotiatedCapabilities {
        let ui = Handshake::new("ui", vec![Capability::PromptStreaming]);
        let backend = Handshake::new("backend", vec![Capability::PromptStreaming]);
        NegotiatedCapabilities::from_handshakes(&ui, &backend)
    }

    fn temp_resource_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("{name}-{unique}"));
        assert!(std::fs::create_dir_all(&path).is_ok());
        path
    }

    struct TempNativeEditRoot {
        path: PathBuf,
    }

    impl TempNativeEditRoot {
        fn root(&self) -> &Path {
            &self.path
        }

        fn write(&self, relative_path: &str, content: &str) {
            let path = self.path.join(relative_path);
            if let Some(parent) = path.parent() {
                assert!(std::fs::create_dir_all(parent).is_ok());
            }
            assert!(std::fs::write(path, content).is_ok());
        }
    }

    impl Drop for TempNativeEditRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_native_edit_root(name: &str) -> TempNativeEditRoot {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path =
            std::env::temp_dir().join(format!("yach-{name}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        assert!(std::fs::create_dir_all(&path).is_ok());
        TempNativeEditRoot { path }
    }

    fn temp_log_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("{name}-{unique}.jsonl"))
    }
}

#[cfg(test)]
#[test]
fn native_session_resume_projection_derives_next_ids_and_transcript() {
    let session_id = NativeSessionId(String::from("session-resume"));
    let mut log = NativeSessionLog::default();

    log.push(NativeSessionEvent::EntryAppended {
        session_id: session_id.clone(),
        entry_id: NativeEntryId(String::from("entry-0")),
        parent_entry_id: None,
        turn_id: NativeTurnId(String::from("turn-0")),
        role: NativeRole::User,
        text: String::from("first"),
        provider: None,
    });
    log.push(NativeSessionEvent::ToolRequestRecorded {
        session_id: session_id.clone(),
        turn_id: NativeTurnId(String::from("turn-2")),
        tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
        tool_name: String::from("fixture_echo_metadata"),
        provider_call_id: Some(String::from("provider-call-1")),
        validation: Ok(()),
        permission: NativeToolPermissionState::Allowed,
        argument_summary: NativeToolPayloadSummary {
            summary: String::from("label=<redacted>"),
            byte_count: 21,
            redacted: true,
            truncated: false,
        },
        argument_content: None,
    });
    log.push(NativeSessionEvent::ToolExecutionFinished {
        session_id: session_id.clone(),
        turn_id: NativeTurnId(String::from("turn-4")),
        tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
        outcome: NativeToolOutcome::Completed,
        reason: None,
        result_summary: None,
        result_content: None,
    });
    log.push(NativeSessionEvent::TurnFinished {
        session_id: session_id.clone(),
        turn_id: NativeTurnId(String::from("turn-6")),
        outcome: NativeTurnOutcome::Completed,
        reason: None,
    });
    log.record_duration_metric(
        session_id.clone(),
        Some(NativeTurnId(String::from("turn-8"))),
        "native_prompt_total",
        std::time::Duration::from_millis(42),
        vec![NativeMetricAttribute {
            key: String::from("source"),
            value: String::from("test"),
        }],
    );
    log.push(NativeSessionEvent::EntryAppended {
        session_id,
        entry_id: NativeEntryId(String::from("entry-1")),
        parent_entry_id: Some(NativeEntryId(String::from("entry-0"))),
        turn_id: NativeTurnId(String::from("not-a-numeric-turn")),
        role: NativeRole::Assistant,
        text: String::from("second"),
        provider: None,
    });

    assert_eq!(log.next_turn_index(), 9);
    assert_eq!(
        log.last_entry_id(),
        Some(NativeEntryId(String::from("entry-1")))
    );
    assert_eq!(
        log.transcript_messages(),
        vec![
            NativeTranscriptMessage {
                role: NativeRole::User,
                text: String::from("first"),
            },
            NativeTranscriptMessage {
                role: NativeRole::Assistant,
                text: String::from("second"),
            },
        ]
    );
    assert_eq!(NativeSessionLog::default().next_turn_index(), 0);
}

#[cfg(test)]
#[test]
fn native_session_log_preserves_metric_records_jsonl() {
    let path = temp_native_session_log_path("native-session-metric-records");
    let session_id = NativeSessionId(String::from("session-metrics"));
    let mut log = NativeSessionLog::default();

    log.record_duration_metric(
        session_id.clone(),
        None,
        "session_log_load",
        std::time::Duration::from_millis(7),
        vec![NativeMetricAttribute {
            key: String::from("status"),
            value: String::from("ok"),
        }],
    );
    log.record_duration_metric(
        session_id.clone(),
        Some(NativeTurnId(String::from("turn-3"))),
        "native_prompt_total",
        std::time::Duration::from_millis(12),
        vec![],
    );

    assert!(log.write_to_file(&path).is_ok());
    let raw = std::fs::read_to_string(&path).ok();
    let loaded = NativeSessionLog::load_from_file(&path).ok();
    assert!(std::fs::remove_file(path).is_ok());

    assert_eq!(loaded, Some(log));
    assert!(
        raw.as_deref()
            .is_some_and(|raw| raw.contains("metric_recorded"))
    );
    assert!(
        raw.as_deref()
            .is_some_and(|raw| raw.contains("session_log_load"))
    );
    assert!(
        raw.as_deref()
            .is_some_and(|raw| !raw.contains("raw_sample"))
    );
    assert!(matches!(
        loaded.as_ref().and_then(|loaded| loaded.events.first()),
        Some(NativeSessionEvent::MetricRecorded {
            session_id: loaded_session_id,
            turn_id: None,
            metric: NativeDurationMetric {
                name,
                duration_ms: 7,
                attributes,
            },
        }) if loaded_session_id == &session_id
            && name == "session_log_load"
            && attributes == &vec![NativeMetricAttribute {
                key: String::from("status"),
                value: String::from("ok"),
            }]
    ));
}

#[cfg(test)]
#[test]
fn native_jsonl_session_store_appends_events_without_rewriting_log() {
    let path = temp_native_session_log_path("native-jsonl-session-store");
    let session_id = NativeSessionId(String::from("session-store"));
    let seeded_log = completed_text_exchange(
        session_id.clone(),
        NativeEntryId(String::from("entry-user-0")),
        NativeEntryId(String::from("entry-assistant-0")),
        NativeTurnId(String::from("turn-0")),
        String::from("hello"),
        String::from("hi"),
    );

    assert!(seeded_log.write_to_file(&path).is_ok());
    let seeded_content = std::fs::read_to_string(&path).unwrap_or_default();
    let seeded_len = seeded_content.len();

    let store = NativeJsonlSessionStore::new(path.clone());
    let next_event = NativeSessionEvent::EntryAppended {
        session_id,
        entry_id: NativeEntryId(String::from("entry-user-1")),
        parent_entry_id: Some(NativeEntryId(String::from("entry-assistant-0"))),
        turn_id: NativeTurnId(String::from("turn-1")),
        role: NativeRole::User,
        text: String::from("again"),
        provider: None,
    };

    assert!(store.append_event(&next_event).is_ok());
    let appended_content = std::fs::read_to_string(&path).unwrap_or_default();
    let loaded = store.load().ok();
    assert!(std::fs::remove_file(path).is_ok());

    assert!(appended_content.starts_with(&seeded_content));
    assert!(appended_content.len() > seeded_len);
    assert_eq!(loaded.as_ref().map(NativeSessionLog::len), Some(4));
    assert_eq!(
        loaded.as_ref().map(NativeSessionLog::next_turn_index),
        Some(2)
    );
}

#[cfg(test)]
#[test]
fn native_jsonl_session_store_batch_appends_events_without_rewriting_log() {
    let path = temp_native_session_log_path("native-jsonl-session-store-batch");
    let session_id = NativeSessionId(String::from("session-store-batch"));
    let seeded_log = completed_text_exchange(
        session_id.clone(),
        NativeEntryId(String::from("entry-user-0")),
        NativeEntryId(String::from("entry-assistant-0")),
        NativeTurnId(String::from("turn-0")),
        String::from("hello"),
        String::from("hi"),
    );

    assert!(seeded_log.write_to_file(&path).is_ok());
    let seeded_content = std::fs::read_to_string(&path).unwrap_or_default();
    let seeded_len = seeded_content.len();

    let store = NativeJsonlSessionStore::new(path.clone());
    let turn_id = NativeTurnId(String::from("turn-1"));
    let next_events = vec![
        NativeSessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: NativeEntryId(String::from("entry-user-1")),
            parent_entry_id: Some(NativeEntryId(String::from("entry-assistant-0"))),
            turn_id: turn_id.clone(),
            role: NativeRole::User,
            text: String::from("again"),
            provider: None,
        },
        NativeSessionEvent::TurnFinished {
            session_id,
            turn_id,
            outcome: NativeTurnOutcome::Completed,
            reason: None,
        },
    ];

    assert!(store.append_events(&next_events).is_ok());
    let appended_content = std::fs::read_to_string(&path).unwrap_or_default();
    let loaded = store.load().ok();
    assert!(std::fs::remove_file(path).is_ok());

    assert!(appended_content.starts_with(&seeded_content));
    assert!(appended_content.len() > seeded_len);
    assert_eq!(loaded.as_ref().map(NativeSessionLog::len), Some(5));
    assert_eq!(
        loaded.as_ref().map(NativeSessionLog::next_turn_index),
        Some(2)
    );
}

#[cfg(test)]
#[test]
fn native_session_log_load_skips_corrupt_middle_line_with_warning() {
    let path = temp_native_session_log_path("native-session-corrupt-middle");
    let session_id = NativeSessionId(String::from("session-corrupt"));
    let log = completed_text_exchange(
        session_id,
        NativeEntryId(String::from("entry-user-0")),
        NativeEntryId(String::from("entry-assistant-0")),
        NativeTurnId(String::from("turn-0")),
        String::from("hello"),
        String::from("hi"),
    );
    let mut lines: Vec<String> = log
        .events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap_or_default())
        .collect();
    lines.insert(1, String::from("{not valid json"));
    assert!(std::fs::write(&path, format!("{}\n", lines.join("\n"))).is_ok());

    let loaded = NativeSessionLog::load_from_file_with_warnings(&path);
    assert!(std::fs::remove_file(path).is_ok());

    assert!(loaded.is_ok());
    let Ok(loaded) = loaded else {
        return;
    };
    assert_eq!(loaded.log, log);
    assert_eq!(loaded.warnings.len(), 1);
    assert!(matches!(
        loaded.warnings.first(),
        Some(NativeSessionLoadWarning::InvalidJson { line_number: 2, reason })
            if !reason.contains("not valid json")
    ));
}

#[cfg(test)]
#[test]
fn native_session_log_load_skips_truncated_final_line_with_warning() {
    let path = temp_native_session_log_path("native-session-truncated-final");
    let session_id = NativeSessionId(String::from("session-truncated"));
    let log = completed_text_exchange(
        session_id,
        NativeEntryId(String::from("entry-user-0")),
        NativeEntryId(String::from("entry-assistant-0")),
        NativeTurnId(String::from("turn-0")),
        String::from("hello"),
        String::from("hi"),
    );
    let mut raw = log
        .events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    raw.push_str("\n{\"type\":\"entry_appended\"");
    assert!(std::fs::write(&path, raw).is_ok());

    let loaded = NativeSessionLog::load_from_file_with_warnings(&path);
    assert!(std::fs::remove_file(path).is_ok());

    assert!(loaded.is_ok());
    let Ok(loaded) = loaded else {
        return;
    };
    assert_eq!(loaded.log, log);
    assert_eq!(loaded.warnings.len(), 1);
    assert!(matches!(
        loaded.warnings.first(),
        Some(NativeSessionLoadWarning::InvalidJson { line_number: 4, reason })
            if !reason.contains("entry_appended")
    ));
}

#[cfg(all(test, unix))]
#[test]
fn native_jsonl_session_store_creates_owner_only_log_file() {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_native_session_log_path("native-session-store-mode");
    let store = NativeJsonlSessionStore::new(path.clone());
    let event = NativeSessionEvent::EntryAppended {
        session_id: NativeSessionId(String::from("session-mode")),
        entry_id: NativeEntryId(String::from("entry-user-0")),
        parent_entry_id: None,
        turn_id: NativeTurnId(String::from("turn-0")),
        role: NativeRole::User,
        text: String::from("hello"),
        provider: None,
    };

    assert!(store.append_event(&event).is_ok());
    let mode = std::fs::metadata(&path).map(|metadata| metadata.permissions().mode() & 0o777);
    assert!(std::fs::remove_file(path).is_ok());

    assert_eq!(mode.ok(), Some(0o600));
}

#[cfg(test)]
fn temp_native_session_log_path(name: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("{name}-{unique}.jsonl"))
}
