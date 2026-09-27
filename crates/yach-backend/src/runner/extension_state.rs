use std::path::Path;

use std::sync::Arc;

use tokio::sync::{Mutex as AsyncMutex, mpsc};
use yach_proto::{
    BackendEvent, ExtensionDiagnosticRecord, ExtensionDiagnosticSnapshotOutcome,
    ExtensionLifecycleAction, ExtensionLifecycleOutcome, ServerEvent,
};

use crate::{ExtensionStaticContextFile, activate_background_metadata_extensions};

#[derive(Clone)]
pub struct ExtensionPackageRootLoader {
    load: Arc<ExtensionPackageRootLoadFn>,
}

type ExtensionPackageRootLoadFn = dyn Fn() -> Vec<crate::ExtensionPackageRoot> + Send + Sync;

impl ExtensionPackageRootLoader {
    pub fn new(
        load: impl Fn() -> Vec<crate::ExtensionPackageRoot> + Send + Sync + 'static,
    ) -> Self {
        Self {
            load: Arc::new(load),
        }
    }

    pub fn load(&self) -> Vec<crate::ExtensionPackageRoot> {
        (self.load)()
    }
}

pub(super) type ExtensionManifestScanState = Arc<AsyncMutex<Option<crate::ExtensionManifestIndex>>>;
pub(super) type ExtensionActivationSnapshotState =
    Arc<AsyncMutex<crate::ExtensionActivationSnapshot>>;

pub(super) fn extension_package_roots_for_scan(
    configured_roots: &[crate::ExtensionPackageRoot],
    loader: Option<&ExtensionPackageRootLoader>,
) -> Vec<crate::ExtensionPackageRoot> {
    let mut roots = configured_roots.to_vec();
    if let Some(loader) = loader {
        roots.extend(loader.load());
    }
    roots
}

pub(super) fn schedule_extension_manifest_scan(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    package_roots: Vec<crate::ExtensionPackageRoot>,
    scan_state: ExtensionManifestScanState,
    activation_state: ExtensionActivationSnapshotState,
    trace: Option<yach_trace::TraceSink>,
    scan_scheduled: &mut bool,
) {
    if *scan_scheduled {
        return;
    }
    *scan_scheduled = true;
    mark_extension_scan(trace.as_ref(), "extension_manifest_scan_scheduled");
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("extension_manifest_scan_scheduled"),
    }));

    let tx = tx.clone();
    tokio::spawn(async move {
        mark_extension_scan(trace.as_ref(), "extension_manifest_scan_started");
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: String::from("extension_manifest_scan_started"),
        }));

        let scan = tokio::task::spawn_blocking(move || {
            crate::ExtensionManifestIndex::from_package_roots(package_roots)
        })
        .await;
        match scan {
            Ok(Ok(index)) => {
                let extension_count = index.records().len();
                let n = u32::try_from(extension_count).unwrap_or(u32::MAX);
                if let Some(trace) = trace.as_ref() {
                    trace.mark_n(
                        yach_trace::TraceScope::Startup,
                        "extension_manifest_scan_finished",
                        n,
                    );
                    trace.flush();
                }
                let host_start_count = index.host_start_count();
                let activation_records = index.records().to_vec();
                {
                    let mut discovered_index = scan_state.lock().await;
                    *discovered_index = Some(index);
                }
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!(
                        "extension_manifest_scan_finished extension_count={extension_count} host_start_count={host_start_count}"
                    ),
                }));
                schedule_extension_background_activation(
                    &tx,
                    activation_records,
                    activation_state,
                    trace.clone(),
                );
            }
            Ok(Err(error)) => {
                mark_extension_scan(trace.as_ref(), "extension_manifest_scan_failed");
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!(
                        "extension_manifest_scan_failed reason={}",
                        extension_manifest_scan_error_label(&error)
                    ),
                }));
            }
            Err(_) => {
                mark_extension_scan(trace.as_ref(), "extension_manifest_scan_failed");
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: String::from("extension_manifest_scan_failed reason=join_failed"),
                }));
            }
        }
    });
}

pub(super) async fn extension_static_context_files_from_scan_state(
    scan_state: &ExtensionManifestScanState,
) -> Vec<ExtensionStaticContextFile> {
    scan_state
        .lock()
        .await
        .as_ref()
        .map(crate::ExtensionManifestIndex::static_context_files)
        .unwrap_or_default()
}

