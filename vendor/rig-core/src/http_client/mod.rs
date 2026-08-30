use crate::http_client::sse::BoxedStream;
use bytes::Bytes;
pub use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, request::Builder};
use http::HeaderName;
use reqwest::Body;
pub mod multipart;
pub mod retry;
pub mod sse;
use crate::wasm_compat::*;
pub use multipart::MultipartForm;
pub use reqwest::Client as ReqwestClient;
use std::pin::Pin;

/// Maximum accepted `Retry-After` header bytes. Longer values are ignored;
/// a bounded prefix is never interpreted as the complete server instruction.
pub const RETRY_AFTER_MAX_BYTES: usize = 128;

/// Maximum bytes retained from a non-success HTTP response body while it is
/// being read. Success bodies are not subject to this cap.
pub const ERROR_BODY_MAX_BYTES: usize = 16 * 1024;

/// Stored length of a truncated error body: one byte past
/// [`ERROR_BODY_MAX_BYTES`] so a bounded prefix cannot be treated as a
/// complete, classifiable payload.
pub const TRUNCATED_ERROR_BODY_LEN: usize = ERROR_BODY_MAX_BYTES + 1;

/// Result of consuming a non-success HTTP body with a byte cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedErrorBody {
    text: String,
    truncated: bool,
}

impl BoundedErrorBody {
    fn truncated() -> Self {
        Self {
            text: "\0".repeat(TRUNCATED_ERROR_BODY_LEN),
            truncated: true,
        }
    }

    fn complete(text: String) -> Self {
        Self {
            text,
            truncated: false,
        }
    }

    /// Consume chunks, never growing the capture buffer past
    /// [`ERROR_BODY_MAX_BYTES`]. Overflow discards the prefix, drops any
    /// remaining iterator items, and stores a sentinel of length
    /// [`TRUNCATED_ERROR_BODY_LEN`].
    pub fn from_chunks<I, B>(chunks: I) -> Self
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        match Self::try_from_chunks(chunks.into_iter().map(Ok::<_, std::convert::Infallible>)) {
            Ok(body) => body,
            Err(never) => match never {},
        }
    }

    /// Like [`Self::from_chunks`], but a transport error aborts with `Err` and
    /// does not return captured body bytes.
    pub fn try_from_chunks<I, B, E>(chunks: I) -> std::result::Result<Self, E>
    where
        I: IntoIterator<Item = std::result::Result<B, E>>,
        B: AsRef<[u8]>,
    {
        let mut buf = Vec::new();
        let mut chunks = chunks.into_iter();
        while let Some(chunk) = chunks.next() {
            let chunk = chunk?;
            if push_error_body_chunk(&mut buf, chunk.as_ref()).is_err() {
                drop(chunks);
                return Ok(Self::truncated());
            }
        }
        Ok(Self::complete(String::from_utf8_lossy(&buf).into_owned()))
    }

    /// Capture at most [`ERROR_BODY_MAX_BYTES`] from a complete slice.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self::from_chunks(std::iter::once(bytes))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.text
    }
}

fn push_error_body_chunk(buf: &mut Vec<u8>, chunk: &[u8]) -> std::result::Result<(), ()> {
    let remaining = ERROR_BODY_MAX_BYTES.saturating_sub(buf.len());
    if remaining == 0 || chunk.len() > remaining {
        buf.clear();
        return Err(());
    }
    buf.extend_from_slice(chunk);
    Ok(())
}

/// One bounded `Retry-After` header value.
///
/// Display and Debug omit the raw bytes so callers can keep the hint on an
/// error without leaking it into logs.
#[derive(Clone, PartialEq, Eq)]
pub struct BoundedRetryAfter(String);

impl BoundedRetryAfter {
    /// Captures a complete header value up to [`RETRY_AFTER_MAX_BYTES`].
    ///
    /// Returns `None` when the value is empty, oversized, or not valid UTF-8.
    #[must_use]
    pub fn from_header_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > RETRY_AFTER_MAX_BYTES {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        Some(Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for BoundedRetryAfter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Retry-After")
    }
}

impl std::fmt::Display for BoundedRetryAfter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Retry-After")
    }
}

