//! Shared logic for inspecting provider error response bodies across capability errors.
use http::StatusCode;

/// A raw error response preserved from a provider.
///
/// Capability errors store this in their `ProviderResponse` variants when Rig
/// has the provider's response body in hand. Unlike `ProviderError(String)`,
/// which may carry Rig-generated diagnostics, this type always represents the
/// payload the provider actually returned.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderResponseError {
    /// HTTP status of the provider response, when it was captured alongside the body.
    pub status: Option<StatusCode>,
    /// Raw response body as returned by the provider.
    ///
    /// Inspect this field (or [`Self::body`]) explicitly. Debug and Display omit
    /// the payload so it is not leaked through formatting.
    pub body: String,
}

impl ProviderResponseError {
    pub(crate) fn without_status(body: impl Into<String>) -> Self {
        Self {
            status: None,
            body: body.into(),
        }
    }

    /// Raw provider body. Debug and Display do not include this value.
    #[must_use]
    pub fn body(&self) -> &str {
        self.body.as_str()
    }
}

impl std::fmt::Debug for ProviderResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderResponseError")
            .field("status", &self.status)
            .field("body", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Display for ProviderResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(f, "status {status}"),
            None => f.write_str("provider response error"),
        }
    }
}

impl std::error::Error for ProviderResponseError {}

/// Parses an optional response body as JSON.
///
/// Returns:
/// - `Ok(Some(value))` when a body is present and valid JSON.
/// - `Ok(None)` when no body is present.
/// - `Err(error)` when a body is present but isn't valid JSON.
pub(crate) fn json(body: Option<&str>) -> Result<Option<serde_json::Value>, serde_json::Error> {
    body.filter(|body| !body.is_empty())
        .map(serde_json::from_str)
        .transpose()
}

pub(crate) fn completion_error_from_body(
    body: impl AsRef<[u8]>,
) -> crate::completion::CompletionError {
    let body = crate::http_client::BoundedErrorBody::from_slice(body.as_ref()).into_string();
    crate::completion::CompletionError::ProviderResponse(ProviderResponseError::without_status(
        body,
    ))
}

