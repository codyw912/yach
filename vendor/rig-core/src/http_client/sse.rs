//! An SSE implementation that leverages [`crate::http_client::HttpClientExt`] to allow streaming with automatic retry handling for any implementor of HttpClientExt.
//!
//! Primarily intended for internal usage. However if you also wish to implement generic HTTP streaming for your custom completion model,
//! you may find this helpful.
use crate::{
    http_client::{
        HttpClientExt, Result as StreamResult,
        retry::{DEFAULT_RETRY, ExponentialBackoff, RetryPolicy},
    },
    wasm_compat::{WasmCompatSend, WasmCompatSendStream},
};
use bytes::Bytes;
use eventsource_stream::{Event as MessageEvent, EventStreamError, Eventsource};
use futures::Stream;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use futures::{future::BoxFuture, stream::BoxStream};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use futures_timer::Delay;
use http::Response;
use http::{HeaderName, HeaderValue, Request, StatusCode};
use mime_guess::mime;
use pin_project_lite::pin_project;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

pub type BoxedStream = Pin<Box<dyn WasmCompatSendStream<InnerItem = StreamResult<Bytes>>>>;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
type ResponseFuture = BoxFuture<'static, Result<Response<BoxedStream>, super::Error>>;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
type ResponseFuture = LocalBoxFuture<'static, Result<Response<BoxedStream>, super::Error>>;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
type EventStream = BoxStream<'static, Result<MessageEvent, EventStreamError<super::Error>>>;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
type EventStream = LocalBoxStream<'static, Result<MessageEvent, EventStreamError<super::Error>>>;

/// Maximum bytes accumulated for a single SSE event before `eventsource()`
/// sees it. High enough for a normal OpenAI `response.completed` payload,
/// low enough that the parser cannot grow without bound.
pub const SSE_EVENT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Counts bytes of the current unterminated SSE event, including line endings
/// that span chunk boundaries (`\n`|`\n`, `\r`|`\n`, `\r\n`|`\r\n`).
#[derive(Debug, Clone)]
struct SseEventByteLimiter {
    max_bytes: usize,
    current_event_bytes: usize,
    saw_line_end: bool,
    last_was_cr: bool,
}

impl SseEventByteLimiter {
    const fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            current_event_bytes: 0,
            saw_line_end: false,
            last_was_cr: false,
        }
    }

    fn accept(&mut self, chunk: &[u8]) -> Result<(), ()> {
        for &byte in chunk {
            if self.last_was_cr && byte == b'\n' {
                self.last_was_cr = false;
                // The LF that completes a blank-line CRLF after dispatch is not
                // part of the next event.
                if self.current_event_bytes == 0 {
                    continue;
                }
                self.current_event_bytes = self.current_event_bytes.saturating_add(1);
                if self.current_event_bytes > self.max_bytes {
                    return Err(());
                }
                continue;
            }

            self.last_was_cr = byte == b'\r';
            self.current_event_bytes = self.current_event_bytes.saturating_add(1);
            if self.current_event_bytes > self.max_bytes {
                return Err(());
            }

            if byte == b'\n' || byte == b'\r' {
                if self.saw_line_end {
                    self.current_event_bytes = 0;
                    self.saw_line_end = false;
                } else {
                    self.saw_line_end = true;
                }
            } else {
                self.saw_line_end = false;
            }
        }
        Ok(())
    }
}

pin_project! {
    /// Caps per-event accumulation on the byte stream before `eventsource()`.
    struct BoundedSseByteStream<S> {
        #[pin]
        inner: S,
        limiter: SseEventByteLimiter,
        overflowed: bool,
    }
}

impl<S> BoundedSseByteStream<S> {
    fn new(inner: S) -> Self {
        Self::with_max(inner, SSE_EVENT_MAX_BYTES)
    }

    fn with_max(inner: S, max_bytes: usize) -> Self {
        Self {
            inner,
            limiter: SseEventByteLimiter::new(max_bytes),
            overflowed: false,
        }
    }
}

impl<S> Stream for BoundedSseByteStream<S>
where
    S: Stream<Item = StreamResult<Bytes>>,
{
    type Item = StreamResult<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        if *this.overflowed {
            return Poll::Ready(None);
        }
        match this.inner.poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if this.limiter.accept(chunk.as_ref()).is_err() {
                    *this.overflowed = true;
                    Poll::Ready(Some(Err(super::Error::SseEventTooLarge)))
                } else {
                    Poll::Ready(Some(Ok(chunk)))
                }
            }
            other => other,
        }
    }
}

