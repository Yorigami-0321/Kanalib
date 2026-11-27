//! 把本地目录里的图片发布到 kanalib-web（private listener + 设备令牌）。
//!
//! 两阶段：先写元数据（条目保持 hidden），再上传原图；web 端生成缩略图、
//! 建搜索索引并放上墙。重复运行按 sha1 去重。

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use reqwest::blocking::Client;
use serde::Serialize;
use sha1::{Digest, Sha1};

const DEFAULT_WEB: &str = "http://10.126.126.2:8788";
const META_CHUNK: usize = 50;

#[derive(Debug, Parser)]
#[command(
    name = "kanalib-sync",
    about = "把本地图片发布到 kanalib-web（建条目 + 传原图 + 生成缩略图）"
)]
struct Args {
    /// private listener 基址。
    #[arg(long, default_value = DEFAULT_WEB)]
    web: String,
    /// 设备令牌文件（内容为 `kanalib-web --issue-device` 打印的 token）。
    #[arg(long, value_name = "PATH", env = "KANALIB_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    /// 直接给令牌；优先于 `--token-file`。
    #[arg(
        long,
        value_name = "TOKEN",
        env = "KANALIB_DEVICE_TOKEN",
        hide_env_values = true
    )]
    token: Option<String>,
    /// 待发布目录，可重复。
    #[arg(long = "dir", value_name = "PATH", required = true)]
    dirs: Vec<PathBuf>,
    /// 递归子目录。
    #[arg(long)]
    recursive: bool,
    /// 最多处理多少张（调试用）。
    #[arg(long)]
    limit: Option<usize>,
    /// 只扫描并打印，不发请求。
    #[arg(long)]
    dry_run: bool,
    /// 写入网页的 group 字段（默认 `local`）。
    #[arg(long, default_value = "local")]
    group: String,
    /// 默认分级。
    #[arg(long, default_value = "G")]
    rating: String,
    /// 附加标签，可重复；格式 `name` 或 `name:category`。
    #[arg(long = "tag", value_name = "NAME[:CATEGORY]")]
    tags: Vec<String>,
    /// 默认星级 0-5。
    #[arg(long, default_value_t = 0)]
    star: u8,
    /// 标记为收藏。
    #[arg(long)]
    favorite: bool,
}

#[derive(Serialize)]
struct ItemMeta {
    id: String,
    name: String,
    title: String,
    width: u32,
    height: u32,
    rating: String,
    star: u8,
    favorite: bool,
    mtime: i64,
    posted_at: i64,
    orig: String,
    group: String,
    tags: Vec<Tag>,
    work_id: String,
    work_title: String,
    page: u32,
    page_count: u32,
    source: String,
}

#[derive(Clone, Serialize)]
struct Tag {
    name: String,
    category: String,
    score: f32,
}

