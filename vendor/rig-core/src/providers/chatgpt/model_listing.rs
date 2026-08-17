use crate::{
    client::ModelLister,
    http_client::{self, HttpClientExt},
    model::{Model, ModelList, ModelListingError},
    providers::chatgpt::{Client, auth::AuthContext},
    wasm_compat::{WasmCompatSend, WasmCompatSync},
};
use serde::Deserialize;

const MODELS_PATH: &str = "/models";

#[derive(Debug, Deserialize)]
struct CodexModelsResponse {
    #[serde(default)]
    models: Vec<CodexModelEntry>,
    #[serde(default)]
    data: Vec<OpenAiModelEntry>,
}

#[derive(Debug, Deserialize)]
struct CodexModelEntry {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
    id: String,
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexCatalogDocument {
    NotModified,
    Modified {
        body: String,
        etag: Option<String>,
    },
}

/// [`ModelLister`] for the ChatGPT Codex backend (`GET /models`).
#[derive(Clone)]
pub struct ChatGptModelLister<H = reqwest::Client> {
    client: Client<H>,
}

impl<H> ModelLister<H> for ChatGptModelLister<H>
where
    H: HttpClientExt + Clone + Default + std::fmt::Debug + WasmCompatSend + WasmCompatSync + 'static,
{
    type Client = Client<H>;

    fn new(client: Self::Client) -> Self {
        Self { client }
    }



    async fn list_all(&self) -> Result<ModelList, ModelListingError> {
        let context = self
            .client
            .ext()
            .auth_context()
            .await
            .map_err(|error| ModelListingError::AuthError {
                message: error.to_string(),
            })?;
        let mut req = add_auth_headers(self.client.get(MODELS_PATH)?, &context);
        if let Some(headers) = req.headers_mut() {
            headers.insert(
                http::header::ACCEPT,
                http::HeaderValue::from_static("application/json"),
            );
        }
        let req = req.body(http_client::NoBody).map_err(|error| {
            ModelListingError::RequestError {
                message: error.to_string(),
            }
        })?;
        let response = self
            .client
            .send::<_, Vec<u8>>(req)
            .await
            .map_err(|error| ModelListingError::RequestError {
                message: error.to_string(),
            })?;

        if !response.status().is_success() {
            let status_code = response.status().as_u16();
            let body = response.into_body().await.map_err(|error| {
                ModelListingError::RequestError {
                    message: error.to_string(),
                }
            })?;
            return Err(ModelListingError::api_error_with_context(
                "ChatGPT",
                MODELS_PATH,
                status_code,
                &body,
            ));
        }

        let body = response.into_body().await.map_err(|error| {
            ModelListingError::RequestError {
                message: error.to_string(),
            }
        })?;
        let parsed: CodexModelsResponse = serde_json::from_slice(&body).map_err(|error| {
            ModelListingError::parse_error_with_context("ChatGPT", MODELS_PATH, &error, &body)
        })?;
        let models = if parsed.models.is_empty() {
            parsed
                .data
                .into_iter()
                .map(|entry| Model::from_id(entry.id))
                .collect()
        } else {
            parsed
                .models
                .into_iter()
                .filter(|entry| entry.visibility.as_deref() == Some("list"))
                .map(|entry| match entry.display_name {
                    Some(name) if !name.is_empty() => Model::new(entry.slug, name),
                    _ => Model::from_id(entry.slug),
                })
                .collect()
        };

        Ok(ModelList::new(models))
    }
}

impl<H> ChatGptModelLister<H>
where
    H: HttpClientExt + Clone + Default + std::fmt::Debug + WasmCompatSend + WasmCompatSync + 'static,
{
    pub async fn fetch_catalog_document(
        &self,
        if_none_match: Option<&str>,
    ) -> Result<CodexCatalogDocument, ModelListingError> {
        let context = self
            .client
            .ext()
            .auth_context()
            .await
            .map_err(|error| ModelListingError::AuthError {
                message: error.to_string(),
            })?;
        let mut req = add_auth_headers(self.client.get(MODELS_PATH)?, &context);
        if let Some(headers) = req.headers_mut() {
            headers.insert(
                http::header::ACCEPT,
                http::HeaderValue::from_static("application/json"),
            );
            if let Some(etag) = if_none_match {
                if let Ok(value) = http::HeaderValue::from_str(etag) {
                    headers.insert(http::header::IF_NONE_MATCH, value);
                }
            }
        }
        let req = req.body(http_client::NoBody).map_err(|error| {
            ModelListingError::RequestError {
                message: error.to_string(),
            }
        })?;
        let response = self
            .client
            .send::<_, Vec<u8>>(req)
            .await
            .map_err(|error| ModelListingError::RequestError {
                message: error.to_string(),
            })?;
        if response.status() == http::StatusCode::NOT_MODIFIED {
            return Ok(CodexCatalogDocument::NotModified);
        }
        if !response.status().is_success() {
            let status_code = response.status().as_u16();
            let body = response.into_body().await.map_err(|error| {
                ModelListingError::RequestError {
                    message: error.to_string(),
                }
            })?;
            return Err(ModelListingError::api_error_with_context(
                "ChatGPT",
                MODELS_PATH,
                status_code,
                &body,
            ));
        }
        let etag = response
            .headers()
            .get(http::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(String::from);
        let body = response.into_body().await.map_err(|error| {
            ModelListingError::RequestError {
                message: error.to_string(),
            }
        })?;
        let body = String::from_utf8(body).map_err(|error| ModelListingError::RequestError {
            message: error.to_string(),
        })?;
        Ok(CodexCatalogDocument::Modified { body, etag })
    }
}


fn add_auth_headers(req: http_client::Builder, context: &AuthContext) -> http_client::Builder {
    let req = req.header(
        http::header::AUTHORIZATION,
        format!("Bearer {}", context.access_token),
    );
    if let Some(account_id) = &context.account_id {
        req.header("ChatGPT-Account-Id", account_id)
    } else {
        req
    }
}
