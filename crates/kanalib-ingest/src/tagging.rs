//! I2 打标后端：HTTP sidecar 协议、超时重试与失败语义。
//!
//! 失败/超时不产生标签，也不允许上层据此放行；调用方必须把条目转 review。

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

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
    client: reqwest::Client,
    endpoint: String,
    attempts: usize,
    token: Option<String>,
}

impl HttpTaggingBackend {
    pub fn new(endpoint: impl Into<String>, timeout: Duration, attempts: usize) -> Result<Self> {
        let endpoint = endpoint.into();
        if endpoint.trim().is_empty() {
            bail!("tagging endpoint is empty");
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .context("build tagging client")?;
        Ok(Self {
            client,
            endpoint,
            attempts: attempts.clamp(1, 3),
            token: None,
        })
    }

    /// 打标服务的共享 token；与 tagger 的 `--token-file` 相同。
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|value| !value.trim().is_empty());
        self
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
        let length = tokio::fs::metadata(image_path)
            .await
            .with_context(|| format!("reading image {}", image_path.display()))?
            .len();
        if length == 0 {
            bail!("image {} is empty", image_path.display());
        }
        let mut last_error = None;
        for _ in 0..self.attempts {
            // 直接从磁盘流式发送，整图不再进内存；重试时重新打开文件。
            let part = reqwest::multipart::Part::file(image_path)
                .await
                .with_context(|| format!("reading image {}", image_path.display()))?;
            let form = reqwest::multipart::Form::new().part("image", part);
            let mut request = self
                .client
                .post(&self.endpoint)
                .header("x-kanalib-item-id", item_id)
                .multipart(form);
            if let Some(token) = &self.token {
                request = request.header("x-tagger-token", token);
            }
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    let body = response.bytes().await.context("read tagging response")?;
                    if !status.is_success() {
                        let error =
                            anyhow::anyhow!("tagging endpoint returned {}", status.as_u16());
                        // 4xx 是请求/配置问题，重试只会重复等待；立刻失败让上层转 review。
                        if status.is_client_error() {
                            return Err(error);
                        }
                        last_error = Some(error);
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
    }
}
