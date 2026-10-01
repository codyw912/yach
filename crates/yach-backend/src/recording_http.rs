//! Yach-owned HTTP transport that records a bounded provider request id,
//! first-event time, and an opt-in exact request body per attempt.
//!
//! Capture writes only to the directory named by `YACH_CAPTURE_REQUESTS`.
//! Request bodies and headers never enter session JSONL, protocol frames,
//! `ProviderError`, or status output.

use std::io::Write as _;

use futures::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bytes::Bytes;
use rig::http_client::sse::BoxedStream;
use rig::http_client::{
    BoundedErrorBody, Error, HeaderMap, HttpClientExt, LazyBody, MultipartForm, Request, Response,
    StreamingResponse, retry_after_from_headers,
};

use crate::{AttemptDiagnostics, AttemptLabel};

const REQUEST_ID_MAX_BYTES: usize = 128;

fn process_capture_policy() -> CapturePolicy {
    use std::sync::LazyLock;
    static POLICY: LazyLock<CapturePolicy> = LazyLock::new(CapturePolicy::isolated);
    POLICY.clone()
}

#[derive(Debug)]
struct RecorderState {
    created: Instant,
    capture_path: Option<PathBuf>,
    provider_request_id: Option<String>,
    first_event_ms: Option<u64>,
    capture: Option<String>,
}

impl Default for RecorderState {
    fn default() -> Self {
        Self {
            created: Instant::now(),
            capture_path: None,
            provider_request_id: None,
            first_event_ms: None,
            capture: None,
        }
    }
}

/// Process-wide or test-local capture disable. A failure disables later
/// captures that share this policy and warns once.
#[derive(Debug, Clone)]
pub(crate) struct CapturePolicy {
    disabled: Arc<AtomicBool>,
    warned: Arc<AtomicBool>,
}

impl CapturePolicy {
    pub(crate) fn process() -> Self {
        process_capture_policy()
    }

    pub(crate) fn isolated() -> Self {
        Self {
            disabled: Arc::new(AtomicBool::new(false)),
            warned: Arc::new(AtomicBool::new(false)),
        }
    }

    fn disable(&self, error: &std::io::Error) {
        self.disabled.store(true, Ordering::Relaxed);
        self.warn_once(&format!("request capture disabled: {error}"));
    }

    #[cfg(test)]
    pub(crate) fn warned(&self) -> bool {
        self.warned.load(Ordering::Relaxed)
    }

    fn warn_once(&self, message: &str) {
        if self
            .warned
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            let _ = writeln!(std::io::stderr(), "{message}");
        }
    }
}

/// Opaque per-attempt recorder. `pub` because `CompactionPreparation` is a
/// public struct that `yach-cli` constructs; fields and methods stay crate-private.
#[derive(Debug, Clone)]
pub struct AttemptRecorder {
    state: Arc<Mutex<RecorderState>>,
    policy: CapturePolicy,
}

impl Default for AttemptRecorder {
    fn default() -> Self {
        Self::new(None)
    }
}

impl AttemptRecorder {
    pub(crate) fn new(capture_path: Option<PathBuf>) -> Self {
        Self::with_policy(capture_path, CapturePolicy::process())
    }

    pub(crate) fn with_policy(capture_path: Option<PathBuf>, policy: CapturePolicy) -> Self {
        Self {
            state: Arc::new(Mutex::new(RecorderState {
                capture_path,
                ..RecorderState::default()
            })),
            policy,
        }
    }

    pub(crate) fn record_response_headers(&self, headers: &HeaderMap) {
        let Some(request_id) = bounded_request_id(headers) else {
            return;
        };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.provider_request_id = Some(request_id);
    }