/// Implements the `provider_response_*` inspection helpers on a capability error
/// enum.
///
/// The enum must have a `ProviderResponse(`[`ProviderResponseError`]`)` variant
/// and an `HttpError(`[`http_client::Error`](crate::http_client::Error)`)`
/// variant; the generated helpers read from those two sources only, since they
/// are the only ones that genuinely represent a provider's response.
macro_rules! impl_provider_response_helpers {
    ($error:ty) => {
        impl $error {
            /// Builds an error from a captured HTTP status and raw response body,
            /// routing it so the `provider_response_*` helpers stay useful.
            ///
            /// This is the single funnel every HTTP-error path should use instead
            /// of flattening a status and body into a `ProviderError(String)`:
            /// - A **success (2xx)** status carries a provider-authored error
            ///   envelope, so it is preserved as [`Self::ProviderResponse`]
            ///   together with the status.
            /// - A **non-success** status is preserved as
            ///   [`Self::HttpError`]`(`[`http_client::Error::InvalidStatusCodeWithMessage`](crate::http_client::Error::InvalidStatusCodeWithMessage)`)`.
            ///
            /// Either way the raw `body` is kept verbatim and the status stays
            /// recoverable through [`Self::provider_response_status`]. Read the
            /// response body exactly once and hand it here for both branches.
            pub fn from_http_response(status: http::StatusCode, body: impl Into<String>) -> Self {
                if status.is_success() {
                    Self::ProviderResponse($crate::provider_response::ProviderResponseError {
                        status: Some(status),
                        body: body.into(),
                    })
                } else {
                    Self::HttpError($crate::http_client::Error::InvalidStatusCodeWithMessage(
                        status,
                        body.into(),
                        None,
                    ))
                }
            }

            /// Preserves a raw provider error body that has **no HTTP status**.
            ///
            /// Use this for non-HTTP transports (gRPC / SDK clients such as AWS
            /// Bedrock, Vertex AI, or the gRPC Gemini client) where the provider
            /// returns an error payload but no [`http::StatusCode`] is available.
            /// The body is preserved as [`Self::ProviderResponse`] with
            /// `status == None`, so [`Self::provider_response_body`] still surfaces
            /// it while [`Self::provider_response_status`] returns `None`.
            pub fn from_provider_body(body: impl Into<String>) -> Self {
                Self::ProviderResponse(
                    $crate::provider_response::ProviderResponseError::without_status(body),
                )
            }

            /// Returns the raw provider response body when available.
            ///
            /// This is available for:
            /// - `Self::ProviderResponse` using its preserved body.
            /// - `Self::HttpError` when it wraps an HTTP non-success response that
            ///   carries a body.
            ///
            /// Returns `None` for any other variant — for example a Rig-generated
            /// `ProviderError` diagnostic, or a failure from a transport with no
            /// provider response body to preserve. An empty preserved body is
            /// reported as `Some("")` (the provider returned no payload), which is
            /// distinct from `None`; note that [`Self::provider_response_json`]
            /// maps that same empty body to `Ok(None)`.
            pub fn provider_response_body(&self) -> Option<&str> {
                match self {
                    Self::ProviderResponse(response) => Some(response.body.as_str()),
                    Self::HttpError(error) => error.non_success_body(),
                    _ => None,
                }
            }

            /// Parses the provider response body as JSON.
            ///
            /// Returns:
            /// - `Ok(Some(value))` when a body is present and valid JSON.
            /// - `Ok(None)` when no provider response body is available.
            /// - `Err(error)` when a body is present but isn't valid JSON.
            pub fn provider_response_json(
                &self,
            ) -> Result<Option<serde_json::Value>, serde_json::Error> {
                $crate::provider_response::json(self.provider_response_body())
            }

            /// Returns the HTTP status code when this error preserves one, either
            /// from a non-success HTTP response, from a preserved provider
            /// response, or from a 2xx error envelope.
            ///
            /// **Warning:** this can return a **2xx** status. Some providers send
            /// an error envelope alongside a success status, which Rig preserves
            /// via [`Self::ProviderResponse`]. Callers must not infer failure from
            /// the status code alone — the existence of this error already means
            /// the call failed. Returns `None` for non-HTTP transports (gRPC / SDK
            /// clients) and for variants that carry no provider response.
            pub fn provider_response_status(&self) -> Option<http::StatusCode> {
                match self {
                    Self::ProviderResponse(response) => response.status,
                    Self::HttpError(error) => error.non_success_status(),
                    _ => None,
                }
            }

            /// Bounded `Retry-After` header captured from a non-success HTTP
            /// response. The raw value is omitted from Display/Debug.
            pub fn provider_response_retry_after(&self) -> Option<&str> {
                match self {
                    Self::HttpError(error) => error.retry_after(),
                    _ => None,
                }
            }
        }
    };
}

pub(crate) use impl_provider_response_helpers;

#[cfg(test)]
mod tests {
    use http::StatusCode;