fn open_event_stream(body: BoxedStream, last_event_id: Option<&str>) -> EventStream {
    let mut event_stream = BoundedSseByteStream::new(body).eventsource();
    if let Some(id) = last_event_id {
        event_stream.set_last_event_id(id.to_owned());
    }
    Box::pin(event_stream)
}

pin_project! {
    /// Internal state variants for the SSE state machine.
    #[project = SourceStateProjection]
    enum SourceState {
        /// Initial connection attempt (no retry history yet)
        Connecting {
            #[pin]
            response_future: ResponseFuture,
        },
        /// Reconnection attempt after a retry delay (always has retry history)
        Reconnecting {
            #[pin]
            response_future: ResponseFuture,
            last_retry: (usize, Duration),
        },
        /// Actively receiving SSE events
        Open {
            #[pin]
            event_stream: EventStream,
        },
        /// Waiting before retry after an error
        WaitingToRetry {
            #[pin]
            retry_delay: Delay,
            current_retry: (usize, Duration),
        },
        /// Terminal state
        Closed,
    }
}

pin_project! {
    /// A generic SSE event source that works with any [`HttpClientExt`] implementation.
    #[project = GenericEventSourceProjection]
    pub struct GenericEventSource<HttpClient, RequestBody, Retry = ExponentialBackoff> {
        client: HttpClient,
        req: Request<RequestBody>,
        retry_policy: Retry,
        last_event_id: Option<String>,
        allow_missing_content_type: bool,
        #[pin]
        state: SourceState,
    }
}

impl<HttpClient, RequestBody> GenericEventSource<HttpClient, RequestBody>
where
    HttpClient: HttpClientExt + Clone + 'static,
    RequestBody: Into<Bytes> + Clone + WasmCompatSend + 'static,
{
    /// Create a new event source that will connect to the given request.
    pub fn new(client: HttpClient, req: Request<RequestBody>) -> Self {
        let response_future = Self::create_response_future(&client, &req, None);
        let state = SourceState::Connecting { response_future };

        Self {
            client,
            req,
            retry_policy: DEFAULT_RETRY,
            last_event_id: None,
            allow_missing_content_type: false,
            state,
        }
    }

    pub fn allow_missing_content_type(mut self) -> Self {
        self.allow_missing_content_type = true;
        self
    }

    /// Create a response future for connecting/reconnecting
    fn create_response_future(
        client: &HttpClient,
        req: &Request<RequestBody>,
        last_event_id: Option<&str>,
    ) -> ResponseFuture {
        let mut req_clone = req.clone();
        req_clone
            .headers_mut()
            .entry("Accept")
            .or_insert(HeaderValue::from_static("text/event-stream"));

        if let Some(id) = last_event_id
            && let Ok(value) = HeaderValue::from_str(id)
        {
            req_clone
                .headers_mut()
                .insert(HeaderName::from_static("last-event-id"), value);
        }

        let client_clone = client.clone();
        Box::pin(async move { client_clone.send_streaming(req_clone).await })
    }

    /// Get the last event id
    pub fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    /// Close the event source, transitioning to the Closed state.
    /// After calling this, the stream will yield `None` on the next poll.
    pub fn close(&mut self) {
        self.state = SourceState::Closed;
    }
}

/// Events created by the [`GenericEventSource`]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Event {
    /// The event fired when the connection is opened
    Open,
    /// The event fired when a [`MessageEvent`] is received
    Message(MessageEvent),
}

impl From<MessageEvent> for Event {
    fn from(event: MessageEvent) -> Self {
        Event::Message(event)
    }
}

