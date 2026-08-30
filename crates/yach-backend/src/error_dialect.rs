//! Compiled-in provider error-dialect registry.
//!
//! Catalog data supplies an optional dialect ID only. Parser behavior lives
//! here as reviewed typed envelopes; unknown and missing IDs use the
//! conservative generic parser.

use std::time::{Duration, SystemTime};

use rig::completion::CompletionError;
use serde::Deserialize;
const MAX_PROVIDER_ERROR_BODY_BYTES: usize = rig::http_client::ERROR_BODY_MAX_BYTES;

use crate::provider::{
    ClassificationSource, ProviderError, ProviderErrorKind, ProviderErrorMetadata,
};

/// Reviewed dialect parsers selected by an explicit baked catalog ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownErrorDialect {
    OpenAi,
    Anthropic,
    OpenAiCompatible,
    ChatGptSubscription,
}

/// Result of mapping a catalog dialect ID onto compiled-in parser code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialectSelection {
    Known(KnownErrorDialect),
    #[default]
    Missing,
    Unknown,
}

/// Secret-free identity for one provider attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIdentity {
    pub provider: String,
    pub model: String,
    pub error_dialect: DialectSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TypedClassification {
    kind: ProviderErrorKind,
    provider_code: Option<&'static str>,
}

#[derive(Deserialize)]
struct OpenAiErrorEnvelope {
    error: OpenAiErrorBody,
}

#[derive(Deserialize)]
struct OpenAiErrorBody {
    #[serde(rename = "type")]
    error_type: Option<String>,
    code: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicErrorEnvelope {
    error: AnthropicErrorBody,
}

#[derive(Deserialize)]
struct AnthropicErrorBody {
    #[serde(rename = "type")]
    error_type: Option<String>,
    message: Option<String>,
}

/// Maps a baked catalog dialect ID onto compiled-in parser selection.
///
/// `None` is [`DialectSelection::Missing`]. An unrecognized ID is
/// [`DialectSelection::Unknown`] and never prevents startup or a request.
#[must_use]
pub fn select_error_dialect(id: Option<&str>) -> DialectSelection {
    match id {
        None => DialectSelection::Missing,
        Some("openai") => DialectSelection::Known(KnownErrorDialect::OpenAi),
        Some("anthropic") => DialectSelection::Known(KnownErrorDialect::Anthropic),
        Some("openai-compatible") => DialectSelection::Known(KnownErrorDialect::OpenAiCompatible),
        Some("chatgpt-subscription") => {
            DialectSelection::Known(KnownErrorDialect::ChatGptSubscription)
        }
        Some(_) => DialectSelection::Unknown,
    }
}

/// Parses a captured `Retry-After` value into milliseconds.
///
/// Accepts delta-seconds and IMF-fixdate HTTP dates evaluated against `now`.
/// Invalid, negative, past, non-UTF-8, or overflowing values are ignored.
#[must_use]
pub fn parse_retry_after_ms(value: &str, now: SystemTime) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return seconds.checked_mul(1000);
    }
    if value.parse::<i64>().is_ok() {
        return None;
    }
    let unix_secs = parse_imf_fixdate(value)?;
    let target = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(unix_secs))?;
    let delay = target.duration_since(now).ok()?;
    u64::try_from(delay.as_millis()).ok()
}

/// Classifies a Rig completion error using the attempt's dialect selection.
#[must_use]
pub fn classify_completion_error(
    identity: &ProviderIdentity,
    error: &CompletionError,
    now: SystemTime,
) -> ProviderError {
    let (variant, variant_kind) = completion_variant(error);
    let status = error
        .provider_response_status()
        .map(|status| status.as_u16());
    let body = error
        .provider_response_body()
        .filter(|body| body.len() <= MAX_PROVIDER_ERROR_BODY_BYTES);
    let retry_after_ms = error
        .provider_response_retry_after()
        .and_then(|value| parse_retry_after_ms(value, now));

    let mut kind = variant_kind;
    let mut source = ClassificationSource::Variant;
    let mut provider_code = None;

    let typed = match identity.error_dialect {
        DialectSelection::Known(dialect) => classify_typed(dialect, body),
        DialectSelection::Missing | DialectSelection::Unknown => None,
    };
    if let Some(classified) = typed {
        provider_code = classified.provider_code.map(str::to_owned);
    }
    if let Some(status_kind) = retryable_status_kind(status) {
        kind = status_kind;
        source = ClassificationSource::Status;
    } else if let Some(classified) = typed {
        kind = classified.kind;
        source = ClassificationSource::TypedDialect;
    } else if let Some(status_kind) = generic_status_kind(status) {
        kind = status_kind;
        source = ClassificationSource::Status;
    } else if let Some(keyword_kind) = keyword_kind(body) {
        kind = keyword_kind;
        source = ClassificationSource::Keyword;
    }

    let status_label = status.map_or_else(|| String::from("none"), |status| status.to_string());
    let code_label = provider_code.as_deref().unwrap_or("none");
    let source_label = classification_source_label(source);
    ProviderError {
        kind,
        message: String::from("Rig provider call failed"),
        redacted_debug: Some(format!(
            "completion_error variant={variant} status={status_label} code={code_label} source={source_label}"
        )),
        metadata: ProviderErrorMetadata {
            status_code: status,
            provider_code,
            retry_after_ms,
            timeout_phase: None,
            classification_source: source,
        },
    }
}