fn schedule_extension_background_activation(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    package_records: Vec<crate::ExtensionPackageRecord>,
    activation_state: ExtensionActivationSnapshotState,
    trace: Option<yach_trace::TraceSink>,
) {
    mark_extension_scan(trace.as_ref(), "extension_background_activation_scheduled");
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("extension_background_activation_scheduled"),
    }));

    let tx = tx.clone();
    tokio::spawn(async move {
        mark_extension_scan(trace.as_ref(), "extension_background_activation_started");
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: String::from("extension_background_activation_started"),
        }));
        // `TraceSink` is `Clone` over a shared `Arc<Mutex<_>>` and one
        // `start: Instant`, so the blocking task's marks keep the session's
        // time origin.
        let activation_trace = trace.clone();
        let activation = tokio::task::spawn_blocking(move || {
            activate_background_metadata_extensions(
                &package_records,
                crate::ExtensionBackgroundActivationConfig::conservative(),
                activation_trace.as_ref(),
            )
        })
        .await;

        if let Ok(snapshot) = activation {
            mark_extension_scan(trace.as_ref(), "extension_background_activation_finished");
            let active_extension_count = snapshot
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.activation_state == crate::ExtensionActivationState::Active
                })
                .count();
            let registered_tool_count = snapshot.active_tool_names().len();
            let host_start_count = snapshot.host_start_count;
            {
                let mut active_snapshot = activation_state.lock().await;
                let previous_reviewer = active_snapshot.reviewer.take();
                *active_snapshot = snapshot;
                match (
                    previous_reviewer.as_ref(),
                    active_snapshot.reviewer.as_ref(),
                ) {
                    (Some(previous), None) => {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::ReviewerStatusChanged {
                            reviewer_id: previous.reviewer_id.clone(),
                            generation: previous.generation,
                            state: yach_proto::ReviewerState::Unavailable,
                            disclosure_summary: String::new(),
                        }));
                    }
                    (_, Some(reviewer)) => {
                        let changed = previous_reviewer.as_ref().is_none_or(|previous| {
                            previous.reviewer_id != reviewer.reviewer_id
                                || previous.generation != reviewer.generation
                        });
                        if changed {
                            let _ =
                                tx.send(BackendEvent::Server(ServerEvent::ReviewerStatusChanged {
                                    reviewer_id: reviewer.reviewer_id.clone(),
                                    generation: reviewer.generation,
                                    state: yach_proto::ReviewerState::Selected,
                                    disclosure_summary: reviewer.disclosure_summary.clone(),
                                }));
                        }
                    }
                    (None, None) => {}
                }
            }
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!(
                    "extension_background_activation_finished active_extension_count={active_extension_count} registered_tool_count={registered_tool_count} host_start_count={host_start_count}"
                ),
            }));
        } else {
            mark_extension_scan(trace.as_ref(), "extension_background_activation_failed");
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: String::from("extension_background_activation_failed reason=join_failed"),
            }));
        }
    });
}

pub(super) async fn extension_activation_snapshot_from_state(
    activation_state: &ExtensionActivationSnapshotState,
) -> crate::ExtensionActivationSnapshot {
    activation_state.lock().await.clone()
}

pub(super) async fn handle_native_extension_diagnostic_snapshot_request(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    activation_state: &ExtensionActivationSnapshotState,
    request_id: String,
    selector: Option<&str>,
) {
    let selector = selector
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
        .map(str::to_string);
    let snapshot = activation_state.lock().await.clone();
    let mut records = snapshot
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            selector.as_deref().is_none_or(|selector| {
                extension_activation_diagnostic_matches_selector(diagnostic, selector)
            })
        })
        .map(extension_diagnostic_record_from_activation)
        .collect::<Vec<_>>();
    records.sort_by(extension_diagnostic_record_order);

    let outcome = if selector.is_some() && records.is_empty() {
        ExtensionDiagnosticSnapshotOutcome::NotFound
    } else {
        ExtensionDiagnosticSnapshotOutcome::Completed
    };
    let message = match (&selector, outcome) {
        (Some(selector), ExtensionDiagnosticSnapshotOutcome::NotFound) => {
            Some(format!("extension not found: {selector}"))
        }
        _ => None,
    };

    let _ = tx.send(BackendEvent::Server(
        ServerEvent::ExtensionDiagnosticSnapshotUpdated {
            request_id,
            outcome,
            records,
            message,
        },
    ));
}

