//! F-080 契约验收测试：临时库 + 临时导出目录，跑完整发布流程后断言不变量。
//!
//! 覆盖 Lead 要求：分片切分边界（0 条 / 恰好 shard_size / 多一条）、--dry-run 不写盘、
//! 账本二次运行跳过未变资产、导出目录内不含任何绝对路径（含 t256/t512 文件名）。

use std::fs;
use std::path::{Path, PathBuf};

use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, Frame, Rgb, RgbImage, Rgba, RgbaImage};
use kanalib_core::db::Db;
use kanalib_core::media::ContentHashes;
use kanalib_core::model::TagHit;
use kanalib_publish::{Options, run};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kanalib-publish-test-{tag}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Fixture {
    dir: TempDir,
    root: PathBuf,
    thumbs: PathBuf,
    db_path: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let root = dir.join("lib");
        let thumbs = dir.join("thumbs");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&thumbs).unwrap();
        let db_path = dir.join("kanalib.db");
        Self {
            dir,
            root,
            thumbs,
            db_path,
        }
    }

    fn out(&self) -> PathBuf {
        self.dir.join("out")
    }

    fn open(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    fn options(&self) -> Options {
        Options {
            db_path: self.db_path.clone(),
            out_dir: self.out(),
            roots: vec![self.root.clone()],
            ..Options::default()
        }
    }

    fn stem(name: &str) -> String {
        Path::new(name)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }

    fn thumb_path(&self, name: &str) -> PathBuf {
        self.thumbs.join(format!("{}-512.jpg", Self::stem(name)))
    }

    fn tile_path(&self, name: &str) -> PathBuf {
        self.thumbs
            .join(format!("{}-512-t256.jpg", Self::stem(name)))
    }

    /// 写入一条资产（原图 + t512 缩略图 + 可选 t256），返回原图路径。
    fn add(&self, db: &Db, name: &str, mtime: u64, sha1: &str, tile: bool) -> PathBuf {
        self.add_sized(db, name, 1200, 900, mtime, sha1, tile)
    }

    /// 同上，但可以指定原图尺寸（锐利档需要真实可解码的图片）。
    fn add_sized(
        &self,
        db: &Db,
        name: &str,
        width: u32,
        height: u32,
        mtime: u64,
        sha1: &str,
        tile: bool,
    ) -> PathBuf {
        let original = self.root.join(name);
        if let Some(parent) = original.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 80])
        })
        .save(&original)
        .unwrap();
        let bytes = fs::metadata(&original).unwrap().len();
        let thumb = self.thumb_path(name);
        fs::write(&thumb, b"thumb512").unwrap();
        if tile {
            fs::write(self.tile_path(name), b"thumb256").unwrap();
        }
        let hashes = ContentHashes {
            sha1: sha1.to_string(),
            blake3: if sha1.is_empty() {
                String::new()
            } else {
                format!("blake3-of-{name}")
            },
            phash: 7,
            decode_ok: true,
            decode_error: None,
        };
        let root = self.root.to_string_lossy().to_string();
        db.upsert_meta(
            &root,
            &original,
            Some(&thumb),
            width,
            height,
            bytes,
            mtime,
            &hashes,
            false,
        )
        .unwrap();
        original
    }

    /// 按 core 缩略图的同一规则生成 512/256 档：重解码首帧、alpha 合成白底、保存 JPEG。
    ///
    /// 白带检测先用缩略图做候选，所以夹具必须写**真实可解码**的缩略图；
    /// 用 `b"thumb512"` 这类占位字节会让候选阶段直接失败，测不到源图侧逻辑。
    fn write_core_thumbs(&self, name: &str, image: &DynamicImage) {
        for (path, edge) in [
            (self.thumb_path(name), 512u32),
            (self.tile_path(name), 256u32),
        ] {
            let scaled = image.thumbnail(edge, edge);
            let rgba = scaled.to_rgba8();
            let mut rgb = RgbImage::new(rgba.width(), rgba.height());
            for (source, target) in rgba.pixels().zip(rgb.pixels_mut()) {
                let alpha = u32::from(source[3]);
                let inverse = 255 - alpha;
                *target = Rgb([
                    ((u32::from(source[0]) * alpha + 255 * inverse) / 255) as u8,
                    ((u32::from(source[1]) * alpha + 255 * inverse) / 255) as u8,
                    ((u32::from(source[2]) * alpha + 255 * inverse) / 255) as u8,
                ]);
            }
            let mut bytes = Vec::new();
            JpegEncoder::new_with_quality(&mut bytes, 90)
                .encode_image(&rgb)
                .unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    /// 把落盘后的源文件重新解码后再生成缩略图/登记资产（与实际扫描链路一致）。
    fn register_image(&self, db: &Db, name: &str, mtime: u64, sha1: &str) -> PathBuf {
        let original = self.root.join(name);
        let decoded = image::open(&original).unwrap();
        self.write_core_thumbs(name, &decoded);
        let bytes = fs::metadata(&original).unwrap().len();
        let hashes = ContentHashes {
            sha1: sha1.to_string(),
            blake3: format!("blake3-of-{name}"),
            phash: 7,
            decode_ok: true,
            decode_error: None,
        };
        let root = self.root.to_string_lossy().to_string();
        db.upsert_meta(
            &root,
            &original,
            Some(&self.thumb_path(name)),
            decoded.width(),
            decoded.height(),
            bytes,
            mtime,
            &hashes,
            false,
        )
        .unwrap();
        original
    }
}

/// 写一张指定质量的 JPEG 源图（白带夹具需要控制压缩对纯白行的影响）。
fn write_jpeg_source(path: &Path, image: &RgbImage, quality: u8) {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(image)
        .unwrap();
    fs::write(path, bytes).unwrap();
}

/// 逐帧写动画 GIF（image::save 只会写单帧）。
fn write_animated_gif(path: &Path, frames: &[RgbaImage]) {
    let file = fs::File::create(path).unwrap();
    let mut encoder = GifEncoder::new(file);
    encoder
        .encode_frames(frames.iter().cloned().map(Frame::new))
        .unwrap();
}

/// 画一张带底部纯白带的 RGB 图。
fn image_with_white_bottom(width: u32, height: u32, band: u32) -> RgbImage {
    let mut image = RgbImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            image.put_pixel(
                x,
                y,
                if y >= height - band {
                    Rgb([255, 255, 255])
                } else {
                    Rgb([(x % 200) as u8, 120, 200])
                },
            );
        }
    }
    image
}