    pub(crate) fn capture_body(&self, body: &[u8]) {
        if self.policy.disabled.load(Ordering::Relaxed) {
            return;
        }
        let path = {
            let Ok(state) = self.state.lock() else {
                return;
            };
            let Some(path) = state.capture_path.clone() else {
                return;
            };
            path
        };
        match write_capture_file(&path, body) {
            Ok(()) => {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                state.capture = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(String::from);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => self.policy.disable(&error),
        }
    }

    pub(crate) fn mark_first_event(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.first_event_ms.is_some() {
            return;
        }
        state.first_event_ms = u64::try_from(state.created.elapsed().as_millis())
            .ok()
            .or(Some(u64::MAX));
    }

    pub(crate) fn diagnostics(&self) -> AttemptDiagnostics {
        let Ok(state) = self.state.lock() else {
            return AttemptDiagnostics::default();
        };
        AttemptDiagnostics {
            provider_request_id: state.provider_request_id.clone(),
            first_event_ms: state.first_event_ms,
            capture: state.capture.clone(),
        }
    }
}

/// `Clone + Default + Debug` as Rig's completion models require; the default
/// holds an empty recorder that records nothing.
#[derive(Debug, Clone)]
pub(crate) struct RecordingHttpClient {
    client: reqwest::Client,
    recorder: AttemptRecorder,
}

impl Default for RecordingHttpClient {
    fn default() -> Self {
        Self::new(AttemptRecorder::default())
    }
}

impl RecordingHttpClient {
    pub(crate) fn new(recorder: AttemptRecorder) -> Self {
        Self {
            client: reqwest::Client::new(),
            recorder,
        }
    }
}

impl HttpClientExt for RecordingHttpClient {
    fn send<T, U>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = rig::http_client::Result<Response<LazyBody<U>>>> + Send + 'static
    where
        T: Into<Bytes>,
        T: Send,
        U: From<Bytes>,
        U: Send + 'static,
    {
        let client = self.client.clone();
        let recorder = self.recorder.clone();
        let (parts, body) = req.into_parts();
        let bytes = body.into();
        recorder.capture_body(&bytes);
        async move {
            let response = client
                .request(parts.method, parts.uri.to_string())
                .headers(parts.headers)
                .body(bytes)
                .send()
                .await
                .map_err(|error| Error::Instance(Box::new(error)))?;
            recorder.record_response_headers(response.headers());
            into_recorded_response(response).await
        }
    }

    fn send_multipart<U>(
        &self,
        req: Request<MultipartForm>,
    ) -> impl Future<Output = rig::http_client::Result<Response<LazyBody<U>>>> + Send + 'static
    where
        U: From<Bytes>,
        U: Send + 'static,
    {
        let client = self.client.clone();
        let (parts, body) = req.into_parts();
        let body = reqwest::multipart::Form::from(body);
        async move {
            let response = client
                .request(parts.method, parts.uri.to_string())
                .headers(parts.headers)
                .multipart(body)
                .send()
                .await
                .map_err(|error| Error::Instance(Box::new(error)))?;
            into_recorded_response(response).await
        }
    }

    fn send_streaming<T>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = rig::http_client::Result<StreamingResponse>> + Send
    where
        T: Into<Bytes> + Send,
    {
        let client = self.client.clone();
        let recorder = self.recorder.clone();
        let (parts, body) = req.into_parts();
        let bytes = body.into();
        recorder.capture_body(&bytes);
        async move {
            let request = client
                .request(parts.method, parts.uri.to_string())
                .headers(parts.headers)
                .body(bytes)
                .build()
                .map_err(|error| Error::Instance(Box::new(error)))?;
            let response = client
                .execute(request)
                .await
                .map_err(|error| Error::Instance(Box::new(error)))?;
            recorder.record_response_headers(response.headers());
            if !response.status().is_success() {
                return Err(recorded_status_error(response).await);
            }
            let mut builder = Response::builder()
                .status(response.status())
                .version(response.version());
            if let Some(headers) = builder.headers_mut() {
                *headers = response.headers().clone();
            }
            let mapped: BoxedStream = Box::pin(
                response
                    .bytes_stream()
                    .map(|chunk| chunk.map_err(|error| Error::Instance(Box::new(error)))),
            );
            builder.body(mapped).map_err(Error::Protocol)
        }
    }
}

async fn into_recorded_response<U>(
    response: reqwest::Response,
) -> rig::http_client::Result<Response<LazyBody<U>>>
where
    U: From<Bytes> + Send + 'static,
{
    if !response.status().is_success() {
        return Err(recorded_status_error(response).await);
    }
    let mut builder = Response::builder().status(response.status());
    if let Some(headers) = builder.headers_mut() {
        *headers = response.headers().clone();
    }
    let body: LazyBody<U> = Box::pin(async move {
        let bytes = response
            .bytes()
            .await
            .map_err(|error| Error::Instance(Box::new(error)))?;
        Ok(U::from(bytes))
    });
    builder.body(body).map_err(Error::Protocol)
}

async fn recorded_status_error(mut response: reqwest::Response) -> Error {
    let status = response.status();
    let retry_after = retry_after_from_headers(response.headers());
    let mut chunks: Vec<bytes::Bytes> = Vec::new();
    let mut buffered = 0_usize;
    let body = loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                buffered = buffered.saturating_add(chunk.len());
                chunks.push(chunk);
                if buffered > rig::http_client::ERROR_BODY_MAX_BYTES {
                    drop(response);
                    break BoundedErrorBody::from_chunks(&chunks);
                }
            }
            Ok(None) => break BoundedErrorBody::from_chunks(&chunks),
            Err(_) => break BoundedErrorBody::from_slice(b""),
        }
    };
    let message = body.into_string();
    Error::InvalidStatusCodeWithMessage(status, message, retry_after)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CaptureDir {
    Unset,
    Absolute(PathBuf),
    Rejected,
}