pub(super) async fn handle_native_extension_lifecycle_request(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    scan_state: &ExtensionManifestScanState,
    activation_state: &ExtensionActivationSnapshotState,
    request_id: String,
    action: ExtensionLifecycleAction,
    selector: &str,
) {
    let selector = selector.trim().to_string();
    if selector.is_empty() {
        let _ = tx.send(BackendEvent::Server(
            ServerEvent::ExtensionLifecycleFinished {
                request_id,
                action,
                selector,
                outcome: ExtensionLifecycleOutcome::Failed,
                message: String::from("extension selector is required"),
            },
        ));
        return;
    }

    if matches!(
        action,
        ExtensionLifecycleAction::Reload | ExtensionLifecycleAction::Trust
    ) {
        let Some(record) = extension_package_record_from_scan_state(scan_state, &selector).await
        else {
            let _ = tx.send(BackendEvent::Server(
                ServerEvent::ExtensionLifecycleFinished {
                    request_id,
                    action,
                    selector: selector.clone(),
                    outcome: ExtensionLifecycleOutcome::NotFound,
                    message: format!("extension not discovered: {selector}"),
                },
            ));
            return;
        };
        if action == ExtensionLifecycleAction::Trust {
            schedule_native_extension_trust(
                tx.clone(),
                activation_state.clone(),
                request_id,
                selector,
                record,
            );
        } else {
            schedule_native_extension_reload(
                tx.clone(),
                activation_state.clone(),
                request_id,
                selector,
                record,
            );
        }
        return;
    }

    if action == ExtensionLifecycleAction::Revoke {
        let record = extension_package_record_from_scan_state(scan_state, &selector).await;
        let discovered_id = record.as_ref().map(|record| record.manifest.id.0.as_str());
        let extension_id = match crate::grant_id_from_selector(discovered_id, &selector) {
            Ok(id) => id,
            Err(message) => {
                let _ = tx.send(BackendEvent::Server(
                    ServerEvent::ExtensionLifecycleFinished {
                        request_id,
                        action,
                        selector,
                        outcome: ExtensionLifecycleOutcome::Failed,
                        message,
                    },
                ));
                return;
            }
        };
        schedule_native_extension_revoke(
            tx.clone(),
            activation_state.clone(),
            request_id,
            selector,
            extension_id,
        );
        return;
    }

    let previous_reviewer = {
        let snapshot = activation_state.lock().await;
        snapshot.reviewer.clone()
    };
    let (outcome, message) = {
        let mut snapshot = activation_state.lock().await;
        match action {
            ExtensionLifecycleAction::Stop => match snapshot.stop_extension(&selector) {
                Ok(diagnostic) => {
                    let extension_id = diagnostic
                        .extension_id
                        .as_deref()
                        .unwrap_or(selector.as_str());
                    (
                        ExtensionLifecycleOutcome::Completed,
                        format!("extension stopped: {extension_id}"),
                    )
                }
                Err(crate::ExtensionActivationLifecycleError::NotFound { .. }) => (
                    ExtensionLifecycleOutcome::NotFound,
                    format!("extension not found: {selector}"),
                ),
                Err(crate::ExtensionActivationLifecycleError::NotActive { .. }) => (
                    ExtensionLifecycleOutcome::NotActive,
                    format!("extension is not active: {selector}"),
                ),
            },
            ExtensionLifecycleAction::Reload
            | ExtensionLifecycleAction::Trust
            | ExtensionLifecycleAction::Revoke => {
                unreachable!("reload, trust, and revoke are handled before snapshot lock");
            }
        }
    };

    if action == ExtensionLifecycleAction::Stop
        && outcome == ExtensionLifecycleOutcome::Completed
        && let Some(reviewer) = previous_reviewer
    {
        let _ = tx.send(BackendEvent::Server(ServerEvent::ReviewerStatusChanged {
            reviewer_id: reviewer.reviewer_id,
            generation: reviewer.generation,
            state: yach_proto::ReviewerState::Unavailable,
            disclosure_summary: String::new(),
        }));
    }

    let _ = tx.send(BackendEvent::Server(
        ServerEvent::ExtensionLifecycleFinished {
            request_id,
            action,
            selector,
            outcome,
            message,
        },
    ));
}

fn extension_activation_diagnostic_matches_selector(
    diagnostic: &crate::ExtensionActivationDiagnostic,
    selector: &str,
) -> bool {
    diagnostic.extension_id.as_deref() == Some(selector)
        || diagnostic.source_ref.as_deref() == Some(selector)
        || diagnostic.install_source.as_deref() == Some(selector)
        || diagnostic.package_root == Path::new(selector)
        || diagnostic.package_root.to_string_lossy() == selector
        || diagnostic.manifest_path.as_deref() == Some(Path::new(selector))
        || diagnostic
            .manifest_path
            .as_ref()
            .is_some_and(|path| path.to_string_lossy() == selector)
}