fn sha1_of(seed: usize) -> String {
    format!("{:040x}", seed + 1)
}

fn read_index(out: &Path) -> serde_json::Value {
    let bytes = fs::read(out.join("index.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn read_items(out: &Path) -> Vec<serde_json::Value> {
    let index = read_index(out);
    let mut items = Vec::new();
    for shard in index["shards"].as_array().unwrap() {
        let path = out.join(shard["path"].as_str().unwrap());
        let bytes = fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["schema"], 1);
        let shard_items = value["items"].as_array().unwrap();
        assert_eq!(shard_items.len(), shard["count"].as_u64().unwrap() as usize);
        items.extend(shard_items.iter().cloned());
    }
    items
}

fn id_of(out: &Path, name: &str) -> String {
    let item = read_items(out)
        .into_iter()
        .find(|item| item["n"] == name)
        .unwrap_or_else(|| panic!("items 里没有 {name}"));
    item["id"].as_str().unwrap().to_string()
}

fn jpeg_size(path: &Path) -> (u32, u32) {
    let image = image::open(path).unwrap();
    (image.width(), image.height())
}

/// 在已有 JPEG 前插入最小的 EXIF APP1（只含 Orientation），模拟手机照片。
fn inject_exif_orientation(path: &Path, orientation: u8) {
    let jpeg = fs::read(path).unwrap();
    let mut tiff = Vec::new();
    tiff.extend(b"II*\0");
    tiff.extend(8u32.to_le_bytes());
    tiff.extend(1u16.to_le_bytes());
    tiff.extend(0x0112u16.to_le_bytes());
    tiff.extend(3u16.to_le_bytes());
    tiff.extend(1u32.to_le_bytes());
    tiff.extend((orientation as u16).to_le_bytes());
    tiff.extend([0u8, 0u8]);
    tiff.extend(0u32.to_le_bytes());
    let payload = [b"Exif\0\0".as_slice(), tiff.as_slice()].concat();
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend(((payload.len() + 2) as u16).to_be_bytes());
    app1.extend(payload);
    let mut output = Vec::with_capacity(jpeg.len() + app1.len());
    output.extend(&jpeg[..2]);
    output.extend(app1);
    output.extend(&jpeg[2..]);
    fs::write(path, output).unwrap();
}

fn is_valid_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// 导出目录里任何文件都不得出现本地绝对路径（文本或文件名里）。
fn assert_no_absolute_paths(out: &Path, needles: &[String]) {
    let mut files = Vec::new();
    walk(out, &mut files);
    assert!(!files.is_empty(), "导出目录为空：{}", out.display());
    for file in &files {
        let dir_name = file
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let in_media_dir = matches!(dir_name.as_str(), "t256" | "t512" | "t1024" | "t2048" | "o");
        if in_media_dir {
            let stem = file.file_stem().unwrap().to_string_lossy().to_string();
            assert!(is_valid_id(&stem), "产物文件名不是合法 id：{stem}");
            continue;
        }
        // 只有文本产物需要做泄漏扫描；JPEG 是二进制，随机字节里出现 `:/` 不代表路径。
        let text = String::from_utf8_lossy(&fs::read(file).unwrap()).to_string();
        assert!(
            !text.contains(":\\") && !text.contains(":/"),
            "{} 含盘符路径",
            file.display()
        );
        assert!(!text.contains("\\\\"), "{} 含 UNC 路径", file.display());
        for needle in needles {
            assert!(
                !text.contains(needle.as_str()),
                "{} 泄漏本地路径片段 {needle}",
                file.display()
            );
        }
    }
}

fn needles(fixture: &Fixture) -> Vec<String> {
    vec![
        fixture.root.to_string_lossy().to_string(),
        fixture.thumbs.to_string_lossy().to_string(),
        fixture.dir.path.to_string_lossy().to_string(),
    ]
}

#[test]
fn exports_contract_layout_order_and_item_fields() {
    let fixture = Fixture::new("layout");
    let db = fixture.open();
    fixture.add(&db, "a.png", 100, &sha1_of(1), true);
    fixture.add(&db, "b.png", 300, &sha1_of(2), true);
    fixture.add(&db, "c.png", 200, &sha1_of(3), true);

    // 通过唯一公开入口 update_tags 设置 group/rating/自动标签。
    let mut asset = db.load_asset(&fixture.root.join("b.png")).unwrap();
    asset.group = "原创".to_string();
    asset.rating = "S".to_string();
    asset.favorite = true;
    asset.star = 3;
    asset.tags = vec![TagHit {
        tag: "1girl".to_string(),
        category: "general".to_string(),
        score: 0.9961,
        vocab_index: None,
    }];
    db.update_tags(&asset).unwrap();
    // favorite / star 不走 update_tags，各有独立入口。
    let b_path = fixture.root.join("b.png");
    assert_eq!(
        db.set_favorite(std::slice::from_ref(&b_path), true)
            .unwrap(),
        1
    );
    assert_eq!(db.set_star(std::slice::from_ref(&b_path), 3).unwrap(), 1);
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.scanned, 3);
    assert_eq!(stats.exported, 3);
    assert_eq!(stats.shards, 1);
    assert_eq!(stats.thumbs.failed, 0);
    assert_eq!(stats.thumbs.missing, 0);

    let index = read_index(&fixture.out());
    assert_eq!(index["schema"], 1);
    assert_eq!(index["count"], 3);
    assert_eq!(index["shard_size"], 5000);
    assert_eq!(index["shards"].as_array().unwrap().len(), 1);
    assert_eq!(index["shards"][0]["i"], 0);
    assert_eq!(index["shards"][0]["path"], "m/0000.json");
    assert_eq!(index["shards"][0]["count"], 3);
    assert_eq!(index["paths"]["t256"], "t256/");
    assert_eq!(index["paths"]["t512"], "t512/");
    assert_eq!(index["paths"]["t1024"], "t1024/");
    assert_eq!(index["paths"]["t2048"], "t2048/");
    assert_eq!(index["paths"]["original"], "o/");
    assert!(index["generated_at"].as_str().unwrap().ends_with('Z'));

    let items = read_items(&fixture.out());
    assert_eq!(items.len(), 3);
    let mtimes = items
        .iter()
        .map(|item| item["mtime"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mtimes, vec![300, 200, 100], "应按 Modified 降序");

    for item in &items {
        let id = item["id"].as_str().unwrap();
        assert!(is_valid_id(id), "id 不合法：{id}");
        assert!(
            fixture
                .out()
                .join("t512")
                .join(format!("{id}.jpg"))
                .is_file()
        );
        assert!(
            fixture
                .out()
                .join("t256")
                .join(format!("{id}.jpg"))
                .is_file()
        );
        assert!(item["w"].as_u64().unwrap() > 0);
        assert!(item["h"].as_u64().unwrap() > 0);
        assert!(!item["n"].as_str().unwrap().contains('/'));
        assert_eq!(item["ph"], "0000000000000007");
        assert!(item.get("orig").is_none());
        assert_eq!(item["sharp"], 1024, "1200×900 应产出 t1024 并声明");
        if item["n"] != "b.png" {
            // 未设置分级/分组/星级的资产走默认值：G、不写 g 键。
            assert_eq!(item["rt"], "G");
            assert_eq!(item["fav"], serde_json::json!(false));
            assert_eq!(item["st"], 0);
            assert!(item.get("g").is_none());
        }
    }

    let b = items
        .iter()
        .find(|item| item["n"] == "b.png")
        .expect("应包含 b.png");
    assert_eq!(b["rt"], "S");
    assert_eq!(b["g"], "原创");
    assert_eq!(b["fav"], serde_json::json!(true));
    assert_eq!(b["st"], 3);
    let tags = b["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0][0], "1girl");
    assert_eq!(tags[0][1], "general");
    assert!((tags[0][2].as_f64().unwrap() - 0.996).abs() < 1e-9);

    assert_eq!(index["facets"]["groups"].as_array().unwrap().len(), 1);
    assert_eq!(index["facets"]["groups"][0], "原创");
    assert_eq!(index["facets"]["tags"][0]["name"], "1girl");
    assert_eq!(index["facets"]["tags"][0]["count"], 1);

    assert_no_absolute_paths(&fixture.out(), &needles(&fixture));
}

#[test]
fn shard_split_boundaries() {
    let cases: [(usize, Vec<usize>); 4] = [
        (0, vec![]),
        (5, vec![5]),
        (6, vec![5, 1]),
        (11, vec![5, 5, 1]),
    ];
    for (count, expected) in cases {
        let fixture = Fixture::new(&format!("shard-{count}"));
        let db = fixture.open();
        for slot in 0..count {
            fixture.add(
                &db,
                &format!("img-{slot}.png"),
                1_000 + slot as u64,
                &sha1_of(slot),
                true,
            );
        }
        drop(db);

        let mut options = fixture.options();
        options.shard_size = 5;
        let stats = run(&options).unwrap();
        assert_eq!(stats.exported, count, "count={count}");
        assert_eq!(stats.shards, expected.len(), "count={count}");

        let index = read_index(&fixture.out());
        assert_eq!(index["count"], count as u64);
        assert_eq!(index["shard_size"], 5);
        let shards = index["shards"].as_array().unwrap();
        let counts = shards
            .iter()
            .map(|shard| shard["count"].as_u64().unwrap() as usize)
            .collect::<Vec<_>>();
        assert_eq!(counts, expected, "count={count}");
        for (position, shard) in shards.iter().enumerate() {
            assert_eq!(shard["i"], position as u64);
            assert_eq!(shard["path"], format!("m/{position:04}.json").as_str());
        }
        let items = read_items(&fixture.out());
        assert_eq!(items.len(), count);
    }
}

#[test]
fn dry_run_writes_nothing() {
    let fixture = Fixture::new("dry");
    let db = fixture.open();
    fixture.add(&db, "a.png", 100, &sha1_of(1), true);
    fixture.add(&db, "b.png", 200, &sha1_of(2), false);
    drop(db);

    let mut options = fixture.options();
    options.dry_run = true;
    options.with_originals = true;
    let stats = run(&options).unwrap();
    assert_eq!(stats.exported, 2);
    assert_eq!(stats.shards, 1);
    assert_eq!(stats.thumbs.written, 3);
    assert_eq!(stats.thumbs.missing, 1);
    assert_eq!(stats.originals.written, 2);
    assert!(stats.search_bytes > 0, "--dry-run 也应算出搜索索引体积");
    assert!(!options.out_dir.exists(), "--dry-run 不应创建导出目录");
}

#[test]
fn ledger_skips_unchanged_assets_and_rewrites_changed() {
    let fixture = Fixture::new("ledger");
    let db = fixture.open();
    fixture.add(&db, "a.png", 100, &sha1_of(1), true);
    fixture.add(&db, "b.png", 200, &sha1_of(2), true);
    drop(db);

    let mut options = fixture.options();
    options.with_originals = true;
    let first = run(&options).unwrap();
    assert_eq!(first.exported, 2);
    assert_eq!(first.thumbs.written, 4);
    assert_eq!(first.originals.written, 2);
    assert!(options.out_dir.join("publish-ledger.json").is_file());

    let id_a = read_items(&options.out_dir)
        .into_iter()
        .find(|item| item["n"] == "a.png")
        .map(|item| item["id"].as_str().unwrap().to_string())
        .unwrap();
    let t512 = options.out_dir.join("t512").join(format!("{id_a}.jpg"));
    let before = fs::metadata(&t512).unwrap().modified().unwrap();

    // 第二次：源未变 -> 全部跳过，产物不被重写。
    let second = run(&options).unwrap();
    assert_eq!(second.exported, 2);
    assert_eq!(second.thumbs.written, 0);
    assert_eq!(second.thumbs.skipped, 4);
    assert_eq!(second.originals.written, 0);
    assert_eq!(second.originals.skipped, 2);
    assert_eq!(fs::metadata(&t512).unwrap().modified().unwrap(), before);

    // 第三次：只改 a.png 的 mtime -> 仅重抄这一条。
    let db = fixture.open();
    fixture.add(&db, "a.png", 900, &sha1_of(1), true);
    drop(db);
    let third = run(&options).unwrap();
    assert_eq!(third.thumbs.written, 2);
    assert_eq!(third.thumbs.skipped, 2);
    assert_eq!(third.originals.written, 1);
    assert_eq!(third.originals.skipped, 1);
}

#[test]
fn missing_thumb_source_is_skipped_without_error() {
    let fixture = Fixture::new("missing-thumb");
    let db = fixture.open();
    let original = fixture.add(&db, "a.png", 100, &sha1_of(1), true);
    drop(db);
    assert!(original.is_file());
    fs::remove_file(fixture.thumb_path("a.png")).unwrap();

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.scanned, 1);
    assert_eq!(stats.exported, 0);
    assert_eq!(stats.skipped_no_thumb, 1);
    let index = read_index(&fixture.out());
    assert_eq!(index["count"], 0);
    assert!(index["shards"].as_array().unwrap().is_empty());
    assert_eq!(fs::read_dir(fixture.out().join("t512")).unwrap().count(), 0);
}

#[test]
fn originals_are_local_only_and_missing_sources_are_skipped() {
    let fixture = Fixture::new("originals");
    let db = fixture.open();
    fixture.add(&db, "a.png", 100, &sha1_of(1), true);
    fixture.add(&db, "b.jpg", 200, &sha1_of(2), false);
    drop(db);
    fs::remove_file(fixture.root.join("b.jpg")).unwrap();

    let mut options = fixture.options();
    options.with_originals = true;
    let stats = run(&options).unwrap();
    assert_eq!(stats.exported, 2);
    assert_eq!(stats.originals.written, 1);
    assert_eq!(stats.originals.missing, 1);
    assert_eq!(stats.thumbs.missing, 1);

    let items = read_items(&options.out_dir);
    let a = items
        .iter()
        .find(|item| item["n"] == "a.png")
        .expect("应包含 a.png");
    assert_eq!(a["orig"], "png");
    let id = a["id"].as_str().unwrap();
    assert!(
        options
            .out_dir
            .join("o")
            .join(format!("{id}.png"))
            .is_file()
    );
    let b = items
        .iter()
        .find(|item| item["n"] == "b.jpg")
        .expect("应包含 b.jpg");
    assert!(b.get("orig").is_none());
}

#[test]
fn duplicate_ids_are_published_once() {
    let fixture = Fixture::new("dedupe");
    let db = fixture.open();
    let same = sha1_of(7);
    fixture.add(&db, "a.png", 100, &same, true);
    fixture.add(&db, "b.png", 200, &same, true);
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.scanned, 2);
    assert_eq!(stats.exported, 1);
    assert_eq!(stats.duplicates, 1);
    assert_eq!(read_index(&fixture.out())["count"], 1);
}

