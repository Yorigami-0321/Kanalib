//! 入库过滤服务入口：配置、启动与优雅退出；配置了网页地址时顺带排空推送队列。

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use axum::http::header;
use clap::Parser;
use image::{ImageFormat, RgbaImage};
use kanalib_ingest::{
    admin::{self, AdminState},
    config::{self, IngestConfig},
    db::{self, Db},
    download, ingest,
    media::{self, MediaState},
    metrics::Metrics,
    pipeline,
    push::{self, PushConfig, ReqwestPushTransport},
    rules_state::{ReloadOutcome, RulesState},
    source::{
        self, SourceAdapter,
        mock::MockSource,
        pixiv::{self, PixivSource, ReqwestPixivTransport},
        public::PublicSource,
    },
    tagging::{HttpTaggingBackend, TaggingBackend},
};
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(name = "kanalib-ingest")]
struct Args {
    /// `ingest.toml`；缺省时只使用安全默认值。
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    media_dir: Option<PathBuf>,
    /// `mock` 或 `pixiv`；未指定时由配置里的 `source.pixiv.enabled` 决定。
    #[arg(long, value_name = "NAME")]
    source: Option<String>,
    /// 可重复；与配置文件里的 `bookmark_users` 合并。
    #[arg(long = "bookmark-user", value_name = "USER_ID")]
    bookmark_user: Vec<String>,
    #[arg(long)]
    concurrency: Option<usize>,
    #[arg(long)]
    retry_max: Option<u32>,
    #[arg(long)]
    timeout_secs: Option<u64>,
    /// 网页 private listener；配置后每轮排空已持久化的推送批次。
    #[arg(long)]
    web_url: Option<String>,
    /// 网页 ingest 共享密钥；也可用 KANALIB_INGEST_PUSH_SECRET 或配置里的 `secret_file`。
    #[arg(long, value_name = "SECRET")]
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
    let config = config::load(args.config.as_deref())?;
    run(args, config).await
}