fn extension_diagnostic_record_from_activation(
    diagnostic: &crate::ExtensionActivationDiagnostic,
) -> ExtensionDiagnosticRecord {
    ExtensionDiagnosticRecord {
        id: diagnostic.extension_id.clone(),
        version: diagnostic.version.clone(),
        scope: extension_install_scope_label(diagnostic.scope).to_owned(),
        package_root: diagnostic.package_root.to_string_lossy().into_owned(),
        manifest_path: diagnostic
            .manifest_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        source_ref: diagnostic.source_ref.clone(),
        install_source: diagnostic.install_source.clone(),
        activation_state: diagnostic.activation_state.as_str().to_owned(),
        generation: diagnostic.generation,
        last_error_kind: diagnostic
            .last_error_kind
            .map(|error_kind| error_kind.as_str().to_owned()),
        last_error_summary: diagnostic.last_error_summary.clone(),
        registered_tools: diagnostic.registered_tools.clone(),
        provider_visible_tools: diagnostic.provider_visible_tools.clone(),
        capabilities: diagnostic
            .requested_capabilities
            .as_ref()
            .map(proto_capability_names),
        capability_grant: proto_capability_grant(&diagnostic.capability_grant),
    }
}

fn proto_capability_names(
    capabilities: &std::collections::BTreeSet<crate::ExtensionCapability>,
) -> Vec<String> {
    capabilities
        .iter()
        .map(crate::ExtensionCapability::as_str)
        .map(String::from)
        .collect()
}

fn proto_capability_grant(grant: &crate::ExtensionCapabilityGrantStatus) -> Option<Vec<String>> {
    match grant {
        crate::ExtensionCapabilityGrantStatus::Unknown => None,
        crate::ExtensionCapabilityGrantStatus::Absent => Some(Vec::new()),
        crate::ExtensionCapabilityGrantStatus::Approved(set) => Some(proto_capability_names(set)),
    }
}

fn extension_diagnostic_record_order(
    left: &ExtensionDiagnosticRecord,
    right: &ExtensionDiagnosticRecord,
) -> std::cmp::Ordering {
    left.id
        .as_deref()
        .unwrap_or("none")
        .cmp(right.id.as_deref().unwrap_or("none"))
        .then_with(|| left.package_root.cmp(&right.package_root))
}

const fn extension_install_scope_label(scope: crate::ExtensionInstallScope) -> &'static str {
    match scope {
        crate::ExtensionInstallScope::User => "user",
        crate::ExtensionInstallScope::Project => "project",
        crate::ExtensionInstallScope::Ephemeral => "ephemeral",
    }
}

fn schedule_native_extension_reload(
    tx: mpsc::UnboundedSender<BackendEvent>,
    activation_state: ExtensionActivationSnapshotState,
    request_id: String,
    selector: String,
    record: crate::ExtensionPackageRecord,
) {
    tokio::task::spawn_blocking(move || {
        let mut snapshot = activation_state.blocking_lock();
        let (outcome, message) = extension_reload_lifecycle_outcome(
            // Reload is a user-initiated lifecycle command, not session
            // activation; the startup-scoped host marks measure the latter and
            // a mid-session reload has no `process_main_start` relationship.
            &snapshot.reload_extension_from_record(
                &record,
                crate::ExtensionBackgroundActivationConfig::conservative(),
                None,
            ),
            &selector,
        );
        let reviewer_snapshot = snapshot.reviewer.clone();
        let _ = tx.send(BackendEvent::Server(
            ServerEvent::ExtensionLifecycleFinished {
                request_id,
                action: ExtensionLifecycleAction::Reload,
                selector,
                outcome,
                message,
            },
        ));
        if outcome == ExtensionLifecycleOutcome::Completed
            && let Some(reviewer) = reviewer_snapshot
        {
            let _ = tx.send(BackendEvent::Server(ServerEvent::ReviewerStatusChanged {
                reviewer_id: reviewer.reviewer_id,
                generation: reviewer.generation,
                state: yach_proto::ReviewerState::Reloaded,
                disclosure_summary: reviewer.disclosure_summary,
            }));
        }
    });
}

fn schedule_native_extension_revoke(
    tx: mpsc::UnboundedSender<BackendEvent>,
    activation_state: ExtensionActivationSnapshotState,
    request_id: String,
    selector: String,
    extension_id: String,
) {
    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        let mut snapshot = lifecycle_test_seam::blocking_lock_after_probe(&activation_state);
        #[cfg(not(test))]
        let mut snapshot = activation_state.blocking_lock();
        let (outcome, message) =
            match crate::revoke_grant(&extension_id, crate::ExtensionDecisionSurface::Lifecycle) {
                Ok(had_grant) => {
                    let _ = snapshot.stop_extension(&selector);
                    (
                        ExtensionLifecycleOutcome::Completed,
                        crate::revoke_confirmation_message(&extension_id, had_grant),
                    )
                }
                Err(crate::ExtensionAuthorityError::DurabilityUnknown) => (
                    ExtensionLifecycleOutcome::Failed,
                    crate::ExtensionAuthorityError::DurabilityUnknown.to_string(),
                ),
                Err(error) => (
                    ExtensionLifecycleOutcome::Failed,
                    format!("failed to remove capability grant: {error}"),
                ),
            };
        let _ = tx.send(BackendEvent::Server(
            ServerEvent::ExtensionLifecycleFinished {
                request_id,
                action: ExtensionLifecycleAction::Revoke,
                selector,
                outcome,
                message,
            },
        ));
    });
}