#[test]
fn limit_caps_exported_items() {
    let fixture = Fixture::new("limit");
    let db = fixture.open();
    for slot in 0..4 {
        fixture.add(
            &db,
            &format!("img-{slot}.png"),
            1_000 + slot as u64,
            &sha1_of(slot),
            true,
        );
    }
    drop(db);

    let mut options = fixture.options();
    options.limit = Some(2);
    let stats = run(&options).unwrap();
    assert_eq!(stats.exported, 2);
    let index = read_index(&options.out_dir);
    assert_eq!(index["count"], 2);
    assert_eq!(index["shards"].as_array().unwrap().len(), 1);
    assert_eq!(index["shards"][0]["count"], 2);
    let items = read_items(&options.out_dir);
    let mtimes = items
        .iter()
        .map(|item| item["mtime"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mtimes, vec![1_003, 1_002], "limit 应取排序最前的两条");
}

/// 同内容的两份拷贝、只有较旧那份打过标签：导出的条目仍要带上标签。
///
/// 真实数据里踩到过：样例图在 stress 库里有一份未打标签的拷贝且 mtime 更新，
/// Modified 降序把它排在了前面并被保留，标签全丢——看起来像"asset_tags 没 join"。
#[test]
fn duplicate_copy_supplies_tags_when_kept_copy_is_untagged() {
    let fixture = Fixture::new("dup-tags");
    let db = fixture.open();
    let sha = sha1_of(9);
    let tagged = fixture.add(&db, "keep/tagged.png", 100, &sha, true);
    fixture.add(&db, "keep/untagged-newer.png", 900, &sha, true);

    let mut asset = db.load_asset(&tagged).unwrap();
    asset.tags = vec![TagHit {
        tag: "1girl".to_string(),
        category: "general".to_string(),
        score: 0.9,
        vocab_index: None,
    }];
    db.update_tags(&asset).unwrap();
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.exported, 1, "重复 id 只导出一条");
    assert_eq!(stats.duplicates, 1);
    assert_eq!(stats.tags_from_duplicates, 1, "标签应从同内容副本补回");

    let items = read_items(&fixture.out());
    assert_eq!(items.len(), 1);
    let tags = items[0]["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0][0], "1girl");
}

/// 锐利档从原图按长边缩小，小图绝不放大；档位文件是真实可解码的 JPEG。
#[test]
fn sharp_tiers_downscale_and_never_upscale() {
    let fixture = Fixture::new("tiers-scale");
    let db = fixture.open();
    fixture.add_sized(&db, "big.png", 3000, 2000, 100, &sha1_of(1), true);
    fixture.add_sized(&db, "small.png", 800, 600, 200, &sha1_of(2), true);
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.exported, 2);
    assert_eq!(stats.tiers.written_t1024, 1);
    assert_eq!(stats.tiers.written_t2048, 1);
    assert_eq!(stats.tiers.not_needed, 2, "800×600 两个档都不该放大");
    assert_eq!(stats.tiers.failed, 0);

    let out = fixture.out();
    let big = id_of(&out, "big.png");
    let small = id_of(&out, "small.png");
    for (tier, edge) in [("t1024", 1024), ("t2048", 2048)] {
        let path = out.join(tier).join(format!("{big}.jpg"));
        let (w, h) = jpeg_size(&path);
        assert_eq!(w.max(h), edge, "{tier} 长边应为 {edge}");
        assert!(
            (w as f64 / h as f64 - 1.5).abs() < 0.01,
            "{tier} 宽高比走样"
        );
    }
    assert!(!out.join("t1024").join(format!("{small}.jpg")).exists());
    assert!(!out.join("t2048").join(format!("{small}.jpg")).exists());

    let items = read_items(&out);
    let sharp_of = |name: &str| {
        items.iter().find(|item| item["n"] == name).unwrap()["sharp"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(sharp_of("big.png"), 2048);
    assert_eq!(sharp_of("small.png"), 0, "小图必须显式声明无锐利档");
    assert_no_absolute_paths(&out, &needles(&fixture));
}

/// 账本命中时锐利档不重写；源 mtime 变化后只重编这一条。
#[test]
fn sharp_tiers_are_incremental() {
    let fixture = Fixture::new("tiers-ledger");
    let db = fixture.open();
    fixture.add_sized(&db, "big.png", 3000, 2000, 100, &sha1_of(1), true);
    drop(db);

    let first = run(&fixture.options()).unwrap();
    assert_eq!(first.tiers.written(), 2);
    let out = fixture.out();
    let id = id_of(&out, "big.png");
    let t1024 = out.join("t1024").join(format!("{id}.jpg"));
    let before = fs::metadata(&t1024).unwrap().modified().unwrap();

    let second = run(&fixture.options()).unwrap();
    assert_eq!(second.tiers.written(), 0);
    assert_eq!(second.tiers.skipped, 2);
    assert_eq!(fs::metadata(&t1024).unwrap().modified().unwrap(), before);

    let db = fixture.open();
    fixture.add_sized(&db, "big.png", 3000, 2000, 900, &sha1_of(1), true);
    drop(db);
    let third = run(&fixture.options()).unwrap();
    assert_eq!(third.tiers.written(), 2);
    assert_eq!(third.tiers.skipped, 0);
}

/// EXIF 方向必须在生成锐利档时套用，否则手机竖拍的照片在详情页会躺倒。
#[test]
fn sharp_tiers_follow_exif_orientation() {
    let fixture = Fixture::new("tiers-orientation");
    let db = fixture.open();
    let original = fixture.add_sized(&db, "rotated.jpg", 3000, 2000, 100, &sha1_of(1), true);
    drop(db);
    inject_exif_orientation(&original, 6);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.tiers.written(), 2);

    let out = fixture.out();
    let id = id_of(&out, "rotated.jpg");
    let (w, h) = jpeg_size(&out.join("t1024").join(format!("{id}.jpg")));
    assert_eq!((w, h), (683, 1024), "orientation=6 应顺时针转成 683×1024");
    assert_eq!(read_items(&out)[0]["sharp"], 2048);
}

/// 原图损坏时仍导出该资产（t512 可用），只是缺锐利档。
#[test]
fn undecodable_source_keeps_asset_and_skips_tiers() {
    let fixture = Fixture::new("tiers-broken");
    let db = fixture.open();
    let original = fixture.add_sized(&db, "broken.png", 3000, 2000, 100, &sha1_of(1), true);
    drop(db);
    fs::write(&original, b"not a real png").unwrap();

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.exported, 1);
    assert_eq!(stats.tiers.failed, 1);
    assert_eq!(stats.tiers.written(), 0);
    let out = fixture.out();
    let id = id_of(&out, "broken.png");
    assert!(out.join("t512").join(format!("{id}.jpg")).is_file());
    assert_eq!(read_items(&out)[0]["sharp"], 0, "解码失败必须声明无锐利档");
}