    /// Asserts the shared funnel preserves a provider's status + body across the
    /// three routes every capability error exposes: a non-success HTTP response,
    /// a 2xx provider error envelope, and a non-HTTP (gRPC/SDK) transport.
    macro_rules! assert_funnel {
        ($err:ty) => {{
            let body = r#"{"error":{"message":"boom"}}"#;

            // Non-success status -> HttpError, with status + body recoverable.
            let err = <$err>::from_http_response(StatusCode::SERVICE_UNAVAILABLE, body);
            assert_eq!(
                err.provider_response_status(),
                Some(StatusCode::SERVICE_UNAVAILABLE),
                concat!(stringify!($err), ": non-success status not preserved"),
            );
            assert_eq!(
                err.provider_response_body(),
                Some(body),
                concat!(stringify!($err), ": non-success body not preserved"),
            );
            assert_eq!(
                err.provider_response_json()
                    .expect("valid json")
                    .expect("present json")["error"]["message"],
                "boom",
            );

            // A provider error envelope returned with a 2xx status -> ProviderResponse,
            // preserving the (success) status so callers can still see it.
            let err = <$err>::from_http_response(StatusCode::OK, body);
            assert_eq!(
                err.provider_response_status(),
                Some(StatusCode::OK),
                concat!(stringify!($err), ": 2xx envelope status not preserved"),
            );
            assert_eq!(err.provider_response_body(), Some(body));

            // No HTTP status available (gRPC/SDK) -> ProviderResponse with status None.
            let err = <$err>::from_provider_body(body);
            assert_eq!(
                err.provider_response_status(),
                None,
                concat!(
                    stringify!($err),
                    ": status should be None for provider body"
                ),
            );
            assert_eq!(err.provider_response_body(), Some(body));

            // Empty-body asymmetry: the body is `Some("")` but JSON parses to `Ok(None)`.
            let err = <$err>::from_provider_body("");
            assert_eq!(err.provider_response_body(), Some(""));
            assert!(err.provider_response_json().expect("ok").is_none());

            let retry_after = $crate::http_client::BoundedRetryAfter::from_header_bytes(b"120")
                .expect("utf-8 retry-after");
            let err = <$err>::HttpError($crate::http_client::Error::InvalidStatusCodeWithMessage(
                StatusCode::TOO_MANY_REQUESTS,
                body.to_string(),
                Some(retry_after),
            ));
            assert_eq!(err.provider_response_retry_after(), Some("120"));
            let debug = format!("{err:?}");
            let display = format!("{err}");
            assert!(!debug.contains("120"));
            assert!(!display.contains("120"));
            assert_eq!(err.provider_response_retry_after().map(str::len), Some(3));
            assert!(!debug.contains("boom"));
            assert!(!display.contains("boom"));
            assert!(!debug.contains(body));
            assert!(!display.contains(body));
        }};
    }

    #[test]
    fn funnel_preserves_status_and_body_for_every_capability_error() {
        assert_funnel!(crate::completion::CompletionError);
        assert_funnel!(crate::embeddings::embedding::EmbeddingError);
        assert_funnel!(crate::transcription::TranscriptionError);
        assert_funnel!(crate::client::verify::VerifyError);
        assert_funnel!(crate::rerank::RerankError);
        #[cfg(feature = "image")]
        assert_funnel!(crate::image_generation::ImageGenerationError);
        #[cfg(feature = "audio")]
        assert_funnel!(crate::audio_generation::AudioGenerationError);
    }

    #[test]
    fn provider_response_debug_and_display_omit_raw_body() {
        let secret = r#"{"error":{"message":"SECRET_PAYLOAD"}}"#;
        let error = super::ProviderResponseError {
            status: Some(StatusCode::OK),
            body: secret.to_string(),
        };
        assert_eq!(error.body(), secret);
        let debug = format!("{error:?}");
        let display = format!("{error}");
        assert!(!debug.contains("SECRET_PAYLOAD"));
        assert!(!display.contains("SECRET_PAYLOAD"));
        assert!(!debug.contains(secret));
        assert!(!display.contains(secret));
        assert!(debug.contains("<redacted>"));
        assert_eq!(display, "status 200 OK");

        let completion = crate::completion::CompletionError::ProviderResponse(error.clone());
        let debug = format!("{completion:?}");
        let display = format!("{completion}");
        assert!(!debug.contains("SECRET_PAYLOAD"));
        assert!(!display.contains("SECRET_PAYLOAD"));
        assert_eq!(completion.provider_response_body(), Some(secret));
    }

    #[test]
    fn completion_error_from_body_uses_shared_truncated_sentinel() {
        use crate::http_client::{ERROR_BODY_MAX_BYTES, TRUNCATED_ERROR_BODY_LEN};

        let small = br#"{"error":{"message":"slow down"}}"#;
        let error = super::completion_error_from_body(small);
        assert_eq!(
            error.provider_response_body(),
            Some(std::str::from_utf8(small).expect("utf-8"))
        );

        let oversized = vec![b'z'; ERROR_BODY_MAX_BYTES + 32];
        let error = super::completion_error_from_body(&oversized);
        let body = error.provider_response_body().expect("bounded body");
        assert_eq!(body.len(), TRUNCATED_ERROR_BODY_LEN);
        assert!(body.len() > ERROR_BODY_MAX_BYTES);
        assert!(!body.contains('z'));
        let debug = format!("{error:?}");
        let display = format!("{error}");
        assert!(!debug.contains('z'));
        assert!(!display.contains('z'));
    }
}