#[derive(thiserror::Error)]
pub enum Error {
    #[error("Http error: {0}")]
    Protocol(#[from] http::Error),
    #[error("Invalid status code: {0}")]
    InvalidStatusCode(StatusCode),
    #[error("Invalid status code {0} with message")]
    InvalidStatusCodeWithMessage(StatusCode, String, Option<BoundedRetryAfter>),
    #[error("Header value outside of legal range: {0}")]
    InvalidHeaderValue(#[from] http::header::InvalidHeaderValue),
    #[error("Request in error state, cannot access headers")]
    NoHeaders,
    #[error("Stream ended")]
    StreamEnded,
    #[error("Invalid content type was returned: {0:?}")]
    InvalidContentType(HeaderValue),
    /// An SSE event exceeded [`sse::SSE_EVENT_MAX_BYTES`] before a complete
    /// event boundary. The event payload is not stored.
    #[error("SSE event exceeded the maximum size")]
    SseEventTooLarge,
    #[cfg(not(target_family = "wasm"))]
    #[error("Http client error: {0}")]
    Instance(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),

    #[cfg(target_family = "wasm")]
    #[error("Http client error: {0}")]
    Instance(#[from] Box<dyn std::error::Error + 'static>),
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(error) => f.debug_tuple("Protocol").field(error).finish(),
            Self::InvalidStatusCode(status) => {
                f.debug_tuple("InvalidStatusCode").field(status).finish()
            }
            Self::InvalidStatusCodeWithMessage(status, _, retry_after) => f
                .debug_tuple("InvalidStatusCodeWithMessage")
                .field(status)
                .field(&"<redacted>")
                .field(retry_after)
                .finish(),
            Self::InvalidHeaderValue(error) => {
                f.debug_tuple("InvalidHeaderValue").field(error).finish()
            }
            Self::NoHeaders => f.write_str("NoHeaders"),
            Self::StreamEnded => f.write_str("StreamEnded"),
            Self::InvalidContentType(value) => {
                f.debug_tuple("InvalidContentType").field(value).finish()
            }
            Self::SseEventTooLarge => f.write_str("SseEventTooLarge"),
            Self::Instance(error) => f.debug_tuple("Instance").field(error).finish(),
        }
    }
}

impl Error {
    pub(crate) fn non_success_status(&self) -> Option<StatusCode> {
        match self {
            Self::InvalidStatusCode(status)
            | Self::InvalidStatusCodeWithMessage(status, _, _) => Some(*status),
            _ => None,
        }
    }

    pub(crate) fn non_success_body(&self) -> Option<&str> {
        match self {
            Self::InvalidStatusCodeWithMessage(_, body, _) => Some(body.as_str()),
            _ => None,
        }
    }

    /// Bounded `Retry-After` header captured from a non-success response.
    #[must_use]
    pub fn retry_after(&self) -> Option<&str> {
        match self {
            Self::InvalidStatusCodeWithMessage(_, _, retry_after) => {
                retry_after.as_ref().map(BoundedRetryAfter::as_str)
            }
            _ => None,
        }
    }
}

pub fn retry_after_from_headers(headers: &HeaderMap) -> Option<BoundedRetryAfter> {
    headers
        .get(http::header::RETRY_AFTER)
        .and_then(|value| BoundedRetryAfter::from_header_bytes(value.as_bytes()))
}

fn retry_after_from_reqwest(response: &reqwest::Response) -> Option<BoundedRetryAfter> {
    retry_after_from_headers(response.headers())
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(not(target_family = "wasm"))]
pub(crate) fn instance_error<E: std::error::Error + Send + Sync + 'static>(error: E) -> Error {
    Error::Instance(error.into())
}

#[cfg(target_family = "wasm")]
fn instance_error<E: std::error::Error + 'static>(error: E) -> Error {
    Error::Instance(error.into())
}

