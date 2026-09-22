use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::questions::{
    AUTHORIZATION_CRITERIA, AUTHORIZATION_ID, CONSEQUENCE_ID, CONSEQUENCE_LEVELS, EVIDENCE_ID,
    ORIGIN_CONFUSION_ID, RESTRICTION_ID, review_questions,
};

pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
pub const DEFAULT_MODEL: &str = "jev-latest";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

const SYSTEMONE_PATH: &str = "/v1/systemone";

#[derive(Debug, Clone, PartialEq)]
pub struct JevConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JevAssessment {
    pub authorization: String,
    pub restriction_applies: f64,
    pub consequence: f64,
    pub evidence_sufficient: f64,
    pub origin_confusion: f64,
    pub confidence: BTreeMap<String, f64>,
    pub model_returned: String,
    pub usage: Usage,
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    CredentialsUnavailable,
    CredentialsInvalid,
    RequestRejected,
    RateLimited,
    TimedOut,
    Transport,
    MalformedResponse,
}

impl AdapterError {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::CredentialsUnavailable => "credentials_unavailable",
            Self::CredentialsInvalid => "credentials_invalid",
            Self::RequestRejected => "request_rejected",
            Self::RateLimited => "rate_limited",
            Self::TimedOut => "timed_out",
            Self::Transport | Self::MalformedResponse => "transport",
        }
    }
}

#[derive(Debug, Deserialize)]
struct SystemOneResponse {
    model: String,
    answers: Map<String, Value>,
    usage: UsageBody,
}

#[derive(Debug, Deserialize)]
struct UsageBody {
    input_tokens: u64,
    output_tokens: u64,
}

pub fn assess(config: &JevConfig, state: &Value) -> Result<JevAssessment, AdapterError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(config.timeout)
        .build()
        .map_err(|_| AdapterError::Transport)?;
    let url = format!("{}{SYSTEMONE_PATH}", config.base_url.trim_end_matches('/'));
    let started = std::time::Instant::now();
    let response = client
        .post(url)
        .bearer_auth(&config.api_key)
        .json(&serde_json::json!({
            "state": state,
            "model": config.model,
            "questions": review_questions(),
        }))
        .send()
        .map_err(|error| map_send_error(&error))?;
    let status = response.status();
    if status.as_u16() == 401 {
        return Err(AdapterError::CredentialsInvalid);
    }
    if status.as_u16() == 422 {
        return Err(AdapterError::RequestRejected);
    }
    if status.as_u16() == 429 || status.as_u16() == 529 {
        return Err(AdapterError::RateLimited);
    }
    if !status.is_success() {
        return Err(AdapterError::Transport);
    }
    let body = response
        .json::<SystemOneResponse>()
        .map_err(|_| AdapterError::MalformedResponse)?;
    let assessment = parse_assessment(body, started.elapsed())?;
    Ok(assessment)
}

fn map_send_error(error: &reqwest::Error) -> AdapterError {
    if error.is_timeout() {
        AdapterError::TimedOut
    } else {
        AdapterError::Transport
    }
}

fn parse_assessment(
    body: SystemOneResponse,
    duration: Duration,
) -> Result<JevAssessment, AdapterError> {
    if body.model.is_empty() || body.answers.len() != 5 {
        return Err(AdapterError::MalformedResponse);
    }
    for id in [
        AUTHORIZATION_ID,
        RESTRICTION_ID,
        CONSEQUENCE_ID,
        EVIDENCE_ID,
        ORIGIN_CONFUSION_ID,
    ] {
        if !body.answers.contains_key(id) {
            return Err(AdapterError::MalformedResponse);
        }
    }

    let authorization = choice_answer(
        body.answers
            .get(AUTHORIZATION_ID)
            .ok_or(AdapterError::MalformedResponse)?,
        &AUTHORIZATION_CRITERIA,
    )?;
    let restriction_applies = noul_answer(
        body.answers
            .get(RESTRICTION_ID)
            .ok_or(AdapterError::MalformedResponse)?,
    )?;
    let (consequence, consequence_confidence) = score_answer(
        body.answers
            .get(CONSEQUENCE_ID)
            .ok_or(AdapterError::MalformedResponse)?,
        CONSEQUENCE_LEVELS.len(),
    )?;
    let evidence_sufficient = noul_answer(
        body.answers
            .get(EVIDENCE_ID)
            .ok_or(AdapterError::MalformedResponse)?,
    )?;
    let origin_confusion = noul_answer(
        body.answers
            .get(ORIGIN_CONFUSION_ID)
            .ok_or(AdapterError::MalformedResponse)?,
    )?;

    let mut confidence = BTreeMap::new();
    confidence.insert(AUTHORIZATION_ID.to_owned(), authorization.confidence);
    confidence.insert(RESTRICTION_ID.to_owned(), restriction_applies);
    confidence.insert(CONSEQUENCE_ID.to_owned(), consequence_confidence);
    confidence.insert(EVIDENCE_ID.to_owned(), evidence_sufficient);
    confidence.insert(ORIGIN_CONFUSION_ID.to_owned(), origin_confusion);

    Ok(JevAssessment {
        authorization: authorization.label,
        restriction_applies,
        consequence,
        evidence_sufficient,
        origin_confusion,
        confidence,
        model_returned: body.model,
        usage: Usage {
            input_tokens: body.usage.input_tokens,
            output_tokens: body.usage.output_tokens,
        },
        duration,
    })
}