struct Candidate {
    path: PathBuf,
    id: String,
    name: String,
    ext: String,
    width: u32,
    height: u32,
    mtime: i64,
    bytes: u64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("kanalib-sync: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let base = args.web.trim_end_matches('/').to_string();
    let token = resolve_token(&args)?;
    if token.is_empty() && !args.dry_run {
        bail!("缺少设备令牌：传 --token-file / --token，或先运行 kanalib-web --issue-device");
    }
    let client = Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .context("build http client")?;

    let mut candidates = Vec::new();
    for dir in &args.dirs {
        collect(dir, args.recursive, &mut candidates)
            .with_context(|| format!("scan {}", dir.display()))?;
    }
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(limit) = args.limit {
        candidates.truncate(limit);
    }
    if candidates.is_empty() {
        println!("没有找到可发布的图片");
        return Ok(());
    }
    println!(
        "发现 {} 张图片，目标 {}{}",
        candidates.len(),
        base,
        if args.dry_run { "（dry-run）" } else { "" }
    );

    if args.dry_run {
        for candidate in &candidates {
            println!(
                "  {} -> {} ({}x{}, {} bytes)",
                candidate.path.display(),
                candidate.id,
                candidate.width,
                candidate.height,
                candidate.bytes
            );
        }
        return Ok(());
    }

    let tags = parse_tags(&args.tags)?;
    let mut failed = 0usize;
    let mut uploaded = 0usize;
    let total = candidates.len();
    for (index, chunk) in candidates.chunks(META_CHUNK).enumerate() {
        let offset = index * META_CHUNK;
        let metas = chunk
            .iter()
            .map(|candidate| meta(candidate, &args, &tags))
            .collect::<Vec<_>>();
        let response = client
            .post(format!("{base}/api/desktop/items"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "items": metas }))
            .send()
            .context("POST /api/desktop/items")?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!("设备令牌无效或缺少写能力（{status}）：{body}");
        }
        if !status.is_success() {
            bail!("写入元数据失败（{status}）：{body}");
        }
        let value: serde_json::Value =
            serde_json::from_str(&body).context("parse items response")?;
        let results = value["results"].as_array().cloned().unwrap_or_default();
        for result in results {
            let id = result["id"].as_str().unwrap_or_default().to_string();
            let state = result["status"].as_str().unwrap_or("error");
            if state == "rejected" || state == "error" || state == "exists" {
                if state == "exists" {
                    println!("[{}/{}] {id} 已存在（非本地来源），跳过", offset + 1, total);
                } else {
                    failed += 1;
                    println!(
                        "[{}/{}] {id} 元数据被拒绝：{}",
                        offset + 1,
                        total,
                        result["error"].as_str().unwrap_or("unknown")
                    );
                }
                continue;
            }
            let Some((chunk_index, candidate)) = chunk
                .iter()
                .enumerate()
                .find(|(_, candidate)| candidate.id == id)
            else {
                continue;
            };
            let position = offset + chunk_index;
            let started = Instant::now();
            match upload(&client, &base, &token, candidate) {
                Ok(()) => {
                    uploaded += 1;
                    println!(
                        "[{}/{}] {} -> allow（{} bytes，{} ms）",
                        position + 1,
                        total,
                        candidate.name,
                        candidate.bytes,
                        started.elapsed().as_millis()
                    );
                }
                Err(error) => {
                    failed += 1;
                    println!(
                        "[{}/{}] {} 上传失败：{error:#}",
                        position + 1,
                        total,
                        candidate.name
                    );
                }
            }
        }
    }

    println!("完成：上传 {uploaded} 张，失败 {failed} 张，共 {total} 张");
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn resolve_token(args: &Args) -> Result<String> {
    if let Some(token) = args
        .token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        return Ok(token.to_string());
    }
    match args.token_file.as_deref() {
        Some(path) => Ok(std::fs::read_to_string(path)
            .with_context(|| format!("read token file {}", path.display()))?
            .trim()
            .to_string()),
        None => Ok(std::env::var("KANALIB_DEVICE_TOKEN")
            .unwrap_or_default()
            .trim()
            .to_string()),
    }
}

fn collect(dir: &Path, recursive: bool, out: &mut Vec<Candidate>) -> Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)
            .with_context(|| format!("read dir {}", current.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                if recursive {
                    stack.push(path);
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(ext) = supported_ext(&path) else {
                continue;
            };
            let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            let (width, height) = image::image_dimensions(&path)
                .with_context(|| format!("read dimensions {}", path.display()))?;
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_else(unix_now);
            out.push(Candidate {
                id: sha1_hex(&bytes),
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("image")
                    .to_string(),
                ext,
                width,
                height,
                mtime,
                bytes: bytes.len() as u64,
                path,
            });
        }
    }
    Ok(())
}

fn supported_ext(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif").then_some(ext)
}

fn meta(candidate: &Candidate, args: &Args, tags: &[Tag]) -> ItemMeta {
    let title = Path::new(&candidate.name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&candidate.name)
        .to_string();
    ItemMeta {
        id: candidate.id.clone(),
        name: candidate.name.clone(),
        title: title.clone(),
        width: candidate.width,
        height: candidate.height,
        rating: args.rating.clone(),
        star: args.star.min(5),
        favorite: args.favorite,
        mtime: candidate.mtime,
        posted_at: candidate.mtime,
        orig: candidate.ext.clone(),
        group: args.group.clone(),
        tags: tags.to_vec(),
        work_id: format!("local-{}", candidate.id),
        work_title: title,
        page: 1,
        page_count: 1,
        source: "local".to_string(),
    }
}

fn upload(client: &Client, base: &str, token: &str, candidate: &Candidate) -> Result<()> {
    let bytes = std::fs::read(&candidate.path)
        .with_context(|| format!("read {}", candidate.path.display()))?;
    let response = client
        .post(format!("{base}/api/desktop/upload"))
        .bearer_auth(token)
        .header("x-kanalib-item-id", &candidate.id)
        .header("x-kanalib-ext", &candidate.ext)
        .header("content-type", "application/octet-stream")
        .body(bytes)
        .send()
        .context("POST /api/desktop/upload")?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().unwrap_or_default();
    bail!("{status}: {body}");
}

fn parse_tags(raw: &[String]) -> Result<Vec<Tag>> {
    let mut tags = Vec::new();
    for entry in raw {
        let (name, category) = entry
            .split_once(':')
            .map(|(name, category)| (name.trim(), category.trim()))
            .unwrap_or((entry.trim(), "general"));
        if name.is_empty() || name.len() > 256 || category.len() > 64 {
            bail!("invalid tag {entry:?}");
        }
        tags.push(Tag {
            name: name.to_string(),
            category: category.to_string(),
            score: 0.0,
        });
    }
    Ok(tags)
}

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
