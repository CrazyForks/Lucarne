//! Thin frankenstein [`AsyncTelegramApi`] client on workspace `reqwest` 0.12.
//!
//! frankenstein's built-in `client-reqwest` feature pins reqwest 0.13 with
//! rustls/aws-lc, which fails to cross-compile for `aarch64-pc-windows-msvc`
//! in cargo-dist. The rest of Lucarne already uses native-tls on reqwest 0.12
//! (wechat-ilink), so we implement the async trait ourselves and keep
//! frankenstein for types + API methods only.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use frankenstein::response::ErrorResponse;
use frankenstein::AsyncTelegramApi;
use reqwest::multipart;
use serde_json::Value;

/// Errors from the Telegram HTTP client (API + transport + JSON).
#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error("Api Error {0:?}")]
    Api(ErrorResponse),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("JSON error: {0}")]
    Json(String),
    #[error("Read File Error: {0}")]
    ReadFile(#[source] std::io::Error),
}

/// Asynchronous Telegram Bot API client.
#[derive(Debug, Clone)]
pub struct Bot {
    pub api_url: String,
    pub client: reqwest::Client,
}

impl Bot {
    pub fn new(api_key: &str) -> Self {
        Self::new_url(format!("{}{api_key}", frankenstein::BASE_API_URL))
    }

    pub fn new_url(api_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(500))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_url: api_url.into(),
            client,
        }
    }

    /// Use a shared daemon HTTP client (workspace reqwest 0.12 / native-tls).
    pub fn with_client(api_url: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            api_url: api_url.into(),
            client,
        }
    }

    async fn decode_response<Output>(response: reqwest::Response) -> Result<Output, BotError>
    where
        Output: serde::de::DeserializeOwned,
    {
        let success = response.status().is_success();
        let message = response
            .text()
            .await
            .map_err(|e| BotError::Http(e.without_url().to_string()))?;
        if success {
            serde_json::from_str(&message)
                .map_err(|e| BotError::Json(format!("{e} on {message}")))
        } else {
            match serde_json::from_str::<ErrorResponse>(&message) {
                Ok(api) => Err(BotError::Api(api)),
                Err(e) => Err(BotError::Json(format!("{e} on {message}"))),
            }
        }
    }
}

#[async_trait]
impl AsyncTelegramApi for Bot {
    type Error = BotError;

    async fn request<Params, Output>(
        &self,
        method: &str,
        params: Option<Params>,
    ) -> Result<Output, Self::Error>
    where
        Params: serde::ser::Serialize + std::fmt::Debug + std::marker::Send,
        Output: serde::de::DeserializeOwned,
    {
        let url = format!("{}/{method}", self.api_url);
        let mut prepared = self
            .client
            .post(url)
            .header("Content-Type", "application/json");
        if let Some(params) = params {
            let json_string = serde_json::to_string(&params)
                .map_err(|e| BotError::Json(format!("{e} on {params:?}")))?;
            prepared = prepared.body(json_string);
        }
        let response = prepared
            .send()
            .await
            .map_err(|e| BotError::Http(e.without_url().to_string()))?;
        Self::decode_response(response).await
    }

    async fn request_with_form_data<Params, Output>(
        &self,
        method: &str,
        params: Params,
        files: Vec<(&str, PathBuf)>,
    ) -> Result<Output, Self::Error>
    where
        Params: serde::ser::Serialize + std::fmt::Debug + std::marker::Send,
        Output: serde::de::DeserializeOwned,
    {
        let json_string = serde_json::to_string(&params)
            .map_err(|e| BotError::Json(format!("{e} on {params:?}")))?;
        let json_struct: serde_json::Map<String, Value> = serde_json::from_str(&json_string)
            .map_err(|e| BotError::Json(format!("{e} on {json_string}")))?;
        let file_keys: Vec<&str> = files.iter().map(|(key, _)| *key).collect();

        let mut form = multipart::Form::new();
        for (key, val) in json_struct {
            if !file_keys.contains(&key.as_str()) {
                form = match val {
                    Value::String(val) => form.text(key, val),
                    other => form.text(key, other.to_string()),
                };
            }
        }

        for (parameter_name, file_path) in files {
            let bytes = tokio::fs::read(&file_path)
                .await
                .map_err(BotError::ReadFile)?;
            let file_name = file_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let part = multipart::Part::bytes(bytes).file_name(file_name);
            form = form.part(parameter_name.to_owned(), part);
        }

        let url = format!("{}/{method}", self.api_url);
        let response = self
            .client
            .post(url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| BotError::Http(e.without_url().to_string()))?;
        Self::decode_response(response).await
    }
}
