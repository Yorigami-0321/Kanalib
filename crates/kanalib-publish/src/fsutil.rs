//! 目录/文件写入小工具。所有中间产物都落在目标同目录的隐藏临时文件里，
//! 成功 rename、失败清理，保证导出目录终态没有临时文件。

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// 目录不存在时创建（含父级）。
pub fn ensure_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(path).with_context(|| format!("创建目录 {}", path.display()))
}

/// 原子写：先写同目录 `.名字.pid-nonce.tmp`，再 rename 覆盖目标。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_dir(parent)?;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "out".to_string());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let temp = parent.join(format!(".{name}.{}-{nonce}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = File::create(&temp).with_context(|| format!("创建 {}", temp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("写入 {}", temp.display()))?;
        file.flush()?;
        drop(file);
        fs::rename(&temp, path)
            .with_context(|| format!("重命名 {} -> {}", temp.display(), path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