async fn read_reqwest_error_body(mut response: reqwest::Response) -> Result<BoundedErrorBody> {
    let mut buf = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if push_error_body_chunk(&mut buf, chunk.as_ref()).is_err() {
                    drop(response);
                    return Ok(BoundedErrorBody::truncated());
                }
            }
            Ok(None) => break,
            Err(error) => return Err(instance_error(error)),
        }
    }
    Ok(BoundedErrorBody::complete(
        String::from_utf8_lossy(&buf).into_owned(),
    ))
}

async fn non_success_status_error(response: reqwest::Response) -> Error {
    let status = response.status();
    let retry_after = retry_after_from_reqwest(&response);
    status_error_from_body(status, retry_after, read_reqwest_error_body(response).await)
}

pub type LazyBytes = WasmBoxedFuture<'static, Result<Bytes>>;
pub type LazyBody<T> = WasmBoxedFuture<'static, Result<T>>;

pub type StreamingResponse = Response<BoxedStream>;

#[derive(Debug, Clone, Copy)]
pub struct NoBody;

impl From<NoBody> for Bytes {
    fn from(_: NoBody) -> Self {
        Bytes::new()
    }
}

impl From<NoBody> for Body {
    fn from(_: NoBody) -> Self {
        reqwest::Body::default()
    }
}

pub async fn text(response: Response<LazyBody<Vec<u8>>>) -> Result<String> {
    let text = response.into_body().await?;
    Ok(String::from(String::from_utf8_lossy(&text)))
}

/// Capture status, a bounded error body, and `Retry-After` from a unary HTTP
/// response. Overflow stores a sentinel one byte past [`ERROR_BODY_MAX_BYTES`]
/// instead of the provider prefix. A body-read transport failure after status
/// and headers have arrived preserves status and `Retry-After` with an empty
/// body; no partial payload is stored.
pub async fn error_from_response<U>(response: Response<LazyBody<U>>) -> Error
where
    U: AsRef<[u8]>,
{
    let status = response.status();
    let retry_after = retry_after_from_headers(response.headers());
    let body = match response.into_body().await {
        Ok(bytes) => Ok(BoundedErrorBody::from_slice(bytes.as_ref())),
        Err(error) => Err(error),
    };
    status_error_from_body(status, retry_after, body)
}

fn status_error_from_body(
    status: StatusCode,
    retry_after: Option<BoundedRetryAfter>,
    body: Result<BoundedErrorBody>,
) -> Error {
    match body {
        Ok(body) => Error::InvalidStatusCodeWithMessage(status, body.into_string(), retry_after),
        Err(_) => Error::InvalidStatusCodeWithMessage(status, String::new(), retry_after),
    }
}

pub fn make_auth_header(key: impl AsRef<str>) -> Result<(HeaderName, HeaderValue)> {
    Ok((
        http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", key.as_ref()))?,
    ))
}

pub fn bearer_auth_header(headers: &mut HeaderMap, key: impl AsRef<str>) -> Result<()> {
    let (k, v) = make_auth_header(key)?;

    headers.insert(k, v);

    Ok(())
}

pub fn with_bearer_auth(mut req: Builder, auth: &str) -> Result<Builder> {
    bearer_auth_header(req.headers_mut().ok_or(Error::NoHeaders)?, auth)?;

    Ok(req)
}

/// A helper trait to make generic requests (both regular and SSE) possible.
pub trait HttpClientExt: WasmCompatSend + WasmCompatSync {
    /// Send a HTTP request, get a response back (as bytes). Response must be able to be turned back into Bytes.
    fn send<T, U>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes>,
        T: WasmCompatSend,
        U: From<Bytes>,
        U: WasmCompatSend + 'static;

