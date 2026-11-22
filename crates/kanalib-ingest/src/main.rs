//! 入库过滤服务入口：配置、启动与优雅退出；配置了网页地址时顺带排空推送队列。

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;
use image::{ImageFormat, RgbaImage};
use kanalib_ingest::{
    db::{self, Db},
    download, ingest,
    push::{self, PushConfig, ReqwestPushTransport},
    source,
};
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "kanalib-ingest")]
struct Args {
    #[arg(long, default_value = "ingest.db")]
    db: PathBuf,
    #[arg(long, default_value = "media")]
    media_dir: PathBuf,
    #[arg(long, default_value = "mock")]
    source: String,
    #[arg(long)]
    bookmark_user: Option<String>,
    #[arg(long, default_value_t = 3)]
    concurrency: usize,
    #[arg(long, default_value_t = 5)]
    retry_max: u32,
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
    /// 网页 private listener；配置后每轮排空已持久化的推送批次。
    #[arg(long)]
    web_url: Option<String>,
    /// 网页 ingest 共享密钥；也可用 KANALIB_INGEST_PUSH_SECRET。
    #[arg(long)]
    push_secret: Option<String>,
    /// 重放指定推送批次（可重复）；启动时重置为 pending。
    #[arg(long = "replay-push", value_name = "BATCH_ID")]
    replay_push: Vec<String>,
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    if args.source != "mock" {
        bail!("I0/I1 only wires the mock source; got {}", args.source);
    }
    let db = Db::open(&args.db)?;
    for batch_id in &args.replay_push {
        let found = push::replay(&db, batch_id)?;
        info!(batch_id, found, "push batch replay requested");
    }
    let recovered = download::recover(&db, &args.media_dir)?;
    info!(schema = db::SCHEMA_VERSION, recovered, "database ready");

    let adapter = source::mock::MockSource::new("mock");
    let mut db = db;
    let config = download::DownloadConfig {
        media_dir: args.media_dir,
        concurrency: args.concurrency,
        retry_max: args.retry_max,
        timeout: std::time::Duration::from_secs(args.timeout_secs),
    };
    loop {
        if let Some(user) = args.bookmark_user.as_deref() {
            let outcome = ingest::ingest_bookmarks(&mut db, &adapter, user).await?;
            info!(
                pages = outcome.fetched_pages,
                new_works = outcome.new_works,
                stopped_early = outcome.stopped_early,
                "bookmarks synced"
            );
        }
        let transport = MockTransport::new()?;
        let report = download::run_queue(&db, &adapter, &transport, &config).await?;
        info!(
            succeeded = report.succeeded.len(),
            failed = report.failed.len(),
            "queue drained"
        );
        if let Some(url) = args.web_url.as_deref() {
            let secret = args
                .push_secret
                .clone()
                .or_else(|| std::env::var("KANALIB_INGEST_PUSH_SECRET").ok())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow::anyhow!("--push-secret is required with --web-url"))?;
            let config = PushConfig::new(url, secret);
            let transport = ReqwestPushTransport::new(config.timeout)?;
            let outcome = push::flush(&db, &config, &transport, db::now(), 100).await?;
            info!(
                acked = outcome.acked,
                retried = outcome.retried,
                rejected = outcome.rejected,
                "push queue flushed"
            );
        }

        if args.once {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    info!("shutdown complete");
    Ok(())
}

struct MockTransport {
    bytes: Vec<u8>,
}

impl MockTransport {
    fn new() -> Result<Self> {
        let image = RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba([x as u8 * 4, y as u8 * 4, 128, 255])
        });
        let mut bytes = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)?;
        Ok(Self { bytes })
    }
}

impl download::ImageTransport for MockTransport {
    fn fetch<'a>(
        &self,
        _url: &str,
        _range_start: Option<u64>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<download::ImageResponse, source::SourceError>>
                + Send
                + 'a,
        >,
    > {
        let bytes = self.bytes.clone();
        Box::pin(async move { Ok(download::ImageResponse::ok(bytes, "image/png")) })
    }
}