/// --no-tiers：只写清单与 t512/t256，不创建锐利档目录。
#[test]
fn tiers_can_be_disabled() {
    let fixture = Fixture::new("tiers-off");
    let db = fixture.open();
    fixture.add_sized(&db, "big.png", 3000, 2000, 100, &sha1_of(1), true);
    drop(db);

    let mut options = fixture.options();
    options.tiers = false;
    let stats = run(&options).unwrap();
    assert_eq!(stats.tiers.written(), 0);
    assert!(!options.out_dir.join("t1024").exists());
    assert!(!options.out_dir.join("t2048").exists());
    assert_eq!(read_index(&options.out_dir)["paths"]["t2048"], "t2048/");
    assert_eq!(read_items(&options.out_dir)[0]["sharp"], 0);
}

fn rgb_at(path: &Path) -> RgbImage {
    image::open(path).unwrap().to_rgb8()
}

fn assert_near_white(pixel: &Rgb<u8>, context: &str) {
    assert!(
        pixel.0.iter().all(|channel| *channel >= 245),
        "{context} 应为白底，实际 {pixel:?}"
    );
}

/// 透明 PNG：底部全透明在缩略图里已经合成白底，源图检测也必须走同一表面，
/// 否则 `to_rgb8()` 看到的是透明像素下的 RGB（通常 0），白带永远检测不到；
/// 裁剪后的 JPEG 档也会把透明区域压成黑色。
#[test]
fn trims_transparent_bottom_band_from_png() {
    let fixture = Fixture::new("band-alpha-png");
    let db = fixture.open();
    let (width, height, band) = (1200u32, 900u32, 135u32);
    let mut source = RgbaImage::from_pixel(width, height, Rgba([20, 80, 200, 255]));
    for y in height - band..height {
        for x in 0..width {
            // 透明像素的 RGB 故意留 0：旧路径会把它当成黑色，检测不到白带。
            source.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        }
    }
    // 保留区域里再挖一个透明方块：验证编码表面同样合成白底（否则导出会是黑块）。
    for y in 120..220 {
        for x in 120..220 {
            source.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        }
    }
    DynamicImage::ImageRgba8(source)
        .save(fixture.root.join("alpha.png"))
        .unwrap();
    fixture.register_image(&db, "alpha.png", 100, &sha1_of(1));
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.exported, 1);
    assert_eq!(stats.trimmed, 1, "透明底带必须被裁掉");

    let out = fixture.out();
    let item = read_items(&out)
        .into_iter()
        .find(|item| item["n"] == "alpha.png")
        .unwrap();
    assert_eq!(item["h"], 765, "900 - 135");
    assert_eq!(item["sharp"], 1024, "1200 长边只产出 t1024");
    let id = item["id"].as_str().unwrap();

    let t512 = rgb_at(&out.join("t512").join(format!("{id}.jpg")));
    assert_eq!(t512.width().max(t512.height()), 512);
    // 透明方块在缩放后中心仍是纯透明，编码表面必须合成白底。
    let hole_at = |image: &RgbImage| {
        let x = (170.0 * f64::from(image.width()) / f64::from(width)) as u32;
        let y = (170.0 * f64::from(image.height()) / f64::from(height - band)) as u32;
        *image.get_pixel(x, y)
    };
    assert_near_white(&hole_at(&t512), "t512 透明方块中心");
    let t1024 = rgb_at(&out.join("t1024").join(format!("{id}.jpg")));
    assert_eq!(t1024.width().max(t1024.height()), 1024);
    assert_near_white(&hole_at(&t1024), "t1024 透明方块中心");
}