struct ChoiceAnswer {
    label: String,
    confidence: f64,
}

fn choice_answer(value: &Value, criteria: &[&str]) -> Result<ChoiceAnswer, AdapterError> {
    if value.get("type").and_then(Value::as_str) != Some("choice") {
        return Err(AdapterError::MalformedResponse);
    }
    let Some(label) = value.get("choice").and_then(Value::as_str) else {
        return Err(AdapterError::MalformedResponse);
    };
    if !criteria.contains(&label) {
        return Err(AdapterError::MalformedResponse);
    }
    let probabilities = value
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or(AdapterError::MalformedResponse)?;
    for (key, probability) in probabilities {
        if !criteria.contains(&key.as_str()) || !finite_unit(probability) {
            return Err(AdapterError::MalformedResponse);
        }
    }
    let confidence = required_unit(value, "confidence")?;
    Ok(ChoiceAnswer {
        label: label.to_owned(),
        confidence,
    })
}

fn noul_answer(value: &Value) -> Result<f64, AdapterError> {
    if value.get("type").and_then(Value::as_str) != Some("noul") {
        return Err(AdapterError::MalformedResponse);
    }
    let noul = value.get("noul").ok_or(AdapterError::MalformedResponse)?;
    if !finite_unit(noul) {
        return Err(AdapterError::MalformedResponse);
    }
    noul.as_f64().ok_or(AdapterError::MalformedResponse)
}

fn score_answer(value: &Value, level_count: usize) -> Result<(f64, f64), AdapterError> {
    if value.get("type").and_then(Value::as_str) != Some("score") {
        return Err(AdapterError::MalformedResponse);
    }
    let score = value.get("score").ok_or(AdapterError::MalformedResponse)?;
    let Some(score) = score.as_f64() else {
        return Err(AdapterError::MalformedResponse);
    };
    let max = f64::from(u32::try_from(level_count.saturating_sub(1)).unwrap_or(u32::MAX));
    if !score.is_finite() || !(0.0..=max).contains(&score) {
        return Err(AdapterError::MalformedResponse);
    }
    let probabilities = value
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or(AdapterError::MalformedResponse)?;
    for (key, probability) in probabilities {
        let Ok(index) = key.parse::<usize>() else {
            return Err(AdapterError::MalformedResponse);
        };
        if index >= level_count || !finite_unit(probability) {
            return Err(AdapterError::MalformedResponse);
        }
    }
    let confidence = required_unit(value, "confidence")?;
    Ok((score, confidence))
}

fn required_unit(value: &Value, field: &str) -> Result<f64, AdapterError> {
    let number = value.get(field).ok_or(AdapterError::MalformedResponse)?;
    if !finite_unit(number) {
        return Err(AdapterError::MalformedResponse);
    }
    number.as_f64().ok_or(AdapterError::MalformedResponse)
}