impl<HttpClient, RequestBody> Stream for GenericEventSource<HttpClient, RequestBody>
where
    HttpClient: HttpClientExt + Clone + 'static,
    RequestBody: Into<Bytes> + Clone + WasmCompatSend + 'static,
{
    type Item = Result<Event, super::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            match this.state.as_mut().project() {
                SourceStateProjection::Connecting { response_future } => {
                    match response_future.poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Ok(response)) => {
                            match check_response(response, *this.allow_missing_content_type) {
                                Ok(response) => {
                                    // Transition: Connecting -> Open
                                    let event_stream = open_event_stream(
                                        response.into_body(),
                                        this.last_event_id.as_deref(),
                                    );
                                    this.state.set(SourceState::Open { event_stream });
                                    return Poll::Ready(Some(Ok(Event::Open)));
                                }
                                Err(err) => {
                                    // Transition: Connecting -> Closed (non-retryable error)
                                    this.state.set(SourceState::Closed);
                                    return Poll::Ready(Some(Err(err)));
                                }
                            }
                        }
                        Poll::Ready(Err(err)) => {
                            // First connection attempt failed - start retry cycle
                            if let Some(delay_duration) = this.retry_policy.retry(&err, None) {
                                // Transition: Connecting -> WaitingToRetry
                                this.state.set(SourceState::WaitingToRetry {
                                    retry_delay: Delay::new(delay_duration),
                                    current_retry: (1, delay_duration),
                                });
                                return Poll::Ready(Some(Err(err)));
                            } else {
                                // Transition: Connecting -> Closed
                                this.state.set(SourceState::Closed);
                                return Poll::Ready(Some(Err(err)));
                            }
                        }
                    }
                }

                SourceStateProjection::Reconnecting {
                    response_future,
                    last_retry,
                } => {
                    match response_future.poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Ok(response)) => {
                            match check_response(response, *this.allow_missing_content_type) {
                                Ok(response) => {
                                    // Transition: Reconnecting -> Open (retry cycle complete)
                                    let event_stream = open_event_stream(
                                        response.into_body(),
                                        this.last_event_id.as_deref(),
                                    );
                                    this.state.set(SourceState::Open { event_stream });
                                    return Poll::Ready(Some(Ok(Event::Open)));
                                }
                                Err(err) => {
                                    // Transition: Reconnecting -> Closed (non-retryable error)
                                    this.state.set(SourceState::Closed);
                                    return Poll::Ready(Some(Err(err)));
                                }
                            }
                        }
                        Poll::Ready(Err(err)) => {
                            // Reconnection attempt failed - continue retry cycle
                            if let Some(delay_duration) =
                                this.retry_policy.retry(&err, Some(*last_retry))
                            {
                                let (retry_num, _) = *last_retry;
                                // Transition: Reconnecting -> WaitingToRetry
                                this.state.set(SourceState::WaitingToRetry {
                                    retry_delay: Delay::new(delay_duration),
                                    current_retry: (retry_num + 1, delay_duration),
                                });
                                return Poll::Ready(Some(Err(err)));
                            } else {
                                // Transition: Reconnecting -> Closed (max retries exceeded)
                                this.state.set(SourceState::Closed);
                                return Poll::Ready(Some(Err(err)));
                            }
                        }
                    }
                }

                SourceStateProjection::Open { event_stream } => {
                    match event_stream.poll_next(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Some(Ok(event))) => {
                            if !event.id.is_empty() {
                                *this.last_event_id = Some(event.id.clone());
                            }
                            if let Some(duration) = event.retry {
                                this.retry_policy.set_reconnection_time(duration);
                            }
                            return Poll::Ready(Some(Ok(Event::Message(event))));
                        }
                        Poll::Ready(Some(Err(EventStreamError::Transport(err)))) => {
                            // Connection error while open - start fresh retry cycle
                            if let Some(delay_duration) = this.retry_policy.retry(&err, None) {
                                // Transition: Open -> WaitingToRetry
                                this.state.set(SourceState::WaitingToRetry {
                                    retry_delay: Delay::new(delay_duration),
                                    current_retry: (1, delay_duration),
                                });
                                return Poll::Ready(Some(Err(err)));
                            } else {
                                // Transition: Open -> Closed
                                this.state.set(SourceState::Closed);
                                return Poll::Ready(Some(Err(err)));
                            }
                        }
                        Poll::Ready(Some(Err(EventStreamError::Parser(_)))) => {
                            // Parser errors are recoverable - continue polling
                            continue;
                        }
                        Poll::Ready(Some(Err(EventStreamError::Utf8(_)))) => {
                            // UTF-8 errors are recoverable - continue polling
                            continue;
                        }
                        Poll::Ready(None) => {
                            // Transition: Open -> Closed
                            this.state.set(SourceState::Closed);
                            return Poll::Ready(None);
                        }
                    }
                }

                SourceStateProjection::WaitingToRetry {
                    retry_delay,
                    current_retry,
                } => {
                    // Copy before polling to avoid borrow conflicts
                    let retry_info = *current_retry;
                    match retry_delay.poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(()) => {
                            // Transition: WaitingToRetry -> Reconnecting
                            let response_future =
                                GenericEventSource::<HttpClient, RequestBody>::create_response_future(
                                    this.client,
                                    this.req,
                                    this.last_event_id.as_deref(),
                                );
                            this.state.set(SourceState::Reconnecting {
                                response_future,
                                last_retry: retry_info,
                            });
                            continue;
                        }
                    }
                }

                SourceStateProjection::Closed => {
                    return Poll::Ready(None);
                }
            }
        }
    }
}

