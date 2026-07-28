use std::path::Path;
use std::sync::Arc;

use tokio::sync::{Mutex as AsyncMutex, mpsc};
use yach_proto::{
    BackendEvent, ExtensionDiagnosticRecord, ExtensionDiagnosticSnapshotOutcome,
    ExtensionLifecycleAction, ExtensionLifecycleOutcome, ServerEvent,
};

use crate::{NativeExtensionStaticContextFile, activate_background_metadata_extensions};

#[derive(Clone)]
pub struct NativeStartupTraceMarker {
    mark: Arc<NativeStartupTraceMarkFn>,
}

type NativeStartupTraceMarkFn = dyn Fn(&str) + Send + Sync;

impl NativeStartupTraceMarker {
    pub fn new(mark: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            mark: Arc::new(mark),
        }
    }

    pub fn mark(&self, label: &str) {
        (self.mark)(label);
    }
}

impl std::fmt::Debug for NativeStartupTraceMarker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeStartupTraceMarker")
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct NativeExtensionPackageRootLoader {
    load: Arc<NativeExtensionPackageRootLoadFn>,
}

type NativeExtensionPackageRootLoadFn = dyn Fn() -> Vec<crate::ExtensionPackageRoot> + Send + Sync;

impl NativeExtensionPackageRootLoader {
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
    loader: Option<&NativeExtensionPackageRootLoader>,
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
    startup_trace: Option<NativeStartupTraceMarker>,
    scan_scheduled: &mut bool,
) {
    if *scan_scheduled {
        return;
    }
    *scan_scheduled = true;
    mark_extension_scan(startup_trace.as_ref(), "extension_manifest_scan_scheduled");
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("extension_manifest_scan_scheduled"),
    }));

    let tx = tx.clone();
    tokio::spawn(async move {
        mark_extension_scan(startup_trace.as_ref(), "extension_manifest_scan_started");
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: String::from("extension_manifest_scan_started"),
        }));

        let scan = tokio::task::spawn_blocking(move || {
            crate::ExtensionManifestIndex::from_package_roots(package_roots)
        })
        .await;
        match scan {
            Ok(Ok(index)) => {
                mark_extension_scan(startup_trace.as_ref(), "extension_manifest_scan_finished");
                let extension_count = index.records().len();
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
                    startup_trace.clone(),
                );
            }
            Ok(Err(error)) => {
                mark_extension_scan(startup_trace.as_ref(), "extension_manifest_scan_failed");
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!(
                        "extension_manifest_scan_failed reason={}",
                        extension_manifest_scan_error_label(&error)
                    ),
                }));
            }
            Err(_) => {
                mark_extension_scan(startup_trace.as_ref(), "extension_manifest_scan_failed");
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: String::from("extension_manifest_scan_failed reason=join_failed"),
                }));
            }
        }
    });
}

pub(super) async fn extension_static_context_files_from_scan_state(
    scan_state: &ExtensionManifestScanState,
) -> Vec<NativeExtensionStaticContextFile> {
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
    startup_trace: Option<NativeStartupTraceMarker>,
) {
    mark_extension_scan(
        startup_trace.as_ref(),
        "extension_background_activation_scheduled",
    );
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("extension_background_activation_scheduled"),
    }));

    let tx = tx.clone();
    tokio::spawn(async move {
        mark_extension_scan(
            startup_trace.as_ref(),
            "extension_background_activation_started",
        );
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: String::from("extension_background_activation_started"),
        }));
        let activation = tokio::task::spawn_blocking(move || {
            activate_background_metadata_extensions(
                &package_records,
                crate::ExtensionBackgroundActivationConfig::conservative(),
            )
        })
        .await;

        if let Ok(snapshot) = activation {
            mark_extension_scan(
                startup_trace.as_ref(),
                "extension_background_activation_finished",
            );
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
                *active_snapshot = snapshot;
            }
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!(
                        "extension_background_activation_finished active_extension_count={active_extension_count} registered_tool_count={registered_tool_count} host_start_count={host_start_count}"
                    ),
                }));
        } else {
            mark_extension_scan(
                startup_trace.as_ref(),
                "extension_background_activation_failed",
            );
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

    if action == ExtensionLifecycleAction::Reload {
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
        schedule_native_extension_reload(
            tx.clone(),
            activation_state.clone(),
            request_id,
            selector,
            record,
        );
        return;
    }

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
            ExtensionLifecycleAction::Reload => {
                unreachable!("reload is scheduled before snapshot lock");
            }
        }
    };

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
            &snapshot.reload_extension_from_record(
                &record,
                crate::ExtensionBackgroundActivationConfig::conservative(),
            ),
            &selector,
        );
        let _ = tx.send(BackendEvent::Server(
            ServerEvent::ExtensionLifecycleFinished {
                request_id,
                action: ExtensionLifecycleAction::Reload,
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

fn mark_extension_scan(trace: Option<&NativeStartupTraceMarker>, label: &str) {
    if let Some(trace) = trace {
        trace.mark(label);
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