fn schedule_native_extension_trust(
    tx: mpsc::UnboundedSender<BackendEvent>,
    activation_state: ExtensionActivationSnapshotState,
    request_id: String,
    selector: String,
    record: crate::ExtensionPackageRecord,
) {
    tokio::task::spawn_blocking(move || {
        let extension_id = record.manifest.id.0.as_str();
        let mut snapshot = activation_state.blocking_lock();
        let (outcome, message) = match crate::grant_requested(
            extension_id,
            &record.manifest.version,
            &record.manifest.contributes.tools,
            record.manifest.contributes.reviewer.as_ref(),
            crate::ExtensionDecisionSurface::Lifecycle,
        ) {
            Ok(None) => (
                ExtensionLifecycleOutcome::Completed,
                crate::nothing_to_grant_message(extension_id),
            ),
            Ok(Some(grant)) => {
                #[cfg(test)]
                lifecycle_test_seam::wait_after_durable_write();
                let grant_message = crate::grant_confirmation_message(
                    extension_id,
                    &record.manifest.contributes.tools,
                    &grant.approved,
                );
                let (reload_outcome, reload_message) = extension_reload_lifecycle_outcome(
                    &snapshot.reload_extension_from_record(
                        &record,
                        crate::ExtensionBackgroundActivationConfig::conservative(),
                        None,
                    ),
                    &selector,
                );
                match reload_outcome {
                    ExtensionLifecycleOutcome::Completed => {
                        (ExtensionLifecycleOutcome::Completed, grant_message)
                    }
                    _ => (reload_outcome, format!("{grant_message}; {reload_message}")),
                }
            }
            Err(crate::ExtensionAuthorityError::DurabilityUnknown) => (
                ExtensionLifecycleOutcome::Failed,
                crate::ExtensionAuthorityError::DurabilityUnknown.to_string(),
            ),
            Err(error) => (
                ExtensionLifecycleOutcome::Failed,
                format!("failed to write capability grant: {error}"),
            ),
        };
        let _ = tx.send(BackendEvent::Server(
            ServerEvent::ExtensionLifecycleFinished {
                request_id,
                action: ExtensionLifecycleAction::Trust,
                selector,
                outcome,
                message,
            },
        ));
    });
}

async fn extension_package_record_from_scan_state(
    scan_state: &ExtensionManifestScanState,
    selector: &str,
) -> Option<crate::ExtensionPackageRecord> {
    scan_state.lock().await.as_ref().and_then(|index| {
        index
            .records()
            .iter()
            .find(|record| extension_package_record_matches_selector(record, selector))
            .cloned()
    })
}

fn extension_package_record_matches_selector(
    record: &crate::ExtensionPackageRecord,
    selector: &str,
) -> bool {
    record.manifest.id.0 == selector
        || record.source_ref.as_deref() == Some(selector)
        || record.package_root == Path::new(selector)
        || record.package_root.to_string_lossy() == selector
        || record.manifest_path == Path::new(selector)
        || record.manifest_path.to_string_lossy() == selector
}

fn extension_reload_lifecycle_outcome(
    diagnostic: &crate::ExtensionActivationDiagnostic,
    selector: &str,
) -> (ExtensionLifecycleOutcome, String) {
    let extension_id = diagnostic.extension_id.as_deref().unwrap_or(selector);
    match diagnostic.activation_state {
        crate::ExtensionActivationState::Active => (
            ExtensionLifecycleOutcome::Completed,
            format!("extension reloaded: {extension_id}"),
        ),
        crate::ExtensionActivationState::Discovered => (
            ExtensionLifecycleOutcome::NotActive,
            format!("extension is not post-first-paint metadata extension: {extension_id}"),
        ),
        crate::ExtensionActivationState::Blocked => (
            ExtensionLifecycleOutcome::Failed,
            format!(
                "extension reload blocked: {extension_id}: {}",
                diagnostic
                    .last_error_summary
                    .as_deref()
                    .unwrap_or("activation blocked")
            ),
        ),
        crate::ExtensionActivationState::Failed => (
            ExtensionLifecycleOutcome::Failed,
            format!(
                "extension reload failed: {extension_id}: {}",
                diagnostic
                    .last_error_summary
                    .as_deref()
                    .unwrap_or("activation failed")
            ),
        ),
        _ => (
            ExtensionLifecycleOutcome::Failed,
            format!("extension reload ended in unexpected state: {extension_id}"),
        ),
    }
}