fn check_response<T>(
    response: Response<T>,
    allow_missing_content_type: bool,
) -> Result<Response<T>, super::Error> {
    let StatusCode::OK = response.status() else {
        return Err(super::Error::InvalidStatusCode(response.status()));
    };

    let content_type =
        if let Some(content_type) = response.headers().get(&reqwest::header::CONTENT_TYPE) {
            content_type
        } else if allow_missing_content_type {
            return Ok(response);
        } else {
            return Err(super::Error::InvalidContentType(HeaderValue::from_static(
                "",
            )));
        };

    if content_type
        .to_str()
        .map_err(|_| ())
        .and_then(|s| s.parse::<mime::Mime>().map_err(|_| ()))
        .map(|mime_type| {
            matches!(
                (mime_type.type_(), mime_type.subtype()),
                (mime::TEXT, mime::EVENT_STREAM)
            )
        })
        .unwrap_or(false)
    {
        Ok(response)
    } else {
        Err(super::Error::InvalidContentType(content_type.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedSseByteStream, SSE_EVENT_MAX_BYTES, SseEventByteLimiter};
    use bytes::Bytes;
    use futures::StreamExt;

    #[test]
    fn sse_event_max_bytes_is_high_enough_for_completed_payloads() {
        assert_eq!(SSE_EVENT_MAX_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn event_boundary_resets_across_lf_chunk_split() {
        let mut limiter = SseEventByteLimiter::new(16);
        limiter.accept(b"data: hi\n").expect("prefix");
        assert_eq!(limiter.current_event_bytes, 9);
        limiter.accept(b"\n").expect("blank line completes event");
        assert_eq!(limiter.current_event_bytes, 0);
        limiter.accept(b"data: lo").expect("next event");
        assert_eq!(limiter.current_event_bytes, 8);
    }

    #[test]
    fn event_boundary_resets_across_crlf_chunk_split() {
        let mut limiter = SseEventByteLimiter::new(32);
        limiter.accept(b"data: hi\r").expect("cr");
        assert_eq!(limiter.current_event_bytes, 9);
        limiter
            .accept(b"\n\r\n")
            .expect("lf completes crlf then blank line");
        assert_eq!(limiter.current_event_bytes, 0);

        let mut limiter = SseEventByteLimiter::new(32);
        limiter
            .accept(b"data: hi\r\n\r\n")
            .expect("crlf event in one chunk");
        assert_eq!(limiter.current_event_bytes, 0);
    }

    #[test]
    fn overflow_is_detected_across_chunk_boundaries() {
        let mut limiter = SseEventByteLimiter::new(8);
        limiter.accept(b"aaaa").expect("under cap");
        assert_eq!(limiter.current_event_bytes, 4);
        assert!(limiter.accept(b"aaaaa").is_err());
    }

    #[test]
    fn completed_event_at_cap_is_accepted_then_next_event_counts_separately() {
        let mut limiter = SseEventByteLimiter::new(8);
        limiter.accept(b"aa\n\n").expect("event of 4 bytes");
        assert_eq!(limiter.current_event_bytes, 0);
        limiter.accept(b"bbbbbbbb").expect("exactly cap");
        assert_eq!(limiter.current_event_bytes, 8);
        assert!(limiter.accept(b"c").is_err());
    }

    #[tokio::test]
    async fn bounded_stream_emits_one_redacted_overflow_error() {
        let chunks = [
            Ok::<_, crate::http_client::Error>(Bytes::from_static(b"data: ")),
            Ok(Bytes::from(vec![b'z'; 8])),
        ];
        let mut stream = BoundedSseByteStream::with_max(futures::stream::iter(chunks), 8);
        let first = stream
            .next()
            .await
            .expect("first chunk under cap")
            .expect("ok");
        assert_eq!(first.as_ref(), b"data: ");
        let error = stream
            .next()
            .await
            .expect("overflow item")
            .expect_err("overflow is a transport error");
        assert!(matches!(error, crate::http_client::Error::SseEventTooLarge));
        let debug = format!("{error:?}");
        let display = format!("{error}");
        assert_eq!(debug, "SseEventTooLarge");
        assert_eq!(display, "SSE event exceeded the maximum size");
        assert!(!debug.contains('z'));
        assert!(!display.contains("zzzz"));
        assert!(stream.next().await.is_none(), "one overflow error only");
    }
}