fn completion_variant(error: &CompletionError) -> (&'static str, ProviderErrorKind) {
    match error {
        CompletionError::HttpError(_) => ("http", ProviderErrorKind::Network),
        CompletionError::JsonError(_) => ("json", ProviderErrorKind::MalformedStream),
        CompletionError::UrlError(_) | CompletionError::RequestError(_) => {
            ("request", ProviderErrorKind::InvalidRequest)
        }
        CompletionError::ResponseError(_) => ("response", ProviderErrorKind::MalformedStream),
        CompletionError::ProviderError(_) => ("provider", ProviderErrorKind::ProviderInternal),
        CompletionError::ProviderResponse(_) => {
            ("provider_response", ProviderErrorKind::ProviderInternal)
        }
        CompletionError::Auth(_) => ("auth", ProviderErrorKind::Authentication),
        _ => ("unknown", ProviderErrorKind::Unknown),
    }
}

fn classify_typed(dialect: KnownErrorDialect, body: Option<&str>) -> Option<TypedClassification> {
    let body = body.filter(|body| !body.is_empty())?;
    match dialect {
        KnownErrorDialect::OpenAi
        | KnownErrorDialect::OpenAiCompatible
        | KnownErrorDialect::ChatGptSubscription => classify_openai_envelope(body),
        KnownErrorDialect::Anthropic => classify_anthropic_envelope(body),
    }
}

fn classify_openai_envelope(body: &str) -> Option<TypedClassification> {
    let envelope: OpenAiErrorEnvelope = serde_json::from_str(body).ok()?;
    let error_type = envelope.error.error_type.as_deref().unwrap_or("");
    let code = envelope.error.code.as_deref().unwrap_or("");
    if code == "unsupported_parameter" {
        return Some(TypedClassification {
            kind: ProviderErrorKind::InvalidRequest,
            provider_code: Some("unsupported_parameter"),
        });
    }
    openai_like_classification(error_type, code)
}

fn classify_anthropic_envelope(body: &str) -> Option<TypedClassification> {
    let envelope: AnthropicErrorEnvelope = serde_json::from_str(body).ok()?;
    let error_type = envelope.error.error_type.as_deref().unwrap_or("");
    let message = envelope.error.message.as_deref().unwrap_or("");
    let kind = match error_type {
        "authentication_error" => ProviderErrorKind::Authentication,
        "rate_limit_error" => ProviderErrorKind::RateLimited,
        "not_found_error" => ProviderErrorKind::UnavailableModel,
        "overloaded_error" | "api_error" => ProviderErrorKind::ProviderInternal,
        "invalid_request_error"
            if message.contains("prompt is too long") || message.contains("too many tokens") =>
        {
            ProviderErrorKind::ContextLength
        }
        "invalid_request_error" => ProviderErrorKind::InvalidRequest,
        _ => return None,
    };
    Some(TypedClassification {
        kind,
        provider_code: recognized_provider_code(error_type),
    })
}

fn openai_like_classification(error_type: &str, code: &str) -> Option<TypedClassification> {
    let token = if code.is_empty() { error_type } else { code };
    let kind = match (error_type, code) {
        (_, "invalid_api_key") | ("authentication_error", _) => ProviderErrorKind::Authentication,
        (_, "rate_limit_exceeded") | ("rate_limit_error", _) => ProviderErrorKind::RateLimited,
        (_, "context_length_exceeded") => ProviderErrorKind::ContextLength,
        (_, "model_not_found" | "model_not_available") => ProviderErrorKind::UnavailableModel,
        ("invalid_request_error", _) | (_, "unsupported_parameter") => {
            ProviderErrorKind::InvalidRequest
        }
        ("server_error" | "api_error", _) => ProviderErrorKind::ProviderInternal,
        _ => return None,
    };
    Some(TypedClassification {
        kind,
        provider_code: recognized_provider_code(token),
    })
}