pub(crate) fn capture_dir_from_env() -> Option<PathBuf> {
    match capture_dir_from_value(std::env::var_os("YACH_CAPTURE_REQUESTS").as_deref()) {
        CaptureDir::Absolute(path) => Some(path),
        CaptureDir::Unset => None,
        CaptureDir::Rejected => {
            CapturePolicy::process()
                .warn_once("YACH_CAPTURE_REQUESTS must be an absolute path; capture disabled");
            None
        }
    }
}

/// Side-effect free. A relative path is `Rejected`; the caller warns.
pub(crate) fn capture_dir_from_value(value: Option<&std::ffi::OsStr>) -> CaptureDir {
    let Some(value) = value else {
        return CaptureDir::Unset;
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        CaptureDir::Absolute(path)
    } else {
        CaptureDir::Rejected
    }
}

/// `<dir>/<session-id>/<turn-id>-<purpose>-<attempt_sequence>.json`
pub(crate) fn capture_path(dir: &Path, label: &AttemptLabel) -> PathBuf {
    let purpose = match label.purpose {
        crate::ProviderAttemptPurpose::Turn => "turn",
        crate::ProviderAttemptPurpose::CompactionSummary => "compaction_summary",
        crate::ProviderAttemptPurpose::CompactionNative => "compaction_native",
    };
    dir.join(label.session_id.0.as_str()).join(format!(
        "{}-{purpose}-{}.json",
        label.turn_id.0, label.attempt_sequence
    ))
}

pub(crate) fn bounded_request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .or_else(|| headers.get("request-id"))
        .and_then(|value| accepted_request_id(value.as_bytes()))
}

fn accepted_request_id(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > REQUEST_ID_MAX_BYTES {
        return None;
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

fn write_capture_file(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(session_dir) = path.parent() {
        if !session_dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(session_dir)?;
        }
        // Only the session directory Yach created is forced private. An
        // existing capture root keeps the mode the caller chose.
        std::fs::set_permissions(session_dir, private_dir())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(body)?;
    file.set_permissions(private_file())?;
    Ok(())
}

#[cfg(unix)]
fn private_dir() -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::Permissions::from_mode(0o700)
}

#[cfg(unix)]
fn private_file() -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::Permissions::from_mode(0o600)
}

#[cfg(not(unix))]
fn private_dir() -> std::fs::Permissions {
    std::fs::Permissions::from(std::fs::FilePermissions::new())
}

#[cfg(not(unix))]
fn private_file() -> std::fs::Permissions {
    std::fs::Permissions::from(std::fs::FilePermissions::new())
}

trait DirBuilderExt {
    fn mode(&mut self, mode: u32) -> &mut Self;
}

#[cfg(unix)]
impl DirBuilderExt for std::fs::DirBuilder {
    fn mode(&mut self, mode: u32) -> &mut Self {
        <Self as std::os::unix::fs::DirBuilderExt>::mode(self, mode)
    }
}

#[cfg(not(unix))]
impl DirBuilderExt for std::fs::DirBuilder {
    fn mode(&mut self, _mode: u32) -> &mut Self {
        self
    }
}

trait OpenOptionsExt {
    fn mode(&mut self, mode: u32) -> &mut Self;
}

#[cfg(unix)]
impl OpenOptionsExt for std::fs::OpenOptions {
    fn mode(&mut self, mode: u32) -> &mut Self {
        <Self as std::os::unix::fs::OpenOptionsExt>::mode(self, mode)
    }
}