async fn run(args: Args, config: IngestConfig) -> Result<()> {
    let db_path = args.db.clone().unwrap_or_else(|| config.service.db.clone());
    let media_dir = args
        .media_dir
        .clone()
        .unwrap_or_else(|| config.service.media_dir.clone());
    let source_id = if let Some(source) = args.source.clone() {
        source
    } else if config.source.pixiv.enabled && config.source.public.enabled {
        bail!("enable only one of source.pixiv and source.public");
    } else if config.source.pixiv.enabled {
        "pixiv".to_string()
    } else if config.source.public.enabled {
        "public".to_string()
    } else {
        "mock".to_string()
    };
    if !matches!(source_id.as_str(), "mock" | "pixiv" | "public") {
        bail!("unknown source {source_id}; expected mock, pixiv or public");
    }
    if let Some(public_base) = &config.service.public_base {
        info!(public_base, "configured public base");
    }

    let mut db = Db::open(&db_path)?;
    // mock 来源没有真实内容要过滤，才允许用内置默认规则；真实来源缺规则文件一律拒绝启动。
    let rules_state = if source_id == "mock" && config.service.rules_file.is_none() {
        RulesState::load_without_file(&db)?
    } else {
        RulesState::load(&db, config.service.rules_file.as_deref())?
    };
    let rules_state = Arc::new(rules_state);
    let metrics = Arc::new(Metrics::default());
    let tagging_backend = match (&config.tagging.backend, &config.tagging.endpoint) {
        (Some(backend), Some(endpoint)) if backend == "http" => {
            let token = match &config.tagging.token_file {
                Some(path) => Some(config::read_secret(path)?),
                None => None,
            };
            Some(Arc::new(
                HttpTaggingBackend::new(
                    endpoint.clone(),
                    Duration::from_secs(config.tagging.timeout_secs.max(1)),
                    2,
                )?
                .with_token(token),
            ) as Arc<dyn TaggingBackend>)
        }
        (Some(backend), _) => bail!("unsupported tagging backend {backend}"),
        _ => None,
    };
    for batch_id in &args.replay_push {
        let found = push::replay(&db, batch_id)?;
        info!(batch_id, found, "push batch replay requested");
    }
    let recovered = download::recover(&db, &media_dir)?;
    let backfilled = download::backfill_thumbnails(&db, &media_dir)?;
    info!(
        schema = db::SCHEMA_VERSION,
        recovered,
        backfilled,
        db = %db_path.display(),
        source = %source_id,
        "database ready"
    );

    let download_timeout = Duration::from_secs(
        args.timeout_secs
            .unwrap_or(config.download.timeout_secs)
            .max(1),
    );
    let adapter: Arc<dyn SourceAdapter> = match source_id.as_str() {
        "pixiv" => Arc::new(build_pixiv_source(&config, download_timeout)?),
        "public" => Arc::new(PublicSource::new(&config.source.public, download_timeout)?),
        _ => Arc::new(MockSource::new("mock")),
    };
    let image_transport: Arc<dyn download::ImageTransport> = if source_id == "mock" {
        Arc::new(MockTransport::new()?)
    } else {
        Arc::new(ReqwestImageTransport::new(download_timeout)?)
    };

    let download_config = download::DownloadConfig {
        media_dir: media_dir.clone(),
        concurrency: args
            .concurrency
            .unwrap_or(config.download.concurrency)
            .max(1),
        retry_max: args.retry_max.unwrap_or(config.download.retry_max).max(1),
        timeout: download_timeout,
    };

    let push_config = build_push_config(&args, &config)?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    start_media_server(&config, &db_path, shutdown_rx.clone()).await?;
    start_admin_server(
        &config,
        &db_path,
        Arc::clone(&rules_state),
        Arc::clone(&metrics),
        shutdown_rx,
    )
    .await?;
    let (reload_tx, mut reload_rx) = tokio::sync::mpsc::channel::<()>(4);
    #[cfg(unix)]
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::hangup()) {
            Ok(mut handle) => {
                while handle.recv().await.is_some() {
                    if reload_tx.send(()).await.is_err() {
                        break;
                    }
                }
            }
            Err(error) => {
                warn!(%error, "SIGHUP handler unavailable; relying on file watch");
                std::future::pending::<()>().await;
            }
        }
    });
    #[cfg(not(unix))]
    let _reload_tx_guard = reload_tx;

    let mut bookmark_users = config.source.pixiv.bookmark_users.clone();
    bookmark_users.extend(args.bookmark_user.iter().cloned());
    bookmark_users.retain(|user| !user.trim().is_empty());

    loop {
        report_reload(rules_state.reload_if_changed(&db, "watch"), &rules_state);

        if source_id == "public" {
            match ingest::ingest_feed(&mut db, adapter.as_ref()).await {
                Ok(outcome) => info!(
                    pages = outcome.fetched_pages,
                    new_works = outcome.new_works,
                    "public feed synced"
                ),
                Err(error) => {
                    metrics.record_source_error();
                    warn!(%error, "public feed sync failed; keeping the service alive")
                }
            }
        } else {
            for user in &bookmark_users {
                match ingest::ingest_bookmarks(&mut db, adapter.as_ref(), user).await {
                    Ok(outcome) => info!(
                        user,
                        pages = outcome.fetched_pages,
                        new_works = outcome.new_works,
                        stopped_early = outcome.stopped_early,
                        "bookmarks synced"
                    ),
                    Err(error) => {
                        metrics.record_source_error();
                        warn!(user, %error, "bookmark sync failed; keeping the service alive")
                    }
                }
            }
        }

        match download::run_queue_limited(
            &db,
            adapter.as_ref(),
            image_transport.as_ref(),
            &download_config,
            if config.download.batch_items == 0 {
                usize::MAX
            } else {
                config.download.batch_items
            },
        )
        .await
        {
            Ok(report) => info!(
                succeeded = report.succeeded.len(),
                failed = report.failed.len(),
                "queue drained"
            ),
            Err(error) => warn!(%error, "download queue failed"),
        }

        let active_rules = rules_state.current();
        match pipeline::run_decisions_with_concurrency(
            &db,
            &active_rules.config,
            tagging_backend.as_ref(),
            metrics.as_ref(),
            config.tagging.concurrency.max(1),
        )
        .await
        {
            Ok(report) => info!(
                decided = report.decided,
                allowed = report.allowed,
                blocked = report.blocked,
                reviewed = report.reviewed,
                tagged = report.tagged,
                version = %active_rules.version,
                "rules applied"
            ),
            Err(error) => warn!(%error, "decision pass failed; keeping items pending"),
        }

        if let Some(push_config) = &push_config {
            let push_rules_version = config
                .push
                .rules_version
                .as_deref()
                .unwrap_or(active_rules.version.as_str());
            let enqueued = push::enqueue_decisions(
                &db,
                Some(push_rules_version),
                config.push.batch_size.max(1),
            )?;
            if enqueued.allow_items > 0 || enqueued.delete_items > 0 {
                info!(
                    allow_batches = enqueued.allow_batches,
                    allow_items = enqueued.allow_items,
                    delete_batches = enqueued.delete_batches,
                    delete_items = enqueued.delete_items,
                    "push batches enqueued"
                );
            }
            let transport = ReqwestPushTransport::new(push_config.timeout)?;
            let outcome = push::flush(
                &db,
                push_config,
                &transport,
                db::now(),
                PUSH_BATCHES_PER_ROUND,
            )
            .await?;
            metrics.record_push_failures(
                u64::try_from(outcome.retried + outcome.rejected).unwrap_or(u64::MAX),
            );
            if outcome.acked + outcome.retried + outcome.rejected > 0 {
                info!(
                    acked = outcome.acked,
                    retried = outcome.retried,
                    rejected = outcome.rejected,
                    "push queue flushed"
                );
            }
        }

        if args.once {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(
                config.service.sync_interval_secs.max(5),
            )) => {}
            _ = tokio::signal::ctrl_c() => {
                let _ = shutdown_tx.send(true);
                break;
            }
            _ = reload_rx.recv() => {
                info!("SIGHUP received; reloading rules");
                report_reload(rules_state.reload(&db, "sighup"), &rules_state);
            }
        }
    }
    let _ = shutdown_tx.send(true);
    info!("shutdown complete");
    Ok(())
}

