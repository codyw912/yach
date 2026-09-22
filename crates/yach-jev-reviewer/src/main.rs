mod questions;
mod typesafe;

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use typesafe::{AdapterError, DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_TIMEOUT, JevConfig};

pub const MANIFEST_JSON: &str = include_str!("../yach.extension.json");

const PROTOCOL: &str = "yach.extension-host.v2";
const EXTENSION_ID: &str = "yach.jev-reviewer";
const REVIEWER_ID: &str = "jev-typesafe";
const CONTRACT: &str = "yach.review.v1";
const ASSESSMENT_SCHEMA: &str = "yach.review-assessment.v1";

pub fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_host(stdin.lock(), stdout.lock(), config_from_env().ok().as_ref())
}

pub fn run_host(
    input: impl BufRead,
    mut output: impl Write,
    config: Option<&JevConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    for line in input.lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match message.get("type").and_then(Value::as_str) {
            Some("extension.initialize") => send_registration(&mut output)?,
            Some("review.assess") => handle_review_assess(&mut output, &message, config)?,
            _ => {}
        }
    }
    Ok(())
}

fn send_registration(output: &mut impl Write) -> io::Result<()> {
    send(
        output,
        &json!({
            "type": "extension.ready",
            "protocol": PROTOCOL,
            "extension_id": EXTENSION_ID
        }),
    )?;
    send(
        output,
        &json!({
            "type": "review.ready",
            "reviewer_id": REVIEWER_ID,
            "contract": CONTRACT
        }),
    )
}

fn handle_review_assess(
    output: &mut impl Write,
    message: &Value,
    config: Option<&JevConfig>,
) -> io::Result<()> {
    let Some(request_id) = message.get("request_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(request) = message.get("request") else {
        return send_result(
            output,
            request_id,
            &error_assessment(request_id, "transport"),
        );
    };
    if request_too_large(request) {
        return send_result(
            output,
            request_id,
            &error_assessment(request_id, "transport"),
        );
    }
    let Some(config) = config else {
        return send_result(
            output,
            request_id,
            &error_assessment(request_id, AdapterError::CredentialsUnavailable.as_str()),
        );
    };
    let assessment = match typesafe::assess(config, request) {
        Ok(assessment) => success_assessment(request_id, &assessment),
        Err(error) => error_assessment(request_id, error.as_str()),
    };
    send_result(output, request_id, &assessment)
}

fn request_too_large(request: &Value) -> bool {
    serde_json::to_vec(request).is_ok_and(|bytes| bytes.len() > 64 * 1024)
}

