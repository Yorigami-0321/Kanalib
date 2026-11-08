//! 增量发布账本：导出目录下的 publish-ledger.json。
//!
//! 记录每个资产 id 上次发布时的源指纹（sha1/mtime/bytes）与已写出的相对路径，
//! 下次发布时源指纹一致且目标文件仍在就跳过复制（契约 §6）。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::fsutil;

pub const LEDGER_FILE: &str = "publish-ledger.json";
pub const LEDGER_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// 内容指纹（sha1 优先，缺失时回落 blake3）；与上次一致才跳过。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    pub mtime: u64,
    pub bytes: u64,
    /// 该资产上次写出的相对路径（t512/t256/o）。
    #[serde(default)]
    pub files: Vec<String>,
}

impl LedgerEntry {
    pub fn is_current(&self, sha1: Option<&str>, mtime: u64, bytes: u64) -> bool {
        self.mtime == mtime && self.bytes == bytes && self.sha1.as_deref() == sha1
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    pub version: u32,
    #[serde(default)]
    pub assets: BTreeMap<String, LedgerEntry>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: LEDGER_VERSION,
            assets: BTreeMap::new(),
        }
    }
}

impl Ledger {
    /// 读取账本；文件不存在或损坏时回退为空账本（损坏只警告，不影响发布）。
    pub fn load(out_dir: &Path) -> Self {
        let path = out_dir.join(LEDGER_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "[kanalib-publish] 警告：读取 {} 失败（{error}），本次按全量发布",
                        path.display()
                    );
                }
                return Self::default();
            }
        };
        match serde_json::from_str::<Ledger>(&text) {
            Ok(ledger) => {
                if ledger.version != LEDGER_VERSION {
                    eprintln!(
                        "[kanalib-publish] 警告：账本版本 {} 与当前 {} 不一致，仍按字段复用",
                        ledger.version, LEDGER_VERSION
                    );
                }
                ledger
            }
            Err(error) => {
                eprintln!(
                    "[kanalib-publish] 警告：账本解析失败（{error}），本次按全量发布"
                );
                Self::default()
            }
        }
    }

    pub fn save(&self, out_dir: &Path) -> Result<()> {
        let path = out_dir.join(LEDGER_FILE);
        let bytes = serde_json::to_vec(self).context("序列化账本失败")?;
        fsutil::write_atomic(&path, &bytes)
    }

    pub fn get(&self, id: &str) -> Option<&LedgerEntry> {
        self.assets.get(id)
    }

    pub fn insert(&mut self, id: &str, entry: LedgerEntry) {
        self.assets.insert(id.to_string(), entry);
    }

    pub fn remove(&mut self, id: &str) {
        self.assets.remove(id);
    }

    pub fn len(&self) -> usize {
        self.assets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
                "kanalib-ledger-{tag}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn roundtrip_and_missing_file() {
        let dir = TempDir::new("roundtrip");
        assert!(Ledger::load(&dir.path).is_empty());

        let mut ledger = Ledger::default();
        ledger.insert(
            "abc",
            LedgerEntry {
                sha1: Some("deadbeef".to_string()),
                mtime: 7,
                bytes: 99,
                files: vec!["t512/abc.jpg".to_string()],
            },
        );
        ledger.save(&dir.path).unwrap();

        let reloaded = Ledger::load(&dir.path);
        assert_eq!(reloaded, ledger);
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.get("abc").unwrap().files, vec!["t512/abc.jpg"]);
    }

    #[test]
    fn corrupt_file_falls_back_to_empty() {
        let dir = TempDir::new("corrupt");
        fs::write(dir.path.join(LEDGER_FILE), b"{not json").unwrap();
        assert!(Ledger::load(&dir.path).is_empty());
    }

    #[test]
    fn is_current_compares_fingerprint() {
        let entry = LedgerEntry {
            sha1: Some("aa".to_string()),
            mtime: 5,
            bytes: 6,
            files: Vec::new(),
        };
        assert!(entry.is_current(Some("aa"), 5, 6));
        assert!(!entry.is_current(Some("bb"), 5, 6));
        assert!(!entry.is_current(Some("aa"), 6, 6));
        assert!(!entry.is_current(None, 5, 6));
    }
}
