//! 本地根目录来源：只读视图包装，扫描/监听/清理仍由 `state` 负责。

use std::path::{Path, PathBuf};

use crate::sources::{
    Credentials, Source, SourceError, SourceInfo, SourceKind, SourceStatus, SourceSyncContext,
    SourceSyncReport,
};

#[derive(Clone, Debug)]
pub struct LocalSource {
    info: SourceInfo,
    asset_count: usize,
}

impl LocalSource {
    pub fn new(root: impl Into<PathBuf>, asset_count: usize) -> Self {
        let root = root.into();
        Self {
            info: Self::info_for(&root),
            asset_count,
        }
    }

    /// 本地来源的持久身份：roots 存在 settings，不要求已有 sources 行。
    pub fn info_for(root: &Path) -> SourceInfo {
        let locator = root.to_string_lossy().to_string();
        let name = root
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| locator.clone());
        SourceInfo {
            id: None,
            kind: SourceKind::Local,
            name,
            locator,
            read_only: false,
            last_sync_at: None,
            sync_cursor: None,
        }
    }

    pub fn root(&self) -> PathBuf {
        PathBuf::from(&self.info.locator)
    }
}

impl Source for LocalSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn status(&self) -> SourceStatus {
        let online = Path::new(&self.info.locator).is_dir();
        let mut status = SourceStatus::ok(&self.info, self.asset_count);
        status.online = online;
        if !online {
            status.last_error = Some(SourceError::new(
                crate::sources::SourceErrorKind::Disconnected,
                format!("来源不可访问：{}", self.info.locator),
            ));
        }
        status
    }

    fn test_connection(&self, _credentials: &Credentials) -> Result<SourceStatus, SourceError> {
        Ok(self.status())
    }

    /// 本地来源没有远端增量同步；由 `AppState::start_scan` 走原有扫描语义。
    fn sync(
        &self,
        _context: &SourceSyncContext<'_>,
        _credentials: &Credentials,
    ) -> Result<SourceSyncReport, SourceError> {
        Err(SourceError::unsupported(
            "本地来源由扫描线程处理，不走远端同步",
        ))
    }
}
