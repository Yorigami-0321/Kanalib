//! I2 打标后端：HTTP sidecar 协议、超时重试与失败语义。
//!
//! 失败/超时不产生标签，也不允许上层据此放行；调用方必须把条目转 review。

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::task;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggingResult {
    pub tags: Vec<String>,
}

pub trait TaggingBackend: Send + Sync {
    fn tag<'a>(
        &'a self,
        item_id: &'a str,
        image_path: &'a std::path::Path,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<TaggingResult, anyhow::Error>> + Send + 'a>,
    >;
}

#[derive(Debug)]
pub struct HttpTaggingBackend {
    endpoint: String,
    timeout: Duration,
    attempts: usize,
}

impl HttpTaggingBackend {
    pub fn new(endpoint: impl Into<String>, timeout: Duration, attempts: usize) -> Result<Self> {
        let endpoint = endpoint.into();
        if endpoint.trim().is_empty() {
            bail!("tagging endpoint is empty");
        }
        Ok(Self {
            endpoint,
            timeout,
            attempts: attempts.clamp(1, 3),
        })
    }
}

#[derive(Debug, Deserialize)]
struct TagResponse {
    #[serde(default)]
    tags: Vec<String>,
}

impl TaggingBackend for HttpTaggingBackend {
    fn tag<'a>(
        &'a self,
        item_id: &'a str,
        image_path: &'a std::path::Path,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<TaggingResult, anyhow::Error>> + Send + 'a>,
    > {
        Box::pin(self.tag_inner(item_id, image_path))
    }
}

impl HttpTaggingBackend {
    async fn tag_inner(
        &self,
        item_id: &str,
        image_path: &std::path::Path,
    ) -> Result<TaggingResult, anyhow::Error> {
        let path = image_path.to_path_buf();
        let item_id = item_id.to_string();
        let endpoint = self.endpoint.clone();
        let timeout = self.timeout;
        let attempts = self.attempts;
        task::spawn_blocking(move || {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading image {}", path.display()))?;
            let mut last_error = None;
            for _ in 0..attempts {
                let form = reqwest::blocking::multipart::Form::new().part(
                    "image",
                    reqwest::blocking::multipart::Part::bytes(bytes.clone()).file_name(
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("image")
                            .to_string(),
                    ),
                );
                let blocking_client = reqwest::blocking::Client::builder()
                    .timeout(timeout)
                    .build()
                    .context("build blocking tagging client")?;
                let request = blocking_client
                    .post(&endpoint)
                    .header("x-kanalib-item-id", &item_id)
                    .multipart(form);
                match request.send() {
                    Ok(response) => {
                        let status = response.status();
                        let body = response.bytes().context("read tagging response")?;
                        if !status.is_success() {
                            last_error = Some(anyhow::anyhow!(
                                "tagging endpoint returned {}",
                                status.as_u16()
                            ));
                        } else {
                            let parsed: TagResponse =
                                serde_json::from_slice(&body).context("parse tagging response")?;
                            let tags = parsed
                                .tags
                                .into_iter()
                                .map(|tag| tag.trim().to_string())
                                .filter(|tag| !tag.is_empty())
                                .collect::<Vec<_>>();
                            if tags.is_empty() {
                                return Err(anyhow::anyhow!("tagging endpoint returned no tags"));
                            }
                            return Ok(TaggingResult { tags });
                        }
                    }
                    Err(error) => last_error = Some(anyhow::Error::from(error)),
                }
            }
            Err(last_error.unwrap_or_else(|| anyhow::anyhow!("tagging backend failed")))
        })
        .await
        .context("tagging task failed")?
    }
}
