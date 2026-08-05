use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use rig::client::ModelListingClient;

use crate::{ProviderError, ProviderErrorKind, rig_adapter::RigProviderConfig};

const DISCOVERY_FAILURE_MESSAGE: &str = "provider model discovery failed";
const MAX_DISCOVERED_MODELS: usize = 2_048;
const MAX_DISCOVERED_MODEL_ID_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProviderModel {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDiscoveryError {
    Unsupported { provider: &'static str },
    Provider(ProviderError),
}

pub async fn discover_provider_models(
    provider: &RigProviderConfig,
    timeout: Duration,
) -> Result<Vec<DiscoveredProviderModel>, ModelDiscoveryError> {
    match provider {
        RigProviderConfig::Anthropic { api_key, base_url } => {
            let mut builder = rig::providers::anthropic::Client::builder().api_key(api_key);
            if let Some(base_url) = base_url.as_deref() {
                builder = builder.base_url(base_url);
            }
            let client = builder.build().map_err(|_| {
                ModelDiscoveryError::Provider(redacted_discovery_error(
                    ProviderErrorKind::ProviderInternal,
                    "model_client_build",
                ))
            })?;
            list_with_timeout(client.list_models(), timeout).await
        }
        RigProviderConfig::OpenAi { api_key } => {
            let client = rig::providers::openai::Client::builder()
                .api_key(api_key)
                .build()
                .map_err(|_| {
                    ModelDiscoveryError::Provider(redacted_discovery_error(
                        ProviderErrorKind::ProviderInternal,
                        "model_client_build",
                    ))
                })?;
            list_with_timeout(client.list_models(), timeout).await
        }
        RigProviderConfig::OpenAiCompatible { base_url, api_key } => {
            let client = rig::providers::openai::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .build()
                .map_err(|_| {
                    ModelDiscoveryError::Provider(redacted_discovery_error(
                        ProviderErrorKind::ProviderInternal,
                        "model_client_build",
                    ))
                })?;
            list_with_timeout(client.list_models(), timeout).await
        }
        RigProviderConfig::ChatGptSubscription { .. } => Err(ModelDiscoveryError::Unsupported {
            provider: "chatgpt-subscription",
        }),
    }
}

async fn list_with_timeout<F>(
    future: F,
    timeout: Duration,
) -> Result<Vec<DiscoveredProviderModel>, ModelDiscoveryError>
where
    F: Future<Output = Result<rig::model::ModelList, rig::model::ModelListingError>>,
{
    let list = tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| {
            ModelDiscoveryError::Provider(redacted_discovery_error(
                ProviderErrorKind::Timeout,
                "model_listing_timeout",
            ))
        })?
        .map_err(|error| map_listing_error(&error))?;

    Ok(normalize_model_list(list))
}

fn normalize_model_list(list: rig::model::ModelList) -> Vec<DiscoveredProviderModel> {
    let mut models = BTreeMap::new();

    for model in list.into_iter().take(MAX_DISCOVERED_MODELS) {
        if model.id.is_empty() || model.id.len() > MAX_DISCOVERED_MODEL_ID_BYTES {
            continue;
        }
        models.entry(model.id).or_insert(model.name);
    }

    models
        .into_iter()
        .map(|(id, display_name)| DiscoveredProviderModel { id, display_name })
        .collect()
}

fn map_listing_error(error: &rig::model::ModelListingError) -> ModelDiscoveryError {
    let error = match error {
        rig::model::ModelListingError::ApiError { status_code, .. } => {
            return ModelDiscoveryError::Provider(ProviderError {
                kind: provider_error_kind_for_status(*status_code),
                message: String::from(DISCOVERY_FAILURE_MESSAGE),
                redacted_debug: Some(format!("model_listing_status={status_code}")),
            });
        }
        rig::model::ModelListingError::RequestError { .. } => {
            redacted_discovery_error(ProviderErrorKind::Network, "model_listing_request")
        }
        rig::model::ModelListingError::ParseError { .. } => {
            redacted_discovery_error(ProviderErrorKind::MalformedStream, "model_listing_parse")
        }
        rig::model::ModelListingError::AuthError { .. } => redacted_discovery_error(
            ProviderErrorKind::Authentication,
            "model_listing_authentication",
        ),
        rig::model::ModelListingError::RateLimitError { .. } => {
            redacted_discovery_error(ProviderErrorKind::RateLimited, "model_listing_rate_limited")
        }
        rig::model::ModelListingError::ServiceUnavailable { .. } => redacted_discovery_error(
            ProviderErrorKind::ProviderInternal,
            "model_listing_service_unavailable",
        ),
        rig::model::ModelListingError::UnknownError { .. } => {
            redacted_discovery_error(ProviderErrorKind::Unknown, "model_listing_unknown")
        }
    };

    ModelDiscoveryError::Provider(error)
}

const fn provider_error_kind_for_status(status_code: u16) -> ProviderErrorKind {
    match status_code {
        401 | 403 => ProviderErrorKind::Authentication,
        429 => ProviderErrorKind::RateLimited,
        500..=599 => ProviderErrorKind::ProviderInternal,
        400..=499 => ProviderErrorKind::InvalidRequest,
        _ => ProviderErrorKind::Unknown,
    }
}

fn redacted_discovery_error(kind: ProviderErrorKind, debug: &'static str) -> ProviderError {
    ProviderError {
        kind,
        message: String::from(DISCOVERY_FAILURE_MESSAGE),
        redacted_debug: Some(String::from(debug)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        DISCOVERY_FAILURE_MESSAGE, DiscoveredProviderModel, MAX_DISCOVERED_MODELS,
        ModelDiscoveryError, discover_provider_models, list_with_timeout, map_listing_error,
        normalize_model_list,
    };
    use crate::{ProviderErrorKind, rig_adapter::RigProviderConfig};

    #[test]
    fn normalize_models_deduplicates_sorts_and_keeps_provider_names() {
        let list = rig::model::ModelList::new(vec![
            rig::model::Model::new("z-model", "Z Model"),
            rig::model::Model::from_id("a-model"),
            rig::model::Model::new("z-model", "Duplicate"),
            rig::model::Model::from_id(""),
        ]);

        assert_eq!(
            normalize_model_list(list),
            vec![
                DiscoveredProviderModel {
                    id: String::from("a-model"),
                    display_name: None,
                },
                DiscoveredProviderModel {
                    id: String::from("z-model"),
                    display_name: Some(String::from("Z Model")),
                },
            ]
        );
    }

    #[test]
    fn normalize_models_discards_empty_and_overlong_ids_but_keeps_256_byte_ids() {
        let exact_bound = "a".repeat(256);
        let overlong = "b".repeat(257);
        let list = rig::model::ModelList::new(vec![
            rig::model::Model::from_id(""),
            rig::model::Model::from_id(overlong),
            rig::model::Model::from_id(exact_bound.clone()),
        ]);

        assert_eq!(
            normalize_model_list(list),
            vec![DiscoveredProviderModel {
                id: exact_bound,
                display_name: None,
            }]
        );
    }

    #[test]
    fn normalize_models_discards_entries_after_the_discovery_bound() {
        let list = rig::model::ModelList::new(
            (0..=MAX_DISCOVERED_MODELS)
                .map(|index| rig::model::Model::from_id(format!("model-{index}")))
                .collect(),
        );

        let normalized = normalize_model_list(list);

        assert_eq!(normalized.len(), MAX_DISCOVERED_MODELS);
        assert!(
            !normalized
                .iter()
                .any(|model| model.id == format!("model-{MAX_DISCOVERED_MODELS}"))
        );
    }

    #[test]
    fn listing_api_error_maps_status_without_forwarding_the_body() {
        let error = map_listing_error(&rig::model::ModelListingError::ApiError {
            status_code: 401,
            message: String::from("response_body_preview: secret provider detail"),
        });

        assert_redacted_provider_error(
            error,
            ProviderErrorKind::Authentication,
            "model_listing_status=401",
        );
    }

    #[test]
    fn listing_errors_map_typed_variants_without_forwarding_messages() {
        let cases = [
            (
                rig::model::ModelListingError::ApiError {
                    status_code: 429,
                    message: String::from("secret rate-limit body"),
                },
                ProviderErrorKind::RateLimited,
                "model_listing_status=429",
            ),
            (
                rig::model::ModelListingError::ApiError {
                    status_code: 500,
                    message: String::from("secret server body"),
                },
                ProviderErrorKind::ProviderInternal,
                "model_listing_status=500",
            ),
            (
                rig::model::ModelListingError::RequestError {
                    message: String::from("secret request detail"),
                },
                ProviderErrorKind::Network,
                "model_listing_request",
            ),
            (
                rig::model::ModelListingError::ParseError {
                    message: String::from("secret parse detail"),
                },
                ProviderErrorKind::MalformedStream,
                "model_listing_parse",
            ),
            (
                rig::model::ModelListingError::AuthError {
                    message: String::from("secret authentication detail"),
                },
                ProviderErrorKind::Authentication,
                "model_listing_authentication",
            ),
            (
                rig::model::ModelListingError::RateLimitError {
                    message: String::from("secret rate-limit detail"),
                },
                ProviderErrorKind::RateLimited,
                "model_listing_rate_limited",
            ),
            (
                rig::model::ModelListingError::ServiceUnavailable {
                    message: String::from("secret availability detail"),
                },
                ProviderErrorKind::ProviderInternal,
                "model_listing_service_unavailable",
            ),
            (
                rig::model::ModelListingError::UnknownError {
                    message: String::from("secret unknown detail"),
                },
                ProviderErrorKind::Unknown,
                "model_listing_unknown",
            ),
        ];

        for (listing_error, expected_kind, expected_debug) in cases {
            assert_redacted_provider_error(
                map_listing_error(&listing_error),
                expected_kind,
                expected_debug,
            );
        }
    }

    #[tokio::test]
    async fn listing_timeout_maps_to_a_redacted_timeout_error() {
        let Err(error) = list_with_timeout(
            std::future::pending::<Result<rig::model::ModelList, rig::model::ModelListingError>>(),
            Duration::ZERO,
        )
        .await
        else {
            unreachable!("a pending listing must time out");
        };

        assert_redacted_provider_error(error, ProviderErrorKind::Timeout, "model_listing_timeout");
    }

    #[tokio::test]
    async fn chatgpt_subscription_discovery_is_unsupported_without_network() {
        let Err(error) = discover_provider_models(
            &RigProviderConfig::ChatGptSubscription {
                token_dir: PathBuf::from("/not-used-for-discovery"),
            },
            Duration::from_secs(1),
        )
        .await
        else {
            unreachable!("ChatGPT subscription discovery must not issue a request");
        };

        assert_eq!(
            error,
            ModelDiscoveryError::Unsupported {
                provider: "chatgpt-subscription",
            }
        );
    }

    fn assert_redacted_provider_error(
        error: ModelDiscoveryError,
        expected_kind: ProviderErrorKind,
        expected_debug: &str,
    ) {
        let ModelDiscoveryError::Provider(error) = error else {
            unreachable!("listing errors must map to provider errors");
        };
        assert_eq!(error.kind, expected_kind);
        assert_eq!(error.message, DISCOVERY_FAILURE_MESSAGE);
        assert!(!error.message.contains("secret"));
        assert_eq!(error.redacted_debug.as_deref(), Some(expected_debug));
    }
}
