//! Backend runner groundwork for yach.
//!
//! This crate owns backend-facing concepts that are not specific to the
//! temporary Pi RPC adapter or to the eventual native provider implementation.
//! The public Interface is re-exported here; focused Modules keep the
//! Implementation local to runner, resource, tool, session, and provider concerns.

mod native_runner;
mod provider;
mod resource;
mod runner;
mod session;
mod session_store;
mod tools;

pub mod rig_adapter;
pub mod rig_diagnostics;

pub use native_runner::*;
pub use provider::*;
pub use resource::*;
pub use runner::*;
pub use session::*;
pub use session_store::*;
pub use tools::*;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rig::streaming::{RawStreamingChoice, RawStreamingToolCall, ToolCallDeltaContent};

    use super::{
        BackendCapabilities, BackendKind, BackendMetadata, BoundedProviderStreamBuffer,
        FixtureNativeToolExecutor, NativeEntryId, NativeProviderToolResult,
        NativeResourceContextError, NativeResourceContextPolicy, NativeResourceEntryKind,
        NativeResourcePathError, NativeResourceProviderVisibility, NativeResourceReadError,
        NativeResourceReadPolicy, NativeResourceRoot, NativeResourceRootKind,
        NativeResourceSearchPolicy, NativeRole, NativeSessionEvent, NativeSessionId,
        NativeSessionLog, NativeToolContinuationContext, NativeToolContinuationError,
        NativeToolContinuationPolicy, NativeToolError, NativeToolExecutionError,
        NativeToolExecutionResult, NativeToolExecutor, NativeToolOutcome, NativeToolPayloadSummary,
        NativeToolPermissionPolicy, NativeToolPermissionState, NativeToolRegistry,
        NativeToolRequestId, NativeTurnId, NativeTurnOutcome, PendingNativeToolRequest,
        ProjectReadOnlyToolExecutor, ProviderContinuationMappingError, ProviderContinuationRequest,
        ProviderContinuationValidationError, ProviderContinuationValidationPolicy, ProviderError,
        ProviderErrorKind, ProviderExtension, ProviderFinishReason, ProviderMessage,
        ProviderMetadata, ProviderModel, ProviderRequest, ProviderStreamEvent, ProviderToolCall,
        ProviderUsage, announce_connected, backend_channels, build_fixture_provider_tool_results,
        build_project_readonly_provider_tool_results, build_provider_continuation_submission,
        completed_text_exchange, pending_tool_request_from_provider_call,
        record_native_tool_validation, rig_adapter, start_backend_session,
        validate_provider_continuation_request,
    };
    use yach_proto::{BackendEvent, Capability, ClientEvent, Handshake, NegotiatedCapabilities};

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
        assert_eq!(projected.messages.len(), 3);
        assert_eq!(projected.messages[0].role, NativeRole::User);
        assert_eq!(projected.messages[1].role, NativeRole::Tool);
        assert_eq!(projected.messages[2].role, NativeRole::Tool);

        let first_tool =
            serde_json::from_str::<serde_json::Value>(&projected.messages[1].content).ok();
        let second_tool =
            serde_json::from_str::<serde_json::Value>(&projected.messages[2].content).ok();
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
    fn provider_tool_call_validation_records_redacted_session_events() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let policy = NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata");
        let tool_call = ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"secret-label"}),
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
        let path = temp_log_path("native-provider-tool-validation");
        assert!(log.write_to_file(&path).is_ok());
        let raw = std::fs::read_to_string(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());
        assert!(raw.is_some_and(|raw| {
            raw.contains("tool_payload_redacted") || !raw.contains("secret-label")
        }));
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
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id,
            turn_id,
            tool_request_id,
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(result_summary),
        });
        let path = temp_log_path("native-session-tool-records");

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
    });
    log.push(NativeSessionEvent::ToolExecutionFinished {
        session_id: session_id.clone(),
        turn_id: NativeTurnId(String::from("turn-4")),
        tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
        outcome: NativeToolOutcome::Completed,
        reason: None,
        result_summary: None,
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
fn temp_native_session_log_path(name: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("{name}-{unique}.jsonl"))
}