#[cfg(not(unix))]
impl OpenOptionsExt for std::fs::OpenOptions {
    fn mode(&mut self, _mode: u32) -> &mut Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use futures::StreamExt;
    use rig::http_client::HttpClientExt;

    use super::{
        AttemptRecorder, CaptureDir, CapturePolicy, RecordingHttpClient, bounded_request_id,
        capture_dir_from_value, capture_path,
    };
    use crate::{AttemptLabel, ProviderAttemptPurpose, SessionId, TurnId};

    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
            "yach-capture-{label}-{}-{}-{}",
            std::process::id(),
            FIXTURE_SEQ.fetch_add(1, Ordering::Relaxed),
            unique
        ))
    }

    fn attempt_label(purpose: ProviderAttemptPurpose, sequence: u64) -> AttemptLabel {
        AttemptLabel {
            session_id: SessionId(String::from("session-1")),
            turn_id: TurnId(String::from("turn-1")),
            purpose,
            attempt_sequence: sequence,
        }
    }

    struct LocalHttpFixture {
        url: String,
        server: std::thread::JoinHandle<()>,
    }

    fn local_http_fixture(response: Vec<u8>, hold_open: bool) -> LocalHttpFixture {
        let listener = TcpListener::bind("127.0.0.1:0");
        assert!(listener.is_ok());
        let Ok(listener) = listener else {
            unreachable!("fixture listener should bind");
        };
        let address = listener.local_addr();
        assert!(address.is_ok());
        let Ok(address) = address else {
            unreachable!("fixture listener should have an address");
        };
        let server = std::thread::spawn(move || {
            let accepted = listener.accept();
            let Ok((mut socket, _)) = accepted else {
                return;
            };
            let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            let header_end = loop {
                let read = socket.read(&mut buffer);
                let Ok(read) = read else {
                    return;
                };
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let header = String::from_utf8_lossy(&request[..header_end]);
            let content_length = header
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while request.len() < header_end.saturating_add(content_length) {
                let read = socket.read(&mut buffer);
                let Ok(read) = read else {
                    return;
                };
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let _ = socket.write_all(&response);
            if hold_open {
                std::thread::park();
            }
        });
        LocalHttpFixture {
            url: format!("http://{address}/v1/stream"),
            server,
        }
    }

    fn http_response(status_line: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "{status_line}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn post_bytes(url: &str, body: &'static [u8]) -> rig::http_client::Request<Vec<u8>> {
        let request = rig::http_client::Request::post(url).body(body.to_vec());
        assert!(request.is_ok());
        let Ok(request) = request else {
            unreachable!("fixture request should build");
        };
        request
    }

    #[test]
    fn request_id_is_bounded_and_charset_checked() {
        let mut headers = rig::http_client::HeaderMap::new();
        headers.insert(
            "x-request-id",
            rig::http_client::HeaderValue::from_static("req_01-ab.c:9"),
        );
        assert_eq!(
            bounded_request_id(&headers).as_deref(),
            Some("req_01-ab.c:9")
        );

        let mut fallback = rig::http_client::HeaderMap::new();
        fallback.insert(
            "request-id",
            rig::http_client::HeaderValue::from_static("abc"),
        );
        assert_eq!(bounded_request_id(&fallback).as_deref(), Some("abc"));

        let mut bad = rig::http_client::HeaderMap::new();
        bad.insert(
            "x-request-id",
            rig::http_client::HeaderValue::from_static("a b"),
        );
        assert_eq!(bounded_request_id(&bad), None);

        let long = "a".repeat(129);
        let mut too_long = rig::http_client::HeaderMap::new();
        too_long.insert(
            "x-request-id",
            rig::http_client::HeaderValue::from_str(&long)
                .unwrap_or_else(|_| rig::http_client::HeaderValue::from_static("")),
        );
        assert_eq!(bounded_request_id(&too_long), None);
    }

    #[test]
    fn preferred_request_id_wins_and_invalid_preferred_is_not_replaced() {
        let mut both = rig::http_client::HeaderMap::new();
        both.insert(
            "x-request-id",
            rig::http_client::HeaderValue::from_static("preferred"),
        );
        both.insert(
            "request-id",
            rig::http_client::HeaderValue::from_static("fallback"),
        );
        assert_eq!(bounded_request_id(&both).as_deref(), Some("preferred"));

        let mut invalid_preferred = rig::http_client::HeaderMap::new();
        invalid_preferred.insert(
            "x-request-id",
            rig::http_client::HeaderValue::from_static("has space"),
        );
        invalid_preferred.insert(
            "request-id",
            rig::http_client::HeaderValue::from_static("fallback"),
        );
        assert_eq!(bounded_request_id(&invalid_preferred), None);

        let exact = "b".repeat(128);
        let mut at_bound = rig::http_client::HeaderMap::new();
        at_bound.insert(
            "request-id",
            rig::http_client::HeaderValue::from_str(&exact)
                .unwrap_or_else(|_| rig::http_client::HeaderValue::from_static("")),
        );
        assert_eq!(
            bounded_request_id(&at_bound).as_deref(),
            Some(exact.as_str())
        );
    }

    #[tokio::test]
    async fn streaming_success_keeps_body_and_records_request_id() {
        let body = b"data: [DONE]\n\n";
        let fixture = local_http_fixture(
            http_response(
                "HTTP/1.1 200 OK",
                "Content-Type: text/event-stream\r\nx-request-id: gw-1\r\n",
                body,
            ),
            false,
        );
        let recorder = AttemptRecorder::new(None);
        let client = RecordingHttpClient::new(recorder.clone());
        let response = client
            .send_streaming(post_bytes(&fixture.url, b"{\"prompt\":\"hi\"}"))
            .await;
        assert!(response.is_ok());
        let Ok(response) = response else {
            return;
        };
        let collected = response.into_body().collect::<Vec<_>>().await;
        let mut streamed = Vec::new();
        for chunk in collected {
            let Ok(chunk) = chunk else {
                unreachable!("stream chunk should be bytes");
            };
            streamed.extend_from_slice(&chunk);
        }
        assert_eq!(streamed, body);
        assert_eq!(
            recorder.diagnostics().provider_request_id.as_deref(),
            Some("gw-1")
        );
        drop(fixture.server);
    }

    #[tokio::test]
    async fn error_status_matches_rig_reqwest_and_records_fallback_request_id() {
        let body = br#"{"error":"unavailable"}"#;
        let response_bytes = http_response(
            "HTTP/1.1 503 Service Unavailable",
            "Content-Type: application/json\r\nrequest-id: gw-2\r\nRetry-After: 2\r\n",
            body,
        );
        let recording_fixture = local_http_fixture(response_bytes.clone(), false);
        let recorder = AttemptRecorder::new(None);
        let recording = RecordingHttpClient::new(recorder.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&recording_fixture.url, b"{\"n\":1}"))
            .await;
        let rig_fixture = local_http_fixture(response_bytes, false);
        let rig = reqwest::Client::new()
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&rig_fixture.url, b"{\"n\":1}"))
            .await;

        assert!(matches!(
            &recording,
            Err(rig::http_client::Error::InvalidStatusCodeWithMessage(
                status,
                message,
                Some(_)
            )) if status.as_u16() == 503 && message.as_bytes() == body
        ));
        match (&recording, &rig) {
            (
                Err(rig::http_client::Error::InvalidStatusCodeWithMessage(_, recorded, _)),
                Err(rig::http_client::Error::InvalidStatusCodeWithMessage(_, expected, _)),
            ) => assert_eq!(recorded, expected),
            _ => unreachable!("both clients should return the same status error"),
        }
        assert_eq!(
            recorder.diagnostics().provider_request_id.as_deref(),
            Some("gw-2")
        );
    }

    #[tokio::test]
    async fn oversized_error_body_matches_rig_sentinel_without_waiting() {
        let prefix = vec![b'z'; rig::http_client::ERROR_BODY_MAX_BYTES + 64];
        let mut response = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            prefix.len().saturating_add(64)
        )
        .into_bytes();
        response.extend_from_slice(&prefix);
        let recording_fixture = local_http_fixture(response.clone(), true);
        let recording = tokio::time::timeout(
            Duration::from_secs(2),
            RecordingHttpClient::new(AttemptRecorder::new(None))
                .send::<Vec<u8>, Vec<u8>>(post_bytes(&recording_fixture.url, b"{}")),
        )
        .await;
        assert!(recording.is_ok(), "bounded error read must not wait");
        let Ok(recording) = recording else {
            return;
        };
        let rig_fixture = local_http_fixture(response, true);
        let rig = tokio::time::timeout(
            Duration::from_secs(2),
            reqwest::Client::new().send::<Vec<u8>, Vec<u8>>(post_bytes(&rig_fixture.url, b"{}")),
        )
        .await;
        assert!(rig.is_ok(), "rig reqwest path must not wait either");
        let Ok(rig) = rig else {
            return;
        };
        match (&recording, &rig) {
            (
                Err(rig::http_client::Error::InvalidStatusCodeWithMessage(_, recorded, _)),
                Err(rig::http_client::Error::InvalidStatusCodeWithMessage(_, expected, _)),
            ) => {
                assert_eq!(recorded, expected);
                assert_eq!(recorded.len(), rig::http_client::TRUNCATED_ERROR_BODY_LEN);
                assert!(!recorded.contains('z'));
            }
            _ => unreachable!("both clients should truncate the oversized error body"),
        }
    }

    fn file_mode(path: &Path) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let metadata = std::fs::metadata(path);
            assert!(metadata.is_ok());
            let Ok(metadata) = metadata else {
                return 0;
            };
            metadata.permissions().mode() & 0o777
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            0o600
        }
    }

    #[tokio::test]
    async fn capture_writes_exact_body_with_private_mode_and_names_the_file() {
        let dir = unique_dir("write");
        let path = capture_path(&dir, &attempt_label(ProviderAttemptPurpose::Turn, 3));
        let body = b"{\"model\":\"fixture\",\"input\":\"exact\"}";
        let fixture = local_http_fixture(
            http_response("HTTP/1.1 200 OK", "x-request-id: gw-3\r\n", b"ok"),
            false,
        );
        let recorder = AttemptRecorder::with_policy(Some(path.clone()), CapturePolicy::isolated());
        let response = RecordingHttpClient::new(recorder.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&fixture.url, body))
            .await;
        assert!(response.is_ok());
        let written = std::fs::read(&path);
        assert!(written.is_ok());
        let Ok(written) = written else {
            return;
        };
        assert_eq!(written, body);
        assert_eq!(file_mode(&path), 0o600);
        assert_eq!(file_mode(path.parent().unwrap_or(&dir)), 0o700);
        assert_eq!(
            recorder.diagnostics().capture.as_deref(),
            path.file_name().and_then(|name| name.to_str())
        );

        let silent = AttemptRecorder::new(None);
        let second = local_http_fixture(http_response("HTTP/1.1 200 OK", "", b"ok"), false);
        let response = RecordingHttpClient::new(silent.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&second.url, body))
            .await;
        assert!(response.is_ok());
        assert!(silent.diagnostics().capture.is_none());
        let entries = std::fs::read_dir(dir.join("session-1"));
        assert!(entries.is_ok());
        let Ok(entries) = entries else {
            return;
        };
        assert_eq!(entries.count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn existing_capture_file_is_not_overwritten_and_later_capture_still_works() {
        let dir = unique_dir("collision");
        let first = capture_path(&dir, &attempt_label(ProviderAttemptPurpose::Turn, 1));
        assert!(std::fs::create_dir_all(first.parent().unwrap_or(&dir)).is_ok());
        assert!(std::fs::write(&first, b"original").is_ok());
        let fixture = local_http_fixture(http_response("HTTP/1.1 200 OK", "", b"ok"), false);
        let recorder = AttemptRecorder::with_policy(Some(first.clone()), CapturePolicy::isolated());
        let response = RecordingHttpClient::new(recorder.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&fixture.url, b"new-body"))
            .await;
        assert!(response.is_ok());
        assert!(recorder.diagnostics().capture.is_none());
        assert_eq!(
            std::fs::read(&first).ok().as_deref(),
            Some(b"original".as_slice())
        );

        let second_path = capture_path(&dir, &attempt_label(ProviderAttemptPurpose::Turn, 2));
        let next = local_http_fixture(http_response("HTTP/1.1 200 OK", "", b"ok"), false);
        let next_recorder =
            AttemptRecorder::with_policy(Some(second_path.clone()), CapturePolicy::isolated());
        let response = RecordingHttpClient::new(next_recorder.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&next.url, b"second-body"))
            .await;
        assert!(response.is_ok());
        assert_eq!(
            std::fs::read(&second_path).ok().as_deref(),
            Some(b"second-body".as_slice())
        );
        assert!(next_recorder.diagnostics().capture.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unwritable_capture_dir_disables_only_that_recorders_policy() {
        let blocked = unique_dir("blocked");
        assert!(std::fs::create_dir_all(&blocked).is_ok());
        let file_as_dir = blocked.join("not-a-directory");
        assert!(std::fs::write(&file_as_dir, b"file").is_ok());
        let path = file_as_dir.join("session-1").join("turn-1-turn-1.json");
        let policy = CapturePolicy::isolated();
        let fixture = local_http_fixture(http_response("HTTP/1.1 200 OK", "", b"ok"), false);
        let recorder = AttemptRecorder::with_policy(Some(path), policy.clone());
        let response = RecordingHttpClient::new(recorder.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&fixture.url, b"body"))
            .await;
        assert!(response.is_ok());
        assert!(recorder.diagnostics().capture.is_none());

        let later_dir = unique_dir("after-disable");
        let later = capture_path(&later_dir, &attempt_label(ProviderAttemptPurpose::Turn, 9));
        let next = local_http_fixture(http_response("HTTP/1.1 200 OK", "", b"ok"), false);
        let later_recorder = AttemptRecorder::with_policy(Some(later.clone()), policy);
        let response = RecordingHttpClient::new(later_recorder.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&next.url, b"later"))
            .await;
        assert!(response.is_ok());
        assert!(later_recorder.diagnostics().capture.is_none());
        assert!(!later.exists());

        let other = unique_dir("other-policy");
        let other_path = capture_path(&other, &attempt_label(ProviderAttemptPurpose::Turn, 1));
        let other_fixture = local_http_fixture(http_response("HTTP/1.1 200 OK", "", b"ok"), false);
        let other_recorder =
            AttemptRecorder::with_policy(Some(other_path.clone()), CapturePolicy::isolated());
        let response = RecordingHttpClient::new(other_recorder.clone())
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&other_fixture.url, b"still-captured"))
            .await;
        assert!(response.is_ok());
        assert_eq!(
            std::fs::read(&other_path).ok().as_deref(),
            Some(b"still-captured".as_slice())
        );
        let _ = std::fs::remove_dir_all(&blocked);
        let _ = std::fs::remove_dir_all(&later_dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn capture_path_includes_purpose_so_turn_and_summary_do_not_collide() {
        let dir = Path::new("/tmp/yach-capture");
        let turn = capture_path(dir, &attempt_label(ProviderAttemptPurpose::Turn, 4));
        let summary = capture_path(
            dir,
            &attempt_label(ProviderAttemptPurpose::CompactionSummary, 4),
        );
        assert_ne!(turn, summary);
        assert_eq!(
            turn.file_name().and_then(|name| name.to_str()),
            Some("turn-1-turn-4.json")
        );
        assert_eq!(
            summary.file_name().and_then(|name| name.to_str()),
            Some("turn-1-compaction_summary-4.json")
        );
        assert_eq!(
            turn.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some("session-1")
        );
    }

    #[test]
    fn relative_capture_dir_is_rejected_without_warning() {
        let policy = CapturePolicy::isolated();
        assert_eq!(
            capture_dir_from_value(Some(std::ffi::OsStr::new("relative/capture"))),
            CaptureDir::Rejected
        );
        assert!(!policy.warned());
        let absolute = std::env::temp_dir().join("yach-capture-absolute");
        assert_eq!(
            capture_dir_from_value(Some(absolute.as_os_str())),
            CaptureDir::Absolute(absolute.clone())
        );
        assert_eq!(capture_dir_from_value(None), CaptureDir::Unset);
        assert!(!policy.warned());
    }

    #[tokio::test]
    async fn existing_capture_root_keeps_its_mode() {
        let root = unique_dir("shared-root");
        assert!(std::fs::create_dir_all(&root).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).is_ok()
            );
        }
        let path = capture_path(&root, &attempt_label(ProviderAttemptPurpose::Turn, 1));
        let fixture = local_http_fixture(http_response("HTTP/1.1 200 OK", "", b"ok"), false);
        let recorder = AttemptRecorder::with_policy(Some(path.clone()), CapturePolicy::isolated());
        let response = RecordingHttpClient::new(recorder)
            .send::<Vec<u8>, Vec<u8>>(post_bytes(&fixture.url, b"kept-root"))
            .await;
        assert!(response.is_ok());
        assert_eq!(file_mode(&root), 0o755);
        assert_eq!(file_mode(path.parent().unwrap_or(&root)), 0o700);
        assert_eq!(file_mode(&path), 0o600);
        let _ = std::fs::remove_dir_all(&root);
    }
}
