use crate::{
    client::ModelLister,
    http_client::{self, HttpClientExt},
    model::{Model, ModelList, ModelListingError},
    providers::openai::Client,
    wasm_compat::{WasmCompatSend, WasmCompatSync},
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ListModelsResponse {
    data: Vec<ListModelEntry>,
}

/// OpenAI documents `created` and `owned_by` as always present, but
/// OpenAI-compatible gateways omit them; only `id` is required.
#[derive(Debug, Deserialize)]
struct ListModelEntry {
    id: String,
    created: Option<u64>,
    owned_by: Option<String>,
}

impl From<ListModelEntry> for Model {
    fn from(value: ListModelEntry) -> Self {
        let mut model = Model::from_id(value.id);
        model.created_at = value.created;
        model.owned_by = value.owned_by;
        model
    }
}

/// [`ModelLister`] implementation for the OpenAI API (`GET /models`).
#[derive(Clone)]
pub struct OpenAIModelLister<H = reqwest::Client> {
    client: Client<H>,
}

impl<H> ModelLister<H> for OpenAIModelLister<H>
where
    H: HttpClientExt + WasmCompatSend + WasmCompatSync + 'static,
{
    type Client = Client<H>;

    fn new(client: Self::Client) -> Self {
        Self { client }
    }

    async fn list_all(&self) -> Result<ModelList, ModelListingError> {
        let path = "/models";
        let req = self.client.get(path)?.body(http_client::NoBody)?;
        let response = self.client.send::<_, Vec<u8>>(req).await?;

        if !response.status().is_success() {
            let status_code = response.status().as_u16();
            let body = response.into_body().await?;
            return Err(ModelListingError::api_error_with_context(
                "OpenAI",
                path,
                status_code,
                &body,
            ));
        }

        let body = response.into_body().await?;
        let api_resp: ListModelsResponse = serde_json::from_slice(&body).map_err(|error| {
            ModelListingError::parse_error_with_context("OpenAI", path, &error, &body)
        })?;
        let models = api_resp.data.into_iter().map(Model::from).collect();

        Ok(ModelList::new(models))
    }
}

#[cfg(test)]
mod tests {
    use super::ListModelsResponse;
    use crate::model::Model;

    #[test]
    fn compatible_listing_without_created_or_owner_parses() {
        // OpenAI-compatible gateways often omit `created` and `owned_by`.
        let body = br#"{"object":"list","data":[
            {"id":"openai-codex/gpt-6.1-sol","object":"model","owned_by":"openai-codex"},
            {"id":"bare-model"},
            {"id":"gpt-4o","object":"model","created":1715367049,"owned_by":"system"}
        ]}"#;
        let response: ListModelsResponse =
            serde_json::from_slice(body).expect("compatible listing should parse");
        let models: Vec<Model> = response.data.into_iter().map(Model::from).collect();
        let summary: Vec<_> = models
            .iter()
            .map(|model| (model.id.as_str(), model.created_at, model.owned_by.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                ("openai-codex/gpt-6.1-sol", None, Some("openai-codex")),
                ("bare-model", None, None),
                ("gpt-4o", Some(1_715_367_049), Some("system")),
            ]
        );
    }
}