fn mark_extension_scan(trace: Option<&yach_trace::TraceSink>, label: &str) {
    if let Some(trace) = trace {
        trace.mark(yach_trace::TraceScope::Startup, label);
        trace.flush();
    }
}

pub(super) fn mark_turn(
    trace: Option<&yach_trace::TraceSink>,
    turn_id: &crate::TurnId,
    label: &str,
) {
    if let Some(trace) = trace {
        trace.mark(yach_trace::TraceScope::Turn(&turn_id.0), label);
    }
}

pub(super) fn mark_turn_n(
    trace: Option<&yach_trace::TraceSink>,
    turn_id: &crate::TurnId,
    label: &str,
    n: u32,
) {
    if let Some(trace) = trace {
        trace.mark_n(yach_trace::TraceScope::Turn(&turn_id.0), label, n);
    }
}

fn extension_manifest_scan_error_label(error: &crate::ExtensionPackageIndexError) -> &'static str {
    match error {
        crate::ExtensionPackageIndexError::MissingPackageRoot { .. } => "missing_package_root",
        crate::ExtensionPackageIndexError::MissingManifest { .. } => "missing_manifest",
        crate::ExtensionPackageIndexError::MissingManifestFile { .. } => "missing_manifest_file",
        crate::ExtensionPackageIndexError::MalformedPackageJson { .. } => "malformed_package_json",
        crate::ExtensionPackageIndexError::InvalidManifestPointer { .. } => {
            "invalid_manifest_pointer"
        }
        crate::ExtensionPackageIndexError::ManifestPathEscapedPackageRoot { .. } => {
            "manifest_path_escaped_package_root"
        }
        crate::ExtensionPackageIndexError::Manifest { .. } => "invalid_manifest",
        crate::ExtensionPackageIndexError::Catalog(_) => "catalog_error",
    }
}

#[cfg(test)]
mod lifecycle_test_seam {
    use Future as _;
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender};
    use std::task::{Context, Poll};

    struct Hold {
        arrived: Sender<()>,
        resume: Receiver<()>,
    }

    static HOLD: Mutex<Option<Hold>> = Mutex::new(None);
    static REVOKE_ACQUIRE_PROBED: Mutex<Option<Sender<bool>>> = Mutex::new(None);

    pub fn arm(arrived: Sender<()>, resume: Receiver<()>) {
        let Ok(mut guard) = HOLD.lock() else {
            return;
        };
        *guard = Some(Hold { arrived, resume });
    }

    pub fn arm_revoke(acquire_probed: Sender<bool>) {
        let Ok(mut guard) = REVOKE_ACQUIRE_PROBED.lock() else {
            return;
        };
        *guard = Some(acquire_probed);
    }

    pub fn blocking_lock_after_probe(
        activation_state: &super::ExtensionActivationSnapshotState,
    ) -> tokio::sync::MutexGuard<'_, crate::ExtensionActivationSnapshot> {
        let mut lock = Box::pin(activation_state.lock());
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        let first_poll = lock.as_mut().poll(&mut context);
        let was_pending = matches!(first_poll, Poll::Pending);
        let acquire_probed = match REVOKE_ACQUIRE_PROBED.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => None,
        };
        if let Some(acquire_probed) = acquire_probed {
            let _ = acquire_probed.send(was_pending);
        }
        match first_poll {
            Poll::Ready(snapshot) => snapshot,
            Poll::Pending => futures::executor::block_on(lock),
        }
    }

    pub fn wait_after_durable_write() {
        let hold = match HOLD.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => None,
        };
        let Some(hold) = hold else {
            return;
        };
        let _ = hold.arrived.send(());
        let _ = hold.resume.recv();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    const HELPER_HOME_ENV: &str = "YACH_LIFECYCLE_SNAPSHOT_TEST_HOME";
    const EXTENSION_ID: &str = "example.lifecycle-race";
    const SEQUENCE_HELPER_HOME_ENV: &str = "YACH_LIFECYCLE_SEQUENCE_TEST_HOME";

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(prefix: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "yach-{prefix}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            assert!(
                fs::create_dir_all(&path).is_ok(),
                "temporary directory should be created: {path:?}"
            );
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_network_package(root: &Path) {
        assert!(
            fs::create_dir_all(root).is_ok(),
            "package root should be created: {root:?}"
        );
        let manifest = format!(
            r#"{{
  "schema": "yach.extension.v1",
  "id": "{EXTENSION_ID}",
  "version": "0.1.0",
  "main": {{
    "command": "sh",
    "args": ["host.sh"]
  }},
  "activation": {{
    "events": ["postFirstPaint"]
  }},
  "contributes": {{
    "tools": [{{
      "name": "fetch_url",
      "description": "Return a static fixture payload for a URL.",
      "risk": "uses_network",
      "provider_visible": true
    }}]
  }}
}}"#
        );
        assert!(
            fs::write(root.join("yach.extension.json"), manifest).is_ok(),
            "manifest should write"
        );
        let host = format!(
            r#"while IFS= read -r line; do
case "$line" in
  *extension.initialize*)
    printf '%s\n' \
      '{{"type":"extension.ready","protocol":"yach.extension-host.v2","extension_id":"{EXTENSION_ID}"}}' \
      '{{"type":"tool.register","name":"fetch_url","description":"Return a static fixture payload for a URL.","risk":"uses_network","provider_visible":true,"input_schema":{{"type":"object","additionalProperties":false,"required":["url"],"properties":{{"url":{{"type":"string"}}}},"maxSerializedBytes":1024}}}}'
    ;;