/// 22 帧动画 GIF：底带只在首帧里是透明的。检测、裁剪、各档编码都必须用首帧，
/// 不能串到后续帧（后续帧底带是亮红，内容颜色也不同）。
#[test]
fn trims_first_frame_white_band_from_animated_gif() {
    use image::AnimationDecoder;

    let fixture = Fixture::new("band-gif");
    let db = fixture.open();
    let (width, height, band) = (400u32, 300u32, 45u32);
    let frames = (0..22)
        .map(|index| {
            let mut frame = RgbaImage::from_pixel(width, height, Rgba([30, 90, 200, 255]));
            for y in height - band..height {
                for x in 0..width {
                    frame.put_pixel(
                        x,
                        y,
                        if index == 0 {
                            Rgba([0, 0, 0, 0])
                        } else {
                            Rgba([220, 30, 30, 255])
                        },
                    );
                }
            }
            // 保留区域里的透明方块：首帧透明、后续帧亮红。输出若串帧就会看到红色。
            for y in 40..80 {
                for x in 40..80 {
                    frame.put_pixel(
                        x,
                        y,
                        if index == 0 {
                            Rgba([0, 0, 0, 0])
                        } else {
                            Rgba([220, 30, 30, 255])
                        },
                    );
                }
            }
            frame
        })
        .collect::<Vec<_>>();
    let original = fixture.root.join("anim.gif");
    write_animated_gif(&original, &frames);

    let reader = std::io::BufReader::new(fs::File::open(&original).unwrap());
    let decoder = image::codecs::gif::GifDecoder::new(reader).unwrap();
    let decoded_frames = decoder.into_frames().collect_frames().unwrap();
    assert_eq!(decoded_frames.len(), 22, "夹具必须是 22 帧动画");

    fixture.register_image(&db, "anim.gif", 100, &sha1_of(1));
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.exported, 1);
    assert_eq!(stats.trimmed, 1, "首帧透明底带必须被裁掉");

    let out = fixture.out();
    let item = read_items(&out)
        .into_iter()
        .find(|item| item["n"] == "anim.gif")
        .unwrap();
    assert_eq!(item["h"], 255, "300 - 45，且按首帧高度裁");
    let id = item["id"].as_str().unwrap();

    let t512 = rgb_at(&out.join("t512").join(format!("{id}.jpg")));
    assert_eq!((t512.width(), t512.height()), (400, 255));
    // 透明方块只存在于首帧；输出必须是白底，串到后续帧会变成红色。
    assert_near_white(t512.get_pixel(60, 60), "t512 首帧透明方块");
    let bottom = t512.get_pixel(200, 254);
    assert!(
        bottom[2] > 150 && bottom[0] < 100,
        "裁掉底带后最后一行应是首帧内容：{bottom:?}"
    );
    let top = t512.get_pixel(200, 10);
    assert!(
        top[2] > 150 && top[0] < 100,
        "内容应来自首帧蓝色，而不是后续帧红色：{top:?}"
    );
}