    /// Send a HTTP request with a multipart body, get a response back (as bytes). Response must be able to be turned back into Bytes (although usually for the response, you will probably want to specify Bytes anyway).
    fn send_multipart<U>(
        &self,
        req: Request<MultipartForm>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes>,
        U: WasmCompatSend + 'static;

    /// Send a HTTP request, get a streamed response back (as a stream of [`bytes::Bytes`].)
    fn send_streaming<T>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = Result<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend;
}

async fn into_lazy_response<U>(response: reqwest::Response) -> Result<Response<LazyBody<U>>>
where
    U: From<Bytes>,
    U: WasmCompatSend + 'static,
{
    if !response.status().is_success() {
        return Err(non_success_status_error(response).await);
    }

    let mut res = Response::builder().status(response.status());

    if let Some(headers) = res.headers_mut() {
        *headers = response.headers().clone();
    }

    let body: LazyBody<U> = Box::pin(async {
        let bytes = response.bytes().await.map_err(instance_error)?;
        Ok(U::from(bytes))
    });

    res.body(body).map_err(Error::Protocol)
}

macro_rules! impl_http_client_ext {
    ($(#[$attribute:meta])* $client:ty) => {
        $(#[$attribute])*
        impl HttpClientExt for $client {
            fn send<T, U>(
                &self,
                req: Request<T>,
            ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
            where
                T: Into<Bytes>,
                U: From<Bytes> + WasmCompatSend + 'static,
            {
                let (parts, body) = req.into_parts();
                let req = self
                    .request(parts.method, parts.uri.to_string())
                    .headers(parts.headers)
                    .body(body.into());

                async move {
                    let response = req.send().await.map_err(instance_error)?;
                    into_lazy_response(response).await
                }
            }

            fn send_multipart<U>(
                &self,
                req: Request<MultipartForm>,
            ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
            where
                U: From<Bytes>,
                U: WasmCompatSend + 'static,
            {
                let (parts, body) = req.into_parts();
                let body = reqwest::multipart::Form::from(body);

                let req = self
                    .request(parts.method, parts.uri.to_string())
                    .headers(parts.headers)
                    .multipart(body);

                async move {
                    let response = req.send().await.map_err(instance_error)?;
                    into_lazy_response(response).await
                }
            }

            fn send_streaming<T>(
                &self,
                req: Request<T>,
            ) -> impl Future<Output = Result<StreamingResponse>> + WasmCompatSend
            where
                T: Into<Bytes> + WasmCompatSend,
            {
                let (parts, body) = req.into_parts();

                let client = self.clone();

                async move {
                    let req = self
                        .request(parts.method, parts.uri.to_string())
                        .headers(parts.headers)
                        .body(body.into())
                        .build()
                        .map_err(|error| Error::Instance(error.into()))?;
                    let response: reqwest::Response =
                        client.execute(req).await.map_err(instance_error)?;
                    if !response.status().is_success() {
                        return Err(non_success_status_error(response).await);
                    }

                    #[cfg(not(target_family = "wasm"))]
                    let mut res = Response::builder()
                        .status(response.status())
                        .version(response.version());

                    #[cfg(target_family = "wasm")]
                    let mut res = Response::builder().status(response.status());

                    if let Some(hs) = res.headers_mut() {
                        *hs = response.headers().clone();
                    }

                    use futures::StreamExt;

                    let mapped_stream: Pin<
                        Box<dyn WasmCompatSendStream<InnerItem = Result<Bytes>>>,
                    > = Box::pin(
                        response
                            .bytes_stream()
                            .map(|chunk| chunk.map_err(|e| Error::Instance(Box::new(e)))),
                    );

                    res.body(mapped_stream).map_err(Error::Protocol)
                }
            }
        }
    };
}

impl_http_client_ext!(reqwest::Client);

impl_http_client_ext!(
    #[cfg(feature = "reqwest-middleware")]
    #[cfg_attr(docsrs, doc(cfg(feature = "reqwest-middleware")))]
    reqwest_middleware::ClientWithMiddleware
);

#[cfg(test)]
mod tests {
    use super::{
        BoundedErrorBody, BoundedRetryAfter, ERROR_BODY_MAX_BYTES, HeaderMap, HeaderValue,
        LazyBody, Response, StatusCode, TRUNCATED_ERROR_BODY_LEN, error_from_response,
        RETRY_AFTER_MAX_BYTES,
    };
    use http::header;

    #[test]
    fn retry_after_capture_bounds_and_omits_raw_value_from_debug() {
        let captured = BoundedRetryAfter::from_header_bytes(b"120").expect("utf-8");
        assert_eq!(captured.as_str(), "120");
        let debug = format!("{captured:?}");
        let display = format!("{captured}");
        assert_eq!(debug, "Retry-After");
        assert_eq!(display, "Retry-After");
        assert!(!debug.contains("120"));
        assert!(!display.contains("120"));
        let exact = vec![b'a'; RETRY_AFTER_MAX_BYTES];
        let bounded = BoundedRetryAfter::from_header_bytes(&exact).expect("exact bound");
        assert_eq!(bounded.as_str().len(), RETRY_AFTER_MAX_BYTES);


        let long = vec![b'a'; RETRY_AFTER_MAX_BYTES + 8];
        assert!(BoundedRetryAfter::from_header_bytes(&long).is_none());

        assert!(BoundedRetryAfter::from_header_bytes(b"").is_none());
        assert!(BoundedRetryAfter::from_header_bytes(&[0xff, 0xfe]).is_none());
    }

    fn unary_response(
        status: StatusCode,
        body: Vec<u8>,
        headers: HeaderMap,
    ) -> Response<LazyBody<Vec<u8>>> {
        let body: LazyBody<Vec<u8>> = Box::pin(async move { Ok(body) });
        let mut builder = Response::builder().status(status);
        if let Some(map) = builder.headers_mut() {
            map.extend(headers);
        }
        builder.body(body).expect("response")
    }

    #[test]
    fn error_body_from_chunks_preserves_small_payload() {
        let body = BoundedErrorBody::from_chunks([br#"{"error":"slow down"}"#.as_slice()]);
        assert!(!body.is_truncated());
        assert_eq!(body.as_str(), r#"{"error":"slow down"}"#);
    }

    #[test]
    fn error_body_from_chunks_drops_remaining_items_after_cap() {
        let mut yielded = 0usize;
        let first = vec![b'a'; ERROR_BODY_MAX_BYTES + 1];
        let extra = vec![b'b'; 8];
        let body = BoundedErrorBody::from_chunks(
            [first.as_slice(), extra.as_slice()]
                .into_iter()
                .inspect(|_| yielded += 1),
        );
        assert!(body.is_truncated());
        assert_eq!(yielded, 1);
        assert!(!body.as_str().contains('b'));
    }

    #[test]
    fn error_body_transport_before_cap_returns_no_body() {
        let result = BoundedErrorBody::try_from_chunks([
            Ok::<_, &'static str>(b"partial".as_slice()),
            Err("reset"),
        ]);
        assert_eq!(result, Err("reset"));
    }

    #[test]
    fn error_body_transport_after_cap_is_not_observed_once_remaining_dropped() {
        let overflow = vec![b'a'; ERROR_BODY_MAX_BYTES + 1];
        let result = BoundedErrorBody::try_from_chunks([
            Ok::<_, &'static str>(overflow.as_slice()),
            Err("reset"),
        ]);
        let body = result.expect("truncated capture, not transport");
        assert!(body.is_truncated());
        assert!(!body.as_str().contains('a'));
    }

    #[test]
    fn error_body_from_chunks_caps_during_consumption_and_marks_truncation() {
        let first = vec![b'a'; ERROR_BODY_MAX_BYTES / 2];
        let second = vec![b'b'; ERROR_BODY_MAX_BYTES / 2];
        let overflow = vec![b'c'; 1];
        let body = BoundedErrorBody::from_chunks([first.as_slice(), second.as_slice(), overflow.as_slice()]);
        assert!(body.is_truncated());
        assert_eq!(body.as_str().len(), TRUNCATED_ERROR_BODY_LEN);
        assert!(!body.as_str().contains('a'));
        assert!(!body.as_str().contains('b'));
        assert!(!body.as_str().contains('c'));
    }

    #[test]
    fn error_body_from_slice_exact_cap_is_not_truncated() {
        let raw = vec![b'x'; ERROR_BODY_MAX_BYTES];
        let body = BoundedErrorBody::from_slice(&raw);
        assert!(!body.is_truncated());
        assert_eq!(body.as_str().len(), ERROR_BODY_MAX_BYTES);
        assert_eq!(body.as_str(), "x".repeat(ERROR_BODY_MAX_BYTES));
    }

    #[tokio::test]
    async fn error_from_response_preserves_small_body_status_and_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static("120"));
        let payload = br#"{"error":{"message":"slow down"}}"#.to_vec();
        let response = unary_response(StatusCode::TOO_MANY_REQUESTS, payload.clone(), headers);
        let error = error_from_response(response).await;
        assert_eq!(
            error.non_success_status(),
            Some(StatusCode::TOO_MANY_REQUESTS)
        );
        assert_eq!(
            error.non_success_body(),
            Some(std::str::from_utf8(&payload).expect("utf-8"))
        );
        assert_eq!(error.retry_after(), Some("120"));
    }

    #[tokio::test]
    async fn error_from_response_truncated_body_exceeds_classifier_cap() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static("7"));
        let payload = vec![b'z'; ERROR_BODY_MAX_BYTES + 64];
        let response = unary_response(StatusCode::BAD_REQUEST, payload, headers);
        let error = error_from_response(response).await;
        let body = error.non_success_body().expect("captured body");
        assert_eq!(body.len(), TRUNCATED_ERROR_BODY_LEN);
        assert!(body.len() > ERROR_BODY_MAX_BYTES);
        assert!(!body.contains('z'));
        assert_eq!(error.non_success_status(), Some(StatusCode::BAD_REQUEST));
        assert_eq!(error.retry_after(), Some("7"));
    }

    #[tokio::test]
    async fn error_from_response_transport_error_preserves_status_and_retry_after() {
        let body: LazyBody<Vec<u8>> = Box::pin(async {
            Err(super::instance_error(std::io::Error::other("reset payload")))
        });
        let mut headers = HeaderMap::new();
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static("3"));
        let mut builder = Response::builder().status(StatusCode::BAD_GATEWAY);
        if let Some(map) = builder.headers_mut() {
            map.extend(headers);
        }
        let response = builder.body(body).expect("response");
        let error = error_from_response(response).await;
        assert_eq!(error.non_success_status(), Some(StatusCode::BAD_GATEWAY));
        assert_eq!(error.retry_after(), Some("3"));
        assert_eq!(error.non_success_body(), Some(""));
        assert!(matches!(
            error,
            super::Error::InvalidStatusCodeWithMessage(
                StatusCode::BAD_GATEWAY,
                ref body,
                _
            ) if body.is_empty()
        ));
        let debug = format!("{error:?}");
        let display = format!("{error}");
        assert!(!debug.contains("reset payload"));
        assert!(!display.contains("reset payload"));
        assert!(!debug.contains("SECRET"));
        assert_eq!(display, "Invalid status code 502 Bad Gateway with message");
    }

    #[test]
    fn invalid_status_debug_and_display_omit_raw_body() {
        let retry_after = BoundedRetryAfter::from_header_bytes(b"120").expect("utf-8");
        let secret = r#"{"error":{"message":"SECRET_PAYLOAD"}}"#;
        let error = super::Error::InvalidStatusCodeWithMessage(
            StatusCode::TOO_MANY_REQUESTS,
            secret.to_string(),
            Some(retry_after),
        );
        assert_eq!(error.non_success_body(), Some(secret));
        assert_eq!(error.retry_after(), Some("120"));
        let debug = format!("{error:?}");
        let display = format!("{error}");
        assert!(!debug.contains("SECRET_PAYLOAD"));
        assert!(!display.contains("SECRET_PAYLOAD"));
        assert!(!debug.contains(secret));
        assert!(!display.contains(secret));
        assert!(!debug.contains("120"));
        assert!(!display.contains("120"));
        assert!(debug.contains("<redacted>"));
        assert_eq!(
            display,
            "Invalid status code 429 Too Many Requests with message"
        );
    }
}
