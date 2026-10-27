//! F-061 数据库备份核心：由任务 A 实现（WAL checkpoint、rusqlite backup、
//! 备份库完整性/计数校验）。接口见 [`crate::model::BackupReport`]。