/// 3% 薄带 JPEG：手机上常见的细白边。缩略图候选能命中，源图确认也必须认，
/// 否则 4% 老阈值会把它整条漏掉。
#[test]
fn trims_thin_three_percent_white_band_from_jpeg() {
    let fixture = Fixture::new("band-thin-jpeg");
    let db = fixture.open();
    let (width, height, band) = (1200u32, 900u32, 27u32);
    let source = image_with_white_bottom(width, height, band);
    write_jpeg_source(&fixture.root.join("thin.jpg"), &source, 90);
    fixture.register_image(&db, "thin.jpg", 100, &sha1_of(1));
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.exported, 1);
    assert_eq!(stats.trimmed, 1, "3% 白带必须被裁掉");

    let out = fixture.out();
    let item = read_items(&out)
        .into_iter()
        .find(|item| item["n"] == "thin.jpg")
        .unwrap();
    let height = item["h"].as_u64().unwrap();
    assert!(
        (865..=873).contains(&height),
        "应裁掉约 27 行（JPEG ringing 可能多吃 1-2 行），实际高度 {height}"
    );
    assert_eq!(item["sharp"], 1024);
    let id = item["id"].as_str().unwrap();
    let t512 = rgb_at(&out.join("t512").join(format!("{id}.jpg")));
    assert_eq!(t512.width().max(t512.height()), 512);
    let bottom = t512.get_pixel(t512.width() / 2, t512.height() - 1);
    assert!(
        !bottom.0.iter().all(|channel| *channel >= 245),
        "裁掉白带后最后一行不应仍是白色：{bottom:?}"
    );
}