fn config_from_env() -> Result<JevConfig, AdapterError> {
    let Some(api_key) = std::env::var("TYPESAFE_API_KEY")
        .ok()
        .filter(|key| !key.is_empty())
    else {
        return Err(AdapterError::CredentialsUnavailable);
    };
    let base_url = std::env::var("YACH_JEV_BASE_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
    let model = std::env::var("YACH_JEV_MODEL")
        .ok()
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    Ok(JevConfig {
        base_url,
        api_key,
        model,
        timeout: DEFAULT_TIMEOUT,
    })
}

fn success_assessment(request_id: &str, assessment: &typesafe::JevAssessment) -> Value {
    let mut confidence = serde_json::Map::new();
    for (id, value) in &assessment.confidence {
        confidence.insert(id.clone(), json!(value));
    }
    json!({
        "schema": ASSESSMENT_SCHEMA,
        "request_id": request_id,
        "reviewer_id": REVIEWER_ID,
        "model": assessment.model_returned,
        "authorization": assessment.authorization,
        "restriction_applies": assessment.restriction_applies,
        "consequence": assessment.consequence,
        "evidence_sufficient": assessment.evidence_sufficient,
        "origin_confusion": assessment.origin_confusion,
        "confidence": confidence,
        "evidence_refs": [],
        "adapter_error": Value::Null,
        "usage": {
            "input_tokens": assessment.usage.input_tokens,
            "output_tokens": assessment.usage.output_tokens
        },
        "duration_ms": u64::try_from(assessment.duration.as_millis()).unwrap_or(u64::MAX)
    })
}

fn error_assessment(request_id: &str, adapter_error: &str) -> Value {
    json!({
        "schema": ASSESSMENT_SCHEMA,
        "request_id": request_id,
        "reviewer_id": REVIEWER_ID,
        "model": Value::Null,
        "authorization": "insufficient",
        "restriction_applies": 1.0,
        "consequence": 3.0,
        "evidence_sufficient": 0.0,
        "origin_confusion": 1.0,
        "confidence": {},
        "evidence_refs": [],
        "adapter_error": adapter_error,
        "usage": {"input_tokens": 0, "output_tokens": 0},
        "duration_ms": 0
    })
}

fn send_result(output: &mut impl Write, request_id: &str, assessment: &Value) -> io::Result<()> {
    send(
        output,
        &json!({
            "type": "review.result",
            "request_id": request_id,
            "assessment": assessment
        }),
    )
}

fn send(output: &mut impl Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{EXTENSION_ID, PROTOCOL, REVIEWER_ID, run_host};
    use crate::typesafe;

    fn serve_ok() -> String {
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
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut bytes = [0; 16384];
            let _ = stream.read(&mut bytes);
            let body = json!({
                "model": "jev-1.13.0",
                "answers": {
                    "authorization": {
                        "type": "choice",
                        "choice": "substantive_authorized",
                        "probabilities": {
                            "exact_authorized": 0.1,
                            "substantive_authorized": 0.9,
                            "insufficient": 0.0,
                            "ambiguous": 0.0
                        },
                        "confidence": 0.8
                    },
                    "restriction": {"type": "noul", "noul": 0.0},
                    "consequence": {
                        "type": "score",
                        "score": 0.2,
                        "probabilities": {"0": 0.8, "1": 0.2, "2": 0.0, "3": 0.0},
                        "confidence": 0.7
                    },
                    "evidence": {"type": "noul", "noul": 0.99},
                    "origin_confusion": {"type": "noul", "noul": 0.0}
                },
                "usage": {"input_tokens": 10, "output_tokens": 4}
            })
            .to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        });
        format!("http://{address}")
    }

    #[test]
    fn host_loop_answers_initialize_then_assess_with_matching_request_id() {
        let base_url = serve_ok();
        let config = typesafe::JevConfig {
            base_url,
            api_key: String::from("fixture-key"),
            model: String::from("jev-latest"),
            timeout: Duration::from_secs(2),
        };

        let input = format!(
            "{}\n{}\n",
            json!({
                "type": "extension.initialize",
                "protocol": PROTOCOL,
                "extension_id": EXTENSION_ID
            }),
            json!({
                "type": "review.assess",
                "request_id": "req-host-1",
                "request": {
                    "schema": "yach.review-request.v1",
                    "request_id": "req-host-1"
                }
            })
        );
        let mut output = Vec::new();
        let ran = run_host(std::io::Cursor::new(input), &mut output, Some(&config));
        assert!(ran.is_ok(), "host loop should finish: {ran:?}");

        let frames = String::from_utf8_lossy(&output);
        let messages = frames
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>();
        assert!(messages.is_ok(), "host frames should be json: {messages:?}");
        let Ok(messages) = messages else {
            return;
        };
        assert!(messages.len() >= 3, "expected ready, review.ready, result");
        assert_eq!(messages[0]["type"], "extension.ready");
        assert_eq!(messages[0]["extension_id"], EXTENSION_ID);
        assert_eq!(messages[1]["type"], "review.ready");
        assert_eq!(messages[1]["reviewer_id"], REVIEWER_ID);
        assert_eq!(messages[1]["contract"], "yach.review.v1");
        assert_eq!(messages[2]["type"], "review.result");
        assert_eq!(messages[2]["request_id"], "req-host-1");
        assert_eq!(messages[2]["assessment"]["request_id"], "req-host-1");
        assert_eq!(messages[2]["assessment"]["model"], "jev-1.13.0");
        assert_eq!(
            messages[2]["assessment"]["authorization"],
            "substantive_authorized"
        );
        assert!(messages[2]["assessment"]["adapter_error"].is_null());
        let _ = Duration::from_millis(0);
    }
}