fn recognized_provider_code(token: &str) -> Option<&'static str> {
    Some(match token {
        "unsupported_parameter" => "unsupported_parameter",
        "context_length_exceeded" => "context_length_exceeded",
        "rate_limit_exceeded" => "rate_limit_exceeded",
        "invalid_api_key" => "invalid_api_key",
        "model_not_found" => "model_not_found",
        "model_not_available" => "model_not_available",
        "authentication_error" => "authentication_error",
        "rate_limit_error" => "rate_limit_error",
        "invalid_request_error" => "invalid_request_error",
        "not_found_error" => "not_found_error",
        "overloaded_error" => "overloaded_error",
        "server_error" => "server_error",
        "api_error" => "api_error",
        _ => return None,
    })
}

fn retryable_status_kind(status: Option<u16>) -> Option<ProviderErrorKind> {
    match status {
        Some(429) => Some(ProviderErrorKind::RateLimited),
        Some(408 | 504) => Some(ProviderErrorKind::Timeout),
        _ => None,
    }
}

fn generic_status_kind(status: Option<u16>) -> Option<ProviderErrorKind> {
    match status {
        Some(401 | 403) => Some(ProviderErrorKind::Authentication),
        Some(429) => Some(ProviderErrorKind::RateLimited),
        Some(408 | 504) => Some(ProviderErrorKind::Timeout),
        Some(status) if (400..500).contains(&status) => Some(ProviderErrorKind::InvalidRequest),
        Some(status) if status >= 500 => Some(ProviderErrorKind::ProviderInternal),
        _ => None,
    }
}

fn keyword_kind(body: Option<&str>) -> Option<ProviderErrorKind> {
    let body = body?;
    if body.is_empty() {
        return None;
    }
    Some(crate::rig_adapter::classify_provider_error_debug(body))
}

fn classification_source_label(source: ClassificationSource) -> &'static str {
    match source {
        ClassificationSource::TypedDialect => "typed_dialect",
        ClassificationSource::Status => "status",
        ClassificationSource::Keyword => "keyword",
        ClassificationSource::Variant => "variant",
    }
}

fn parse_imf_fixdate(value: &str) -> Option<u64> {
    if value.len() != 29 || !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    if &bytes[3..5] != b", "
        || bytes[7] != b' '
        || bytes[11] != b' '
        || bytes[16] != b' '
        || bytes[19] != b':'
        || bytes[22] != b':'
        || bytes[25] != b' '
        || &bytes[26..29] != b"GMT"
    {
        return None;
    }
    let weekday = weekday_from_name(&value[0..3])?;
    let day: u32 = value[5..7].parse().ok()?;
    let month = month_from_name(&value[8..11])?;
    let year: i32 = value[12..16].parse().ok()?;
    let hour: u32 = value[17..19].parse().ok()?;
    let minute: u32 = value[20..22].parse().ok()?;
    let second: u32 = value[23..25].parse().ok()?;
    let unix_seconds = unix_seconds_from_civil(year, month, day, hour, minute, second)?;
    let days = unix_seconds / 86_400;
    let actual_weekday = u32::try_from((days + 4) % 7).ok()?;
    (weekday == actual_weekday).then_some(unix_seconds)
}

fn weekday_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "Sun" => 0,
        "Mon" => 1,
        "Tue" => 2,
        "Wed" => 3,
        "Thu" => 4,
        "Fri" => 5,
        "Sat" => 6,
        _ => return None,
    })
}

fn month_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