/// 2% 白带不裁：阈值下调到 3% 后仍要挡住照片自带的窄白边，避免误裁。
#[test]
fn does_not_trim_white_band_below_three_percent() {
    let fixture = Fixture::new("band-below-threshold");
    let db = fixture.open();
    let source = image_with_white_bottom(1200, 900, 18); // 2%
    write_jpeg_source(&fixture.root.join("narrow.jpg"), &source, 90);
    fixture.register_image(&db, "narrow.jpg", 100, &sha1_of(1));
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    assert_eq!(stats.exported, 1);
    assert_eq!(stats.trimmed, 0, "2% 白带不应被裁");
    let items = read_items(&fixture.out());
    assert_eq!(items[0]["h"], 900);
}

fn rewrite_ledger(path: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let mut ledger: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutate(&mut ledger);
    fs::write(path, serde_json::to_vec(&ledger).unwrap()).unwrap();
}

/// 账本 render 版本是产物缓存的一部分：旧版本条目必须重建 thumb/tier/trim，
/// 重建后的二跑再恢复全跳过（幂等）。
#[test]
fn render_version_invalidates_cached_outputs_and_second_run_is_stable() {
    let fixture = Fixture::new("render-version");
    let db = fixture.open();
    fixture.add_sized(&db, "big.png", 3000, 2000, 100, &sha1_of(1), true);
    drop(db);

    let first = run(&fixture.options()).unwrap();
    assert_eq!(first.thumbs.written, 2);
    assert_eq!(first.tiers.written(), 2);

    let second = run(&fixture.options()).unwrap();
    assert_eq!(second.thumbs.written, 0);
    assert_eq!(second.thumbs.skipped, 2);
    assert_eq!(second.tiers.written(), 0);
    assert_eq!(second.tiers.skipped, 2);

    let ledger_path = fixture.out().join("publish-ledger.json");
    let ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(ledger["version"], 2, "账本结构版本");
    for entry in ledger["assets"].as_object().unwrap().values() {
        assert_eq!(entry["render"], 2, "新条目应记录当前渲染版本");
    }

    // 模拟旧渲染器写出的账本：源指纹不变，但 render 过期。
    rewrite_ledger(&ledger_path, |ledger| {
        for entry in ledger["assets"].as_object_mut().unwrap().values_mut() {
            entry["render"] = serde_json::json!(1);
        }
    });
    let rebuilt = run(&fixture.options()).unwrap();
    assert_eq!(rebuilt.thumbs.written, 2, "render 过期必须重抄 t512/t256");
    assert_eq!(rebuilt.thumbs.skipped, 0);
    assert_eq!(rebuilt.tiers.written(), 2, "render 过期必须重编锐利档");
    assert_eq!(rebuilt.tiers.skipped, 0);

    let stable = run(&fixture.options()).unwrap();
    assert_eq!(stable.thumbs.written, 0);
    assert_eq!(stable.tiers.written(), 0, "重建后的二跑应完全命中账本");
}

/// 账本文件结构版本不匹配：整份账本作废，本次全量重建。
#[test]
fn ledger_schema_version_mismatch_forces_full_rebuild() {
    let fixture = Fixture::new("ledger-version");
    let db = fixture.open();
    fixture.add_sized(&db, "big.png", 3000, 2000, 100, &sha1_of(1), true);
    drop(db);

    let first = run(&fixture.options()).unwrap();
    assert_eq!(first.thumbs.written, 2);

    rewrite_ledger(&fixture.out().join("publish-ledger.json"), |ledger| {
        ledger["version"] = serde_json::json!(1);
    });
    let rebuilt = run(&fixture.options()).unwrap();
    assert_eq!(rebuilt.thumbs.written, 2);
    assert_eq!(rebuilt.thumbs.skipped, 0);
    assert_eq!(rebuilt.tiers.written(), 2);
}

// ---- search-index.json（契约 §2.1/§7）----