fn finite_unit(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.is_finite() && (0.0..=1.0).contains(&number))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{AdapterError, JevConfig, assess};

    fn config(base_url: String) -> JevConfig {
        JevConfig {
            base_url,
            api_key: String::from("fixture-key"),
            model: String::from("jev-latest"),
            timeout: Duration::from_secs(2),
        }
    }

    fn state() -> Value {
        json!({"schema": "yach.review-request.v1", "request_id": "req-1"})
    }

    fn ok_body() -> Value {
        json!({
            "model": "jev-1.13.0",
            "answers": {
                "authorization": {
                    "type": "choice",
                    "choice": "exact_authorized",
                    "probabilities": {
                        "exact_authorized": 0.9,
                        "substantive_authorized": 0.1,
                        "insufficient": 0.0,
                        "ambiguous": 0.0
                    },
                    "confidence": 0.84
                },
                "restriction": {"type": "noul", "noul": 0.02},
                "consequence": {
                    "type": "score",
                    "score": 1.05,
                    "legend": {
                        "0": "routine",
                        "1": "reversible",
                        "2": "costly_to_reverse",
                        "3": "destructive_or_disclosing"
                    },
                    "probabilities": {"0": 0.0, "1": 0.95, "2": 0.05, "3": 0.0},
                    "confidence": 0.91
                },
                "evidence": {"type": "noul", "noul": 0.97},
                "origin_confusion": {"type": "noul", "noul": 0.01}
            },
            "usage": {"input_tokens": 120, "output_tokens": 40}
        })
    }

    fn serve(status: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0");
        assert!(listener.is_ok(), "fixture listener should bind");
        let Ok(listener) = listener else {
            return String::from("http://127.0.0.1:9");
        };
        let address = listener.local_addr();
        assert!(address.is_ok(), "fixture address should exist");
        let Ok(address) = address else {
            return String::from("http://127.0.0.1:9");
        };
        let status = status.to_owned();
        let body = body.to_owned();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut bytes = [0; 8192];
            let _ = stream.read(&mut bytes);
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        });
        format!("http://{address}")
    }

    #[test]
    fn assess_parses_model_answers_and_usage_from_ok_fixture() {
        let body = ok_body().to_string();
        let base_url = serve("200 OK", &body);
        let assessment = assess(&config(base_url), &state());
        assert!(
            assessment.is_ok(),
            "200 fixture should parse: {assessment:?}"
        );
        let Ok(assessment) = assessment else {
            return;
        };
        assert_eq!(assessment.model_returned, "jev-1.13.0");
        assert_eq!(assessment.authorization, "exact_authorized");
        assert!((assessment.restriction_applies - 0.02).abs() < f64::EPSILON);
        assert!((assessment.consequence - 1.05).abs() < f64::EPSILON);
        assert!((assessment.evidence_sufficient - 0.97).abs() < f64::EPSILON);
        assert!((assessment.origin_confusion - 0.01).abs() < f64::EPSILON);
        assert_eq!(assessment.usage.input_tokens, 120);
        assert_eq!(assessment.usage.output_tokens, 40);
        assert_eq!(
            assessment.confidence.get("authorization").copied(),
            Some(0.84)
        );
        assert_eq!(
            assessment.confidence.get("consequence").copied(),
            Some(0.91)
        );
    }

    #[test]
    fn assess_maps_unauthorized_fixture_to_credentials_invalid() {
        let base_url = serve("401 Unauthorized", r#"{"error":"unauthorized"}"#);
        assert_eq!(
            assess(&config(base_url), &state()),
            Err(AdapterError::CredentialsInvalid)
        );
    }

    #[test]
    fn assess_rejects_malformed_json() {
        let base_url = serve("200 OK", "not-json");
        assert_eq!(
            assess(&config(base_url), &state()),
            Err(AdapterError::MalformedResponse)
        );
    }

    #[test]
    fn assess_rejects_missing_answer_id() {
        let mut body = ok_body();
        body["answers"]
            .as_object_mut()
            .and_then(|answers| answers.remove("evidence"));
        let base_url = serve("200 OK", &body.to_string());
        assert_eq!(
            assess(&config(base_url), &state()),
            Err(AdapterError::MalformedResponse)
        );
    }

    #[test]
    fn assess_rejects_non_finite_probability() {
        let mut body = ok_body();
        body["answers"]["authorization"]["probabilities"]["exact_authorized"] =
            Value::String(String::from("NaN"));
        let base_url = serve("200 OK", &body.to_string());
        assert_eq!(
            assess(&config(base_url), &state()),
            Err(AdapterError::MalformedResponse)
        );
    }

    #[test]
    fn assess_rejects_answer_type_mismatch() {
        let mut body = ok_body();
        body["answers"]["restriction"] = json!({"type": "choice", "choice": "yes"});
        let base_url = serve("200 OK", &body.to_string());
        assert_eq!(
            assess(&config(base_url), &state()),
            Err(AdapterError::MalformedResponse)
        );
    }
}