const PUSH_BATCHES_PER_ROUND: usize = 10;

fn report_reload(result: Result<ReloadOutcome>, state: &RulesState) {
    match result {
        Ok(ReloadOutcome::Reloaded) => {
            let active = state.current();
            info!(
                version = %active.version,
                hash = %active.config_hash,
                "rules reloaded"
            );
        }
        Ok(ReloadOutcome::Unchanged) | Ok(ReloadOutcome::Skipped) => {}
        Err(error) => warn!(
            %error,
            "rules reload rejected; keeping the previous snapshot"
        ),
    }
}

fn build_push_config(args: &Args, config: &IngestConfig) -> Result<Option<PushConfig>> {
    let Some(base_url) = args
        .web_url
        .clone()
        .or_else(|| config.push.web_base.clone())
    else {
        return Ok(None);
    };
    let secret = if let Some(secret) = args
        .push_secret
        .clone()
        .or_else(|| std::env::var("KANALIB_INGEST_PUSH_SECRET").ok())
        .filter(|value| !value.trim().is_empty())
    {
        secret
    } else if let Some(path) = &config.push.secret_file {
        config::read_secret(path)?
    } else {
        bail!(
            "push is configured but no secret was found; set --push-secret, \
             KANALIB_INGEST_PUSH_SECRET, or push.secret_file"
        );
    };
    let mut push = PushConfig::new(base_url, secret);
    push.timeout = config.push.timeout();
    push.max_attempts = config.push.max_attempts.max(1);
    push.backoff_base = Duration::from_secs(config.push.backoff_base_secs.max(1));
    push.backoff_max = Duration::from_secs(config.push.backoff_max_secs.max(1));
    Ok(Some(push))
}

fn build_pixiv_source(config: &IngestConfig, timeout: Duration) -> Result<PixivSource> {
    let settings = &config.source.pixiv;
    if settings.credential_ref.is_some() {
        bail!(
            "source.pixiv.credential_ref is not implemented yet; use access_token_file or \
             refresh_token_file + client_id/client_secret"
        );
    }
    let user_agent = settings
        .user_agent
        .clone()
        .unwrap_or_else(|| pixiv::DEFAULT_USER_AGENT.to_string());
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(&user_agent)
        .build()
        .context("build pixiv HTTP client")?;
    let transport = match (&settings.access_token_file, &settings.refresh_token_file) {
        (Some(path), _) => {
            let token = config::read_secret(path)?;
            ReqwestPixivTransport::with_access_token(client, token)
        }
        (None, Some(path)) => {
            let refresh_token = config::read_secret(path)?;
            let client_id = settings
                .client_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("source.pixiv.client_id is required with refresh_token_file")
                })?;
            let client_secret = settings
                .client_secret
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "source.pixiv.client_secret is required with refresh_token_file"
                    )
                })?;
            ReqwestPixivTransport::with_refresh_token(
                client,
                client_id,
                client_secret,
                refresh_token,
                Some(path.clone()),
                Some(user_agent),
            )
        }
        (None, None) => bail!(
            "source.pixiv needs access_token_file or refresh_token_file; \
             see ingest.toml.example"
        ),
    };
    Ok(PixivSource::new(Arc::new(transport)))
}