esac
done
"#
        );
        assert!(
            fs::write(root.join("host.sh"), host).is_ok(),
            "host script should write"
        );
    }

    fn run_snapshot_hold_helper() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok(), "helper runtime should build: {runtime:?}");
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let package = TestDir::new("lifecycle-package");
            write_network_package(package.path());
            let index =
                crate::ExtensionManifestIndex::from_package_roots([crate::ExtensionPackageRoot {
                    root: package.path().to_path_buf(),
                    scope: crate::ExtensionInstallScope::User,
                    source_ref: Some(String::from("lifecycle-test")),
                }]);
            assert!(index.is_ok(), "fixture package should scan: {index:?}");
            let Ok(index) = index else {
                return;
            };

            let scan_state = Arc::new(AsyncMutex::new(Some(index)));
            let activation_state = Arc::new(AsyncMutex::new(
                crate::ExtensionActivationSnapshot::default(),
            ));
            let (tx, mut rx) = mpsc::unbounded_channel();
            let (arrived_tx, arrived_rx) = std::sync::mpsc::channel();
            let (resume_tx, resume_rx) = std::sync::mpsc::channel();
            super::lifecycle_test_seam::arm(arrived_tx, resume_rx);

            handle_native_extension_lifecycle_request(
                &tx,
                &scan_state,
                &activation_state,
                String::from("audit-trust"),
                ExtensionLifecycleAction::Trust,
                EXTENSION_ID,
            )
            .await;

            let arrived = arrived_rx.recv_timeout(Duration::from_secs(30));
            assert!(
                arrived.is_ok(),
                "trust should reach the post-write seam: {arrived:?}"
            );
            let snapshot_held = activation_state.try_lock().is_err();
            let external_store = crate::ExtensionAuthorityStore::in_home(Path::new(
                &std::env::var_os("HOME").unwrap_or_default(),
            ));
            let revoked =
                external_store.revoke_grant(EXTENSION_ID, crate::ExtensionDecisionSurface::Cli);
            assert_eq!(revoked, Ok(true));
            let _ = resume_tx.send(());

            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            let mut trust_outcome = None;
            while tokio::time::Instant::now() < deadline {
                let event = tokio::time::timeout_at(deadline, rx.recv()).await;
                if let Ok(Some(BackendEvent::Server(ServerEvent::ExtensionLifecycleFinished {
                    request_id,
                    outcome,
                    ..
                }))) = event
                    && request_id == "audit-trust"
                {
                    trust_outcome = Some(outcome);
                    break;
                }
            }
            assert_eq!(trust_outcome, Some(ExtensionLifecycleOutcome::Failed));
            assert!(
                snapshot_held,
                "trust must hold the activation snapshot across the durable write until reload"
            );
            let snapshot = activation_state.lock().await;
            assert_eq!(snapshot.active_tool_names(), Vec::<&str>::new());
            assert_eq!(snapshot.diagnostics.len(), 1);
            assert_eq!(
                snapshot.diagnostics[0].activation_state,
                crate::ExtensionActivationState::Blocked
            );
            assert_eq!(
                snapshot.diagnostics[0].capability_grant,
                crate::ExtensionCapabilityGrantStatus::Absent
            );
        });
    }

    #[test]
    fn trust_holds_activation_snapshot_after_durable_write_before_reload() {
        if std::env::var(HELPER_HOME_ENV).is_ok() {
            run_snapshot_hold_helper();
            return;
        }

        let home = TestDir::new("lifecycle-home");
        let executable = std::env::current_exe();
        assert!(
            executable.is_ok(),
            "test executable should resolve: {executable:?}"
        );
        let Ok(executable) = executable else {
            return;
        };
        let current_thread = std::thread::current();
        let Some(test_name) = current_thread.name() else {
            return;
        };
        let output = Command::new(&executable)
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env("HOME", home.path())
            .env(HELPER_HOME_ENV, home.path())
            .output();
        assert!(output.is_ok(), "snapshot helper should spawn: {output:?}");
        let Ok(output) = output else {
            return;
        };
        assert!(
            output.status.success(),
            "snapshot helper failed: status={:?} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn run_same_runner_sequence_helper() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok(), "helper runtime should build: {runtime:?}");
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let package = TestDir::new("lifecycle-sequence-package");
            write_network_package(package.path());
            let index =
                crate::ExtensionManifestIndex::from_package_roots([crate::ExtensionPackageRoot {
                    root: package.path().to_path_buf(),
                    scope: crate::ExtensionInstallScope::User,
                    source_ref: Some(String::from("lifecycle-sequence-test")),
                }]);
            assert!(index.is_ok(), "fixture package should scan: {index:?}");
            let Ok(index) = index else {
                return;
            };
            let scan_state = Arc::new(AsyncMutex::new(Some(index)));
            let activation_state = Arc::new(AsyncMutex::new(
                crate::ExtensionActivationSnapshot::default(),
            ));
            let (tx, mut rx) = mpsc::unbounded_channel();
            let (trust_arrived_tx, trust_arrived_rx) = std::sync::mpsc::channel();
            let (trust_resume_tx, trust_resume_rx) = std::sync::mpsc::channel();
            let (revoke_probed_tx, revoke_probed_rx) = std::sync::mpsc::channel();
            super::lifecycle_test_seam::arm(trust_arrived_tx, trust_resume_rx);
            super::lifecycle_test_seam::arm_revoke(revoke_probed_tx);

            handle_native_extension_lifecycle_request(
                &tx,
                &scan_state,
                &activation_state,
                String::from("queued-trust"),
                ExtensionLifecycleAction::Trust,
                EXTENSION_ID,
            )
            .await;
            assert!(
                trust_arrived_rx
                    .recv_timeout(Duration::from_secs(30))
                    .is_ok(),
                "trust should reach the post-write seam"
            );
            handle_native_extension_lifecycle_request(
                &tx,
                &scan_state,
                &activation_state,
                String::from("queued-revoke"),
                ExtensionLifecycleAction::Revoke,
                EXTENSION_ID,
            )
            .await;
            assert_eq!(
                revoke_probed_rx.recv_timeout(Duration::from_secs(30)),
                Ok(true),
                "revoke's actual snapshot acquisition must be pending while trust owns it"
            );
            assert!(
                rx.try_recv().is_err(),
                "queued revoke must not finish while trust owns the snapshot"
            );
            let _ = trust_resume_tx.send(());

            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            let mut finished = Vec::new();
            while finished.len() < 2 && tokio::time::Instant::now() < deadline {
                let event = tokio::time::timeout_at(deadline, rx.recv()).await;
                if let Ok(Some(BackendEvent::Server(ServerEvent::ExtensionLifecycleFinished {
                    request_id,
                    outcome,
                    ..
                }))) = event
                {
                    finished.push((request_id, outcome));
                }
            }
            assert_eq!(
                finished,
                vec![
                    (
                        String::from("queued-trust"),
                        ExtensionLifecycleOutcome::Completed
                    ),
                    (
                        String::from("queued-revoke"),
                        ExtensionLifecycleOutcome::Completed
                    ),
                ]
            );
            let snapshot = activation_state.lock().await;
            assert_eq!(snapshot.active_tool_names(), Vec::<&str>::new());
            assert_eq!(snapshot.diagnostics.len(), 1);
            assert_eq!(
                snapshot.diagnostics[0].activation_state,
                crate::ExtensionActivationState::Stopped
            );
            let store = crate::ExtensionAuthorityStore::in_home(Path::new(
                &std::env::var_os("HOME").unwrap_or_default(),
            ));
            assert_eq!(store.load_grant(EXTENSION_ID), Ok(None));
        });
    }

    #[test]
    fn queued_revoke_runs_after_trust_reload_and_stops_the_host() {
        if std::env::var(SEQUENCE_HELPER_HOME_ENV).is_ok() {
            run_same_runner_sequence_helper();
            return;
        }
        let home = TestDir::new("lifecycle-sequence-home");
        let executable = std::env::current_exe();
        assert!(
            executable.is_ok(),
            "test executable should resolve: {executable:?}"
        );
        let Ok(executable) = executable else {
            return;
        };
        let current_thread = std::thread::current();
        let Some(test_name) = current_thread.name() else {
            return;
        };
        let output = Command::new(&executable)
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env("HOME", home.path())
            .env(SEQUENCE_HELPER_HOME_ENV, home.path())
            .output();
        assert!(output.is_ok(), "sequence helper should spawn: {output:?}");
        let Ok(output) = output else {
            return;
        };
        assert!(
            output.status.success(),
            "sequence helper failed: status={:?} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
