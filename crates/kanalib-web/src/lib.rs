//! 网页轻量后端 W1：只读 API、媒体服务与网页目录 schema。
//!
//! 本 crate 只读网页自己的 SQLite，不连桌面库、不持有来源令牌；
//! 公开 listener 不注册任何写路由。

pub mod crypto;
pub mod db;
pub mod media;
pub mod query;
pub mod response;
pub mod routes;
pub mod state;

pub use state::AppState;