fn read_search(out: &Path) -> serde_json::Value {
    let bytes = fs::read(out.join(kanalib_publish::SEARCH_INDEX_FILE)).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn text_array(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn search_index_aligns_with_shards_and_is_deterministic() {
    let fixture = Fixture::new("search-align");
    let db = fixture.open();
    fixture.add(&db, "a.png", 300, &sha1_of(1), true);
    fixture.add(&db, "b.png", 200, &sha1_of(2), true);
    fixture.add(&db, "c.png", 100, &sha1_of(3), true);

    // b：分组 + S 级 + 两个标签（含重复名，验证去重）；c：只换分级，不带标签。
    let mut b = db.load_asset(&fixture.root.join("b.png")).unwrap();
    b.group = "原创".to_string();
    b.rating = "S".to_string();
    b.tags = vec![
        TagHit {
            tag: "solo".to_string(),
            category: "general".to_string(),
            score: 0.9,
            vocab_index: None,
        },
        TagHit {
            tag: "1girl".to_string(),
            category: "general".to_string(),
            score: 0.8,
            vocab_index: None,
        },
    ];
    db.update_tags(&b).unwrap();
    let mut c = db.load_asset(&fixture.root.join("c.png")).unwrap();
    c.rating = "E".to_string();
    db.update_tags(&c).unwrap();
    drop(db);

    let stats = run(&fixture.options()).unwrap();
    let search = read_search(&fixture.out());
    assert_eq!(search["v"], 1);
    assert_eq!(
        stats.search_bytes,
        fs::metadata(fixture.out().join(kanalib_publish::SEARCH_INDEX_FILE))
            .unwrap()
            .len() as usize
    );

    let tags = text_array(&search["tags"]);
    let mut sorted = tags.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(tags, sorted, "tags 必须升序且去重");
    assert_eq!(tags, vec!["1girl", "solo"]);

    // 未分组用 groupIdx=-1，空串不进 groups 表。
    let groups = text_array(&search["groups"]);
    assert_eq!(groups, vec!["原创"]);

    // 行按分片顺序对齐：mtime 降序 a、b、c。
    let rows = search["items"].as_array().unwrap();
    let items = read_items(&fixture.out());
    assert_eq!(rows.len(), items.len(), "items 行数必须等于分片条目数");
    for (position, item) in items.iter().enumerate() {
        let row = rows[position]
            .as_array()
            .unwrap_or_else(|| panic!("item[{position}] 必须是数组"));
        assert_eq!(row.len(), 6, "行必须是 [id, n, [tagIdx], groupIdx, st, rt]");
        assert_eq!(row[0], item["id"], "item[{position}].id 与分片不一致");
        assert_eq!(row[1], item["n"]);

        let indices = row[2]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry.as_u64().unwrap() as usize)
            .collect::<Vec<_>>();
        assert!(
            indices.windows(2).all(|pair| pair[0] < pair[1]),
            "item[{position}] 的 tagIdx 必须严格升序且去重"
        );
        for index in &indices {
            assert!(
                *index < tags.len(),
                "item[{position}] 的 tagIdx {index} 越界（tags 只有 {} 项）",
                tags.len()
            );
        }
        let mut decoded = indices
            .iter()
            .map(|index| tags[*index].clone())
            .collect::<Vec<_>>();
        decoded.sort();
        let mut expected = item["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| tag[0].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(decoded, expected, "item[{position}] 标签集合与分片不一致");

        let group_index = row[3].as_i64().unwrap();
        let group = item["g"].as_str().unwrap_or("");
        if group.is_empty() {
            assert_eq!(group_index, -1, "未分组必须是 -1");
        } else {
            assert_eq!(
                groups[group_index as usize], group,
                "item[{position}] 的 groupIdx 指向错误"
            );
        }
        assert_eq!(row[4], item["st"], "星级必须与分片一致");
        assert_eq!(
            row[5].as_str().unwrap(),
            item["rt"].as_str().unwrap().to_ascii_lowercase(),
            "rt 必须是小写 token"
        );
    }

    // 确定性：文件无时间戳，两次发布与 --index-only 重建必须逐字节一致。
    let first = fs::read(fixture.out().join(kanalib_publish::SEARCH_INDEX_FILE)).unwrap();
    let second_stats = run(&fixture.options()).unwrap();
    assert_eq!(second_stats.thumbs.written, 0);
    assert_eq!(
        fs::read(fixture.out().join(kanalib_publish::SEARCH_INDEX_FILE)).unwrap(),
        first,
        "二跑搜索索引应逐字节稳定"
    );

    fs::remove_file(fixture.out().join(kanalib_publish::SEARCH_INDEX_FILE)).unwrap();
    let rebuilt = kanalib_publish::rebuild_search_index(&fixture.out(), false).unwrap();
    assert_eq!(rebuilt.items, 3);
    assert_eq!(rebuilt.tags, 2);
    assert!(rebuilt.written);
    assert_eq!(
        fs::read(fixture.out().join(kanalib_publish::SEARCH_INDEX_FILE)).unwrap(),
        first,
        "--index-only 结果应与发布一致"
    );
}

#[test]
fn index_only_rebuild_touches_no_images_or_ledger() {
    let fixture = Fixture::new("search-only");
    let db = fixture.open();
    fixture.add(&db, "a.png", 200, &sha1_of(1), true);
    fixture.add(&db, "b.png", 100, &sha1_of(2), true);
    drop(db);
    run(&fixture.options()).unwrap();

    // 快照除 search-index.json 外的全部产物：内容 + 修改时间都必须原样保留。
    let snapshot = |out: &Path| {
        let mut files = Vec::new();
        walk(out, &mut files);
        files.sort();
        files
            .into_iter()
            .filter(|path| path.file_name().unwrap() != kanalib_publish::SEARCH_INDEX_FILE)
            .map(|path| {
                let meta = fs::metadata(&path).unwrap();
                let bytes = fs::read(&path).unwrap();
                (path, bytes, meta.modified().unwrap(), meta.len())
            })
            .collect::<Vec<_>>()
    };
    let before = snapshot(&fixture.out());
    assert!(
        before
            .iter()
            .any(|(path, ..)| path.ends_with("publish-ledger.json"))
    );
    assert!(
        before
            .iter()
            .any(|(path, ..)| path.to_string_lossy().contains("t512"))
    );

    // 模拟旧发布器产物：删掉搜索索引，随后只重建它。
    fs::remove_file(fixture.out().join(kanalib_publish::SEARCH_INDEX_FILE)).unwrap();
    let stats = kanalib_publish::rebuild_search_index(&fixture.out(), true).unwrap();
    assert_eq!(stats.items, 2);
    assert!(!stats.written, "--dry-run 不应写出文件");
    assert!(
        !fixture
            .out()
            .join(kanalib_publish::SEARCH_INDEX_FILE)
            .exists()
    );

    let stats = kanalib_publish::rebuild_search_index(&fixture.out(), false).unwrap();
    assert_eq!(stats.items, 2);
    assert!(
        fixture
            .out()
            .join(kanalib_publish::SEARCH_INDEX_FILE)
            .is_file()
    );

    let after = snapshot(&fixture.out());
    assert_eq!(after.len(), before.len(), "只重建索引不应增删其它产物");
    for ((path_a, bytes_a, mtime_a, len_a), (path_b, bytes_b, mtime_b, len_b)) in
        before.iter().zip(after.iter())
    {
        assert_eq!(path_a, path_b);
        assert_eq!(len_a, len_b, "{} 大小变了", path_a.display());
        assert_eq!(bytes_a, bytes_b, "{} 内容变了", path_a.display());
        assert_eq!(mtime_a, mtime_b, "{} 被重写了", path_a.display());
    }
}

#[test]
fn index_only_cli_needs_no_database() {
    let fixture = Fixture::new("search-cli");
    let db = fixture.open();
    fixture.add(&db, "a.png", 100, &sha1_of(1), true);
    drop(db);
    run(&fixture.options()).unwrap();
    fs::remove_file(fixture.out().join(kanalib_publish::SEARCH_INDEX_FILE)).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kanalib-publish"))
        .arg("--out")
        .arg(fixture.out())
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "CLI 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("搜索索引"), "stdout 缺索引统计：{stdout}");
    assert!(
        fixture
            .out()
            .join(kanalib_publish::SEARCH_INDEX_FILE)
            .is_file()
    );
}