fn unix_seconds_from_civil(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<u64> {
    if !(1..=12).contains(&month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
        || day < 1
        || day > days_in_month(year, month)?
    {
        return None;
    }
    let y = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = y.div_euclid(400);
    let yoe = u32::try_from(y - era * 400).ok()?;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month_prime + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = i64::from(era) * 146_097 + i64::from(doe) - 719_468;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    u64::try_from(seconds).ok()
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => return None,
    })
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::http_client::{BoundedRetryAfter, Error as HttpError, StatusCode};

    fn identity(dialect: DialectSelection) -> ProviderIdentity {
        ProviderIdentity {
            provider: String::from("openai"),
            model: String::from("gpt-test"),
            error_dialect: dialect,
        }
    }

    fn http_error(status: u16, body: &str, retry_after: Option<&str>) -> CompletionError {
        let status = StatusCode::from_u16(status);
        assert!(status.is_ok());
        let Ok(status) = status else {
            unreachable!("fixture status must be valid");
        };
        let retry_after =
            retry_after.and_then(|value| BoundedRetryAfter::from_header_bytes(value.as_bytes()));
        CompletionError::HttpError(HttpError::InvalidStatusCodeWithMessage(
            status,
            body.to_string(),
            retry_after,
        ))
    }

    fn classify(dialect: DialectSelection, error: &CompletionError) -> ProviderError {
        classify_completion_error(&identity(dialect), error, SystemTime::UNIX_EPOCH)
    }

    #[test]
    fn select_error_dialect_maps_known_missing_and_unknown_ids() {
        assert_eq!(
            select_error_dialect(Some("openai")),
            DialectSelection::Known(KnownErrorDialect::OpenAi)
        );
        assert_eq!(
            select_error_dialect(Some("anthropic")),
            DialectSelection::Known(KnownErrorDialect::Anthropic)
        );
        assert_eq!(
            select_error_dialect(Some("openai-compatible")),
            DialectSelection::Known(KnownErrorDialect::OpenAiCompatible)
        );
        assert_eq!(
            select_error_dialect(Some("chatgpt-subscription")),
            DialectSelection::Known(KnownErrorDialect::ChatGptSubscription)
        );
        assert_eq!(select_error_dialect(None), DialectSelection::Missing);
        assert_eq!(
            select_error_dialect(Some("not-a-dialect")),
            DialectSelection::Unknown
        );
    }

    #[test]
    fn openai_unsupported_parameter_is_invalid_request() {
        let body = r#"{"error":{"type":"invalid_request_error","code":"unsupported_parameter","message":"max_tokens"}}"#;
        let error = classify(
            DialectSelection::Known(KnownErrorDialect::OpenAi),
            &http_error(400, body, None),
        );
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
        assert_eq!(
            error.metadata.provider_code.as_deref(),
            Some("unsupported_parameter")
        );
        assert_eq!(
            error.metadata.classification_source,
            ClassificationSource::TypedDialect
        );
        assert!(error.metadata.hard_non_retryable_status());
        let debug = format!("{error:?}");
        assert!(!debug.contains("max_tokens"));
        assert!(!debug.contains(body));
    }

    #[test]
    fn openai_compatible_without_baked_id_uses_generic_parser() {
        let body = r#"{"error":{"type":"invalid_request_error","code":"model_not_found"}}"#;
        let error = classify(DialectSelection::Missing, &http_error(404, body, None));
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
        assert_eq!(
            error.metadata.classification_source,
            ClassificationSource::Status
        );
        assert!(error.metadata.provider_code.is_none());
    }

    #[test]
    fn generic_404_is_not_unavailable_model() {
        let error = classify(DialectSelection::Unknown, &http_error(404, "", None));
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
        assert_eq!(
            error.metadata.classification_source,
            ClassificationSource::Status
        );
    }

    #[test]
    fn typed_openai_404_model_not_found_is_unavailable_model() {
        let body = r#"{"error":{"code":"model_not_found","type":"invalid_request_error"}}"#;
        let error = classify(
            DialectSelection::Known(KnownErrorDialect::OpenAi),
            &http_error(404, body, None),
        );
        assert_eq!(error.kind, ProviderErrorKind::UnavailableModel);
        assert!(error.metadata.hard_non_retryable_status());
    }

    #[test]
    fn hard_4xx_cannot_become_retryable_from_body() {
        let body = r#"{"error":{"type":"server_error","code":"server_error"}}"#;
        let error = classify(
            DialectSelection::Known(KnownErrorDialect::OpenAi),
            &http_error(401, body, Some("1")),
        );
        assert_eq!(error.kind, ProviderErrorKind::ProviderInternal);
        assert!(error.metadata.hard_non_retryable_status());
        assert_eq!(error.metadata.status_code, Some(401));
    }

    #[test]
    fn retryable_status_wins_over_conflicting_typed_body() {
        let body = r#"{"error":{"type":"invalid_request_error"}}"#;
        for (status, expected) in [
            (408, ProviderErrorKind::Timeout),
            (429, ProviderErrorKind::RateLimited),
            (504, ProviderErrorKind::Timeout),
        ] {
            let error = classify(
                DialectSelection::Known(KnownErrorDialect::OpenAi),
                &http_error(status, body, None),
            );
            assert_eq!(error.kind, expected);
            assert_eq!(
                error.metadata.classification_source,
                ClassificationSource::Status
            );
            assert!(!error.metadata.hard_non_retryable_status());
        }
    }

    #[test]
    fn anthropic_and_chatgpt_typed_envelopes_classify() {
        let anthropic = classify(
            DialectSelection::Known(KnownErrorDialect::Anthropic),
            &http_error(
                429,
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
                None,
            ),
        );
        assert_eq!(anthropic.kind, ProviderErrorKind::RateLimited);
        assert!(!anthropic.metadata.hard_non_retryable_status());

        let context = classify(
            DialectSelection::Known(KnownErrorDialect::Anthropic),
            &http_error(
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 9 > 1"}}"#,
                None,
            ),
        );
        assert_eq!(context.kind, ProviderErrorKind::ContextLength);
        assert!(context.metadata.hard_non_retryable_status());

        let chatgpt = classify(
            DialectSelection::Known(KnownErrorDialect::ChatGptSubscription),
            &http_error(500, r#"{"error":{"type":"server_error"}}"#, None),
        );
        assert_eq!(chatgpt.kind, ProviderErrorKind::ProviderInternal);
        assert!(!chatgpt.metadata.hard_non_retryable_status());
    }

    #[test]
    fn malformed_empty_and_unfamiliar_bodies_fall_back_conservatively() {
        let malformed = classify(
            DialectSelection::Known(KnownErrorDialect::OpenAi),
            &http_error(503, "not-json", None),
        );
        assert_eq!(malformed.kind, ProviderErrorKind::ProviderInternal);
        assert_eq!(
            malformed.metadata.classification_source,
            ClassificationSource::Status
        );

        let empty = classify(
            DialectSelection::Known(KnownErrorDialect::OpenAi),
            &http_error(503, "", None),
        );
        assert_eq!(empty.kind, ProviderErrorKind::ProviderInternal);

        let unfamiliar = classify(
            DialectSelection::Known(KnownErrorDialect::OpenAi),
            &http_error(200, r#"{"error":{"type":"mystery_error"}}"#, None),
        );
        assert_eq!(unfamiliar.kind, ProviderErrorKind::ProviderInternal);
        assert_eq!(
            unfamiliar.metadata.classification_source,
            ClassificationSource::Keyword
        );
    }

    #[test]
    fn retry_after_delta_seconds_and_http_date_parse() {
        assert_eq!(
            parse_retry_after_ms("5", SystemTime::UNIX_EPOCH),
            Some(5_000)
        );
        assert_eq!(parse_retry_after_ms("-1", SystemTime::UNIX_EPOCH), None);
        assert_eq!(
            parse_retry_after_ms("not-a-hint", SystemTime::UNIX_EPOCH),
            None
        );
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(
            parse_retry_after_ms("Thu, 01 Jan 1970 00:00:00 GMT", now),
            None
        );
        assert_eq!(
            parse_retry_after_ms("Thu, 01 Jan 1970 00:00:05 GMT", now),
            None
        );
        let before = SystemTime::UNIX_EPOCH;
        assert_eq!(
            parse_retry_after_ms("Thu, 01 Jan 1970 00:00:05 GMT", before),
            Some(5_000)
        );

        let error = classify(
            DialectSelection::Missing,
            &http_error(429, "{}", Some("12")),
        );
        assert_eq!(error.metadata.retry_after_ms, Some(12_000));
        let debug = format!("{error:?}");
        assert!(!debug.contains("Retry-After: 12"));
        assert!(!debug.contains("\"12\""));
    }

    #[test]
    fn retry_after_rejects_non_imf_dates_and_invalid_calendar_fields() {
        let now = SystemTime::UNIX_EPOCH;
        for value in [
            "Thu, 01 Jan 1970 00:00:05 UTC",
            "Thu, 01 Jan 1970 00:00:05 gmt",
            "Fri, 01 Jan 1970 00:00:05 GMT",
            "Thu, 01 Jan 1970 00:00:60 GMT",
            "Thu, 29 Feb 1970 00:00:05 GMT",
            "Thursday, 01-Jan-70 00:00:05 GMT",
        ] {
            assert_eq!(parse_retry_after_ms(value, now), None, "{value}");
        }
    }

    #[test]
    fn oversized_valid_prefix_skips_typed_and_keyword_classification() {
        let mut body = String::from(r#"{"error":{"type":"server_error","code":"server_error"}}"#);
        body.extend(std::iter::repeat_n(' ', MAX_PROVIDER_ERROR_BODY_BYTES));
        let error = classify(
            DialectSelection::Known(KnownErrorDialect::OpenAi),
            &http_error(500, &body, None),
        );
        assert_eq!(error.kind, ProviderErrorKind::ProviderInternal);
        assert_eq!(
            error.metadata.classification_source,
            ClassificationSource::Status
        );
        assert!(error.metadata.provider_code.is_none());
    }
}
