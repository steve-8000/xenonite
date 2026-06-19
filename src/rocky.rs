use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::RockyConfig;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(4);

pub async fn health(rocky: &RockyConfig) -> bool {
    let Ok(client) = Client::builder().timeout(HEALTH_TIMEOUT).build() else {
        return false;
    };
    client
        .get(format!(
            "{}/models",
            rocky.llm_base_url.trim_end_matches('/')
        ))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

pub async fn embed(rocky: &RockyConfig, input: &[String]) -> Result<Vec<Vec<f32>>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("build rocky embedding HTTP client")?;
    let response = client
        .post(format!(
            "{}/embeddings",
            rocky.embed_base_url.trim_end_matches('/')
        ))
        .bearer_auth(&rocky.embed_api_key)
        .json(&EmbeddingRequest {
            model: rocky.embed_model.clone(),
            input,
        })
        .send()
        .await
        .context("call rocky embeddings")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("rocky embed {status}: {body}");
    }

    let body = response
        .json::<EmbeddingResponse>()
        .await
        .context("parse rocky embeddings")?;
    Ok(body.data.into_iter().map(|item| item.embedding).collect())
}

pub async fn chat_json(rocky: &RockyConfig, system: &str, user: &str) -> Result<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("build rocky LLM HTTP client")?;
    let response = client
        .post(format!(
            "{}/chat/completions",
            rocky.llm_base_url.trim_end_matches('/')
        ))
        .bearer_auth(&rocky.llm_api_key)
        .json(&ChatCompletionRequest {
            model: rocky.llm_model.clone(),
            temperature: 0.0,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system,
                },
                ChatMessage {
                    role: "user",
                    content: user,
                },
            ],
        })
        .send()
        .await
        .context("call rocky chat completions")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("rocky chat {status}: {body}");
    }

    let body = response
        .json::<ChatCompletionResponse>()
        .await
        .context("parse rocky chat completions")?;
    let content = body
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .unwrap_or_default();
    Ok(extract_json_object(&content).unwrap_or(content))
}

fn extract_json_object(value: &str) -> Option<String> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    if end >= start {
        Some(value[start..=end].to_string())
    } else {
        None
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: String,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: String,
    temperature: f32,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}
