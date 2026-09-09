use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use yach_backend::{
    DenyExtensionResources, ExtensionHostClientMessage, ExtensionHostProtocolError,
    ExtensionHostServerMessage, ExtensionHostSession, ExtensionHostTransport,
    ExtensionToolResultStatus, ExtensionToolRisk, ToolInputSchema, ToolRegistry,
};

use crate::perf::registry::{Measured, Workload};
use crate::perf::schema::{Class, Isolation};

pub static EXTENSION: [Workload; 2] = [
    Workload {
        id: "extension_runtime/metadata_host_activation",
        class: Class::Latency,
        isolation: Isolation::InProcessThreaded,
        requires: &[],
        bin: None,
        run: |ctx| run_extension_phase(ctx.samples, ExtensionPhase::HostActivation),
    },
    Workload {
        id: "extension_runtime/metadata_tool_invocation_round_trip",
        class: Class::Latency,
        isolation: Isolation::InProcessThreaded,
        requires: &[],
        bin: None,
        run: |ctx| run_extension_phase(ctx.samples, ExtensionPhase::ToolInvocation),
    },
];

#[derive(Clone, Copy)]
enum ExtensionPhase {
    HostActivation,
    ToolInvocation,
}

struct ExtensionRuntimeProfileSample {
    host_activation: Duration,
    metadata_tool_invocation: Duration,
}

fn run_extension_phase(samples: usize, phase: ExtensionPhase) -> Result<Measured, String> {
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let sample = sample_extension_runtime_profile().map_err(|error| error.to_string())?;
        durations.push(match phase {
            ExtensionPhase::HostActivation => sample.host_activation,
            ExtensionPhase::ToolInvocation => sample.metadata_tool_invocation,
        });
    }
    Ok(Measured::Latency {
        samples: durations,
        alloc: None,
    })
}

fn sample_extension_runtime_profile() -> io::Result<ExtensionRuntimeProfileSample> {
    let transport = BenchExtensionHostTransport::new([
        Ok(ExtensionHostServerMessage::Ready {
            protocol: String::from("yach.extension-host.v2"),
            extension_id: String::from("example.profile-tools"),
        }),
        Ok(ExtensionHostServerMessage::ToolRegister {
            name: String::from("profile_toy_tool"),
            description: String::from("Return static fixture metadata."),
            risk: ExtensionToolRisk::ReadsLocalMetadata,
            provider_visible: true,
            input_schema: ToolInputSchema::string_object(
                ["label"],
                std::iter::empty::<&str>(),
                512,
            ),
        }),
        Ok(ExtensionHostServerMessage::ToolResult {
            request_id: String::from("profile-request-1"),
            content: String::from(r#"{"ok":true,"label":"profile"}"#),
            status: ExtensionToolResultStatus::Completed,
            reason: None,
        }),
    ]);
    let mut session = ExtensionHostSession::new("example.profile-tools", transport, 4096);
    let mut registry = ToolRegistry::default();

    let activation_started = Instant::now();
    session
        .initialize_and_register(&mut registry, None, 1, Duration::from_secs(1))
        .map_err(|error| extension_profile_io_error(&error))?;
    let host_activation = activation_started.elapsed();

    let invocation_started = Instant::now();
    session
        .invoke_tool(
            "profile-request-1",
            "profile_toy_tool",
            serde_json::json!({"label": "profile"}),
            Duration::from_secs(1),
            &DenyExtensionResources,
        )
        .map_err(|error| extension_profile_io_error(&error))?;
    let metadata_tool_invocation = invocation_started.elapsed();

    Ok(ExtensionRuntimeProfileSample {
        host_activation,
        metadata_tool_invocation,
    })
}

fn extension_profile_io_error(error: &ExtensionHostProtocolError) -> io::Error {
    io::Error::other(format!("extension runtime profile failed: {error:?}"))
}

struct BenchExtensionHostTransport {
    received: VecDeque<Result<ExtensionHostServerMessage, ExtensionHostProtocolError>>,
}

impl BenchExtensionHostTransport {
    fn new(
        received: impl IntoIterator<
            Item = Result<ExtensionHostServerMessage, ExtensionHostProtocolError>,
        >,
    ) -> Self {
        Self {
            received: received.into_iter().collect(),
        }
    }
}

impl ExtensionHostTransport for BenchExtensionHostTransport {
    fn send(
        &mut self,
        _message: ExtensionHostClientMessage,
    ) -> Result<(), ExtensionHostProtocolError> {
        Ok(())
    }

    fn recv(
        &mut self,
        _timeout: Duration,
    ) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError> {
        self.received
            .pop_front()
            .unwrap_or(Err(ExtensionHostProtocolError::TimedOut))
    }
}

#[cfg(test)]
mod tests {
    use super::EXTENSION;
    use crate::perf::registry::{Measured, RunCtx};

    #[test]
    fn extension_runtime_workloads_emit_activation_and_invocation_samples() -> Result<(), String> {
        let ctx = RunCtx {
            samples: 1,
            yach_bin: None,
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: None,
        };
        for id in [
            "extension_runtime/metadata_host_activation",
            "extension_runtime/metadata_tool_invocation_round_trip",
        ] {
            let workload = EXTENSION
                .iter()
                .find(|workload| workload.id == id)
                .ok_or_else(|| format!("missing {id}"))?;
            match (workload.run)(&ctx) {
                Ok(Measured::Latency { samples, .. }) => {
                    if samples.len() != 1 {
                        return Err(format!("{id} expected 1 sample, got {}", samples.len()));
                    }
                }
                Ok(_) => return Err(format!("expected Latency from {id}")),
                Err(error) => return Err(format!("{id} failed: {error}")),
            }
        }
        Ok(())
    }
}