async fn start_media_server(
    config: &IngestConfig,
    db_path: &std::path::Path,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let Some(key_file) = &config.media.signing_key_file else {
        warn!("media.signing_key_file is not configured; original endpoint is disabled");
        return Ok(());
    };
    let signing_key = config::read_secret(key_file)?;
    let bind = config.media.bind.unwrap_or(config.service.bind);
    if bind.ip().is_unspecified() {
        warn!(
            %bind,
            "original endpoint is bound to all interfaces; bind the EasyTier virtual IP in production"
        );
    }
    let router = media::router(MediaState::new(db_path.to_path_buf(), signing_key));
    tokio::spawn(async move {
        loop {
            match tokio::net::TcpListener::bind(bind).await {
                Ok(listener) => {
                    info!(%bind, "original endpoint listening");
                    let mut serve_shutdown = shutdown.clone();
                    let serve =
                        axum::serve(listener, router.clone()).with_graceful_shutdown(async move {
                            let _ = serve_shutdown.changed().await;
                        });
                    if let Err(error) = serve.await {
                        warn!(%error, "original endpoint stopped");
                    }
                }
                Err(error) => {
                    warn!(
                        %error,
                        %bind,
                        "original endpoint bind failed; retrying in 30s"
                    );
                }
            }
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                _ = shutdown.changed() => {}
            }
            if *shutdown.borrow() {
                break;
            }
        }
    });
    Ok(())
}

async fn start_admin_server(
    config: &IngestConfig,
    db_path: &std::path::Path,
    rules: Arc<RulesState>,
    metrics: Arc<Metrics>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    if !config.admin.enabled {
        info!("admin UI disabled by config");
        return Ok(());
    }
    let bind = config.admin.bind;
    if bind.ip().is_unspecified() {
        warn!(
            %bind,
            "admin UI is bound to all interfaces; bind the EasyTier virtual IP in production"
        );
    }
    let token = match &config.admin.token_file {
        Some(path) => Some(config::read_secret(path)?),
        None => None,
    };
    if token.is_none() && !bind.ip().is_loopback() {
        bail!(
            "admin.bind {bind} is not loopback; configure admin.token_file before exposing the admin API"
        );
    }
    if token.is_some() {
        info!("admin API token authentication enabled");
    }
    if !config.admin.allowed_ips.is_empty() {
        info!(
            count = config.admin.allowed_ips.len(),
            "admin IP allowlist enabled"
        );
    }
    let state = AdminState::new(
        db_path.to_path_buf(),
        rules,
        config.service.rules_file.clone(),
        metrics,
    )
    .with_auth(token, config.admin.allowed_ips.clone());
    let router = admin::router(state);
    tokio::spawn(async move {
        loop {
            match tokio::net::TcpListener::bind(bind).await {
                Ok(listener) => {
                    info!(%bind, "admin UI listening");
                    let mut serve_shutdown = shutdown.clone();
                    let serve =
                        axum::serve(listener, router.clone()).with_graceful_shutdown(async move {
                            let _ = serve_shutdown.changed().await;
                        });
                    if let Err(error) = serve.await {
                        warn!(%error, "admin UI stopped");
                    }
                }
                Err(error) => {
                    warn!(%error, %bind, "admin UI bind failed; retrying in 30s");
                }
            }
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                _ = shutdown.changed() => {}
            }
            if *shutdown.borrow() {
                break;
            }
        }
    });
    Ok(())
}

struct ReqwestImageTransport {
    client: reqwest::Client,
}

impl ReqwestImageTransport {
    fn new(timeout: Duration) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(pixiv::DEFAULT_USER_AGENT)
            .build()
            .context("build image HTTP client")?;
        Ok(Self { client })
    }
}

impl download::ImageTransport for ReqwestImageTransport {
    fn fetch<'a>(
        &self,
        url: &str,
        range_start: Option<u64>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<download::ImageResponse, source::SourceError>>
                + Send
                + 'a,
        >,
    > {
        let client = self.client.clone();
        let url = url.to_string();
        Box::pin(async move {
            let mut request = client
                .get(url)
                .header(header::REFERER, "https://www.pixiv.net/");
            if let Some(start) = range_start {
                request = request.header(header::RANGE, format!("bytes={start}-"));
            }
            let response = request
                .send()
                .await
                .map_err(|error| source::SourceError::network(error.to_string()))?;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let content_range = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let content_length = response.content_length();
            let body = response
                .bytes()
                .await
                .map_err(|error| source::SourceError::network(error.to_string()))?
                .to_vec();
            Ok(download::ImageResponse {
                status,
                content_type,
                content_range,
                content_length,
                body,
            })
        })
    }
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
