//! 网页轻量后端入口：公开只读 listener + 私网最小 listener。

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use kanalib_web::{AppState, crypto, db, routes, state::WriteConfig};

#[derive(Debug, Parser)]
#[command(
    name = "kanalib-web",
    version,
    about = "Kanalib 网页轻量后端（只读 public + 私网写入）"
)]
struct Args {
    /// 网页自己的 SQLite 路径。
    #[arg(long, value_name = "PATH")]
    db: PathBuf,

    /// 缩略图媒体根目录，内部布局为 t256/t512/t1024/t2048。
    #[arg(long, value_name = "PATH")]
    media: PathBuf,

    /// 可选：静态 SPA 目录（含 index.html）。
    #[arg(long, value_name = "PATH")]
    web: Option<PathBuf>,

    /// 公开只读 listener，默认仅回环；生产由 Nginx 反代。
    #[arg(long, default_value = "127.0.0.1:8787")]
    bind_public: SocketAddr,

    /// EasyTier 私网 listener；写路由与原图回源只在这里。
    #[arg(long, default_value = "127.0.0.1:8788")]
    bind_private: SocketAddr,

    /// box → 网页批次 HMAC 共享密钥；也可用 KANALIB_INGEST_SECRET。
    #[arg(long, value_name = "SECRET")]
    ingest_secret: Option<String>,

    /// box 原图回源根地址，例如 http://10.126.126.3:8789。
    #[arg(long, value_name = "URL")]
    box_origin: Option<String>,

    /// 原图签名密钥；也可用 KANALIB_MEDIA_SIGNING_KEY。
    #[arg(long, value_name = "SECRET")]
    media_signing_key: Option<String>,

    /// 桌面上传单文件上限（MiB）。
    #[arg(long, default_value_t = 32)]
    upload_limit_mb: u64,

    /// 原图签名 URL 有效期（秒）。
    #[arg(long, default_value_t = 300)]
    signed_url_ttl_secs: u64,

    /// 生成一枚桌面设备令牌并写入数据库，打印后退出。
    #[arg(long, value_name = "NAME")]
    issue_device: Option<String>,

    /// 写入固定样本数据，便于本地冒烟。
    #[arg(long)]
    seed_sample: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kanalib_web=info".into()),
        )
        .init();

    let args = Args::parse();

    if let Some(name) = args.issue_device.as_deref() {
        issue_device(&args.db, name)?;
        return Ok(());
    }

    if args.seed_sample {
        db::seed_sample(&args.db)
            .with_context(|| format!("seed sample database {}", args.db.display()))?;
    } else if !args.db.exists() {
        db::open(&args.db).with_context(|| format!("initialize database {}", args.db.display()))?;
    }
    if db::repair_search_index_if_needed(&args.db)
        .with_context(|| format!("repair search index {}", args.db.display()))?
    {
        tracing::warn!(path = %args.db.display(), "search index repaired on startup");
    }
    std::fs::create_dir_all(&args.media)
        .with_context(|| format!("create media root {}", args.media.display()))?;

    if let Some(web) = &args.web
        && !web.join("index.html").is_file()
    {
        tracing::warn!(path = %web.display(), "web root has no index.html");
    }

    let writes = WriteConfig {
        ingest_secret: secret(args.ingest_secret, "KANALIB_INGEST_SECRET"),
        box_origin: args
            .box_origin
            .map(|value| value.trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .map(Arc::from),
        media_signing_key: secret(args.media_signing_key, "KANALIB_MEDIA_SIGNING_KEY"),
        upload_limit_bytes: args.upload_limit_mb.saturating_mul(1024 * 1024),
        signed_url_ttl: Duration::from_secs(args.signed_url_ttl_secs.max(1)),
    };
    if writes.ingest_secret.is_none() {
        tracing::warn!("ingest shared secret is not configured; /internal/ingest/* returns 503");
    }
    if writes.box_origin.is_some() != writes.media_signing_key.is_some() {
        tracing::warn!("box origin and media signing key must both be set for original proxy");
    }
    if args.bind_private.ip().is_unspecified() {
        tracing::warn!(
            bind = %args.bind_private,
            "private listener is bound to all interfaces; bind the EasyTier virtual IP in production"
        );
    }

    let state = AppState::new(args.db, args.media, args.web).with_writes(writes);
    let public_listener = tokio::net::TcpListener::bind(args.bind_public)
        .await
        .with_context(|| format!("bind public listener {}", args.bind_public))?;
    tracing::info!(%args.bind_public, %args.bind_private, "kanalib-web listening");

    let public = axum::serve(public_listener, routes::public_router(state.clone()))
        .with_graceful_shutdown(shutdown_signal());
    let private_state = state;
    let private_addr = args.bind_private;
    tokio::spawn(async move {
        serve_private(private_addr, private_state).await;
    });

    public.await?;
    Ok(())
}

/// private listener 独立重试：EasyTier 未就绪时 public 只读照常服务。
async fn serve_private(addr: SocketAddr, state: AppState) {
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                tracing::info!(%addr, "private listener bound");
                let result = axum::serve(listener, routes::private_router(state.clone())).await;
                if let Err(error) = result {
                    tracing::warn!(%error, %addr, "private listener stopped; retrying");
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    %addr,
                    "private listener bind failed; public read stays up, retrying in 30s"
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

fn secret(flag: Option<String>, env: &str) -> Option<Arc<str>> {
    flag.or_else(|| std::env::var(env).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(Arc::from)
}

fn issue_device(db_path: &std::path::Path, name: &str) -> Result<()> {
    let mut token_bytes = [0u8; 32];
    getrandom::fill(&mut token_bytes)
        .map_err(|error| anyhow::anyhow!("generate device token: {error}"))?;
    let token = crypto::hex_lower(&token_bytes);
    let hash = crypto::sha256_hex(token.as_bytes());
    let id = format!("dev_{}", &hash[..16]);
    let conn = db::open(db_path).with_context(|| format!("open database {}", db_path.display()))?;
    db::insert_device(&conn, &id, name.trim(), &hash)?;
    println!("device_id: {id}");
    println!("device_name: {}", name.trim());
    println!("device_token: {token}");
    println!("令牌只显示这一次；桌面端粘贴后不要写入日志。");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
