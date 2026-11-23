//! 来源同步：元数据落库、去重/快速追平、错误分类与下载入队。

use anyhow::Result;
use sha1::{Digest, Sha1};
use tracing::{info, warn};

use crate::db::{Db, SourceAuthorRow, SourceWorkRow, now};
use crate::source::{SourceAdapter, SourceError, SourceErrorKind, SourceWork, author_row};

#[derive(Debug, Clone)]
pub struct IngestOutcome {
    pub fetched_pages: usize,
    pub new_works: usize,
    pub stopped_early: bool,
}

pub async fn ingest_bookmarks(
    db: &mut Db,
    adapter: &dyn SourceAdapter,
    user_id: &str,
) -> Result<IngestOutcome> {
    db.sync_source(adapter.id(), "remote")?;
    let mut cursor = None;
    let mut outcome = IngestOutcome {
        fetched_pages: 0,
        new_works: 0,
        stopped_early: false,
    };

    loop {
        let page = match adapter
            .list_public_bookmarks(user_id, cursor.as_deref())
            .await
        {
            Ok(page) => page,
            Err(error) => {
                handle_source_error(db, adapter, None, &error).await?;
                return Err(error.into());
            }
        };
        outcome.fetched_pages += 1;

        let all_known = page
            .items
            .iter()
            .all(|work| all_pages_known(db, adapter.id(), work));
        for work in &page.items {
            match ingest_work(db, adapter, work) {
                Ok(is_new) => outcome.new_works += usize::from(is_new),
                Err(error) => {
                    warn!(error = %error, work = %work.external_id, "ingest work failed");
                    return Err(error);
                }
            }
        }

        for work in &page.items {
            for page_index in 0..work.page_count.max(1) {
                if let Ok(image) = adapter.resolve_image(work, page_index).await {
                    db.persist_bookmark_image(
                        &item_id(adapter.id(), &work.external_id, page_index),
                        &image.url,
                        image.ext.as_deref(),
                    )?;
                }
            }
        }

        match page.next_cursor {
            Some(next) if !all_known => cursor = Some(next),
            Some(_) => {
                outcome.stopped_early = true;
                break;
            }
            None => break,
        }
    }
    db.record_source_sync(adapter.id(), now())?;
    Ok(outcome)
}

pub async fn ingest_single_work(
    db: &mut Db,
    adapter: &dyn SourceAdapter,
    external_id: &str,
) -> Result<bool> {
    db.sync_source(adapter.id(), "remote")?;
    let work = match adapter.fetch_work(external_id).await {
        Ok(work) => work,
        Err(error) => {
            handle_source_error(db, adapter, None, &error).await?;
            return Err(error.into());
        }
    };
    ingest_work(db, adapter, &work)
}

pub fn ingest_work(db: &mut Db, adapter: &dyn SourceAdapter, work: &SourceWork) -> Result<bool> {
    let source = adapter.id();
    let author = work
        .author
        .as_ref()
        .map(|author| author_row(source, author))
        .unwrap_or_else(|| SourceAuthorRow {
            source: source.to_string(),
            external_id: "unknown".to_string(),
            name: String::new(),
            avatar_url: None,
        });
    let row = SourceWorkRow {
        source: source.to_string(),
        external_id: work.external_id.clone(),
        author: Some(author),
        title: work.title.clone(),
        page_count: work.page_count.max(1),
        posted_at: work.posted_at,
        rating: work.rating,
        ai: work.ai,
        x_restrict: work.x_restrict,
        sanity_level: work.sanity_level,
        restriction_attributes: serde_json::to_string(&work.restriction_attributes)
            .unwrap_or_else(|_| "[]".to_string()),
        tag_count: i64::try_from(work.tags.len()).unwrap_or(i64::MAX),
        status: "active",
        tags: work
            .tags
            .iter()
            .map(|tag| crate::db::TagRecord {
                name: tag.name.clone(),
                category: tag.category.clone(),
                score: tag.score,
                translated_name: tag.translated_name.clone(),
            })
            .collect(),
        metadata_synced_at: Some(now()),
        next_retry_at: None,
    };
    let tx = db.transaction()?;
    let work_id = Db::upsert_work(&tx, &row)?;
    for page in 0..row.page_count {
        let item_id = item_id(source, &work.external_id, page);
        Db::persist_page(&tx, &item_id, &work_id, source, page, None, None)?;
    }
    tx.commit()?;
    db.record_work_tags(&work_id, &row.tags)?;
    info!(source, work = %work.external_id, pages = row.page_count, "work ingested");
    Ok(true)
}

pub async fn handle_source_error(
    db: &Db,
    _adapter: &dyn SourceAdapter,
    work_id: Option<&str>,
    error: &SourceError,
) -> Result<()> {
    match error.kind {
        SourceErrorKind::NotFound => {
            if let Some(work_id) = work_id {
                db.mark_work_deleted(work_id)?;
            }
        }
        SourceErrorKind::RateLimited => {
            let retry_at = now() + error.retry_after.map_or(60, |delay| delay.as_secs() as i64);
            db.pause_downloads(Some(retry_at), "rate limited")?;
        }
        SourceErrorKind::Auth => {
            db.pause_downloads(None, "source authentication failed")?;
        }
        SourceErrorKind::Internal | SourceErrorKind::Network => {}
    }
    Ok(())
}

pub fn backoff_delay(attempts: u32, retry_after: Option<std::time::Duration>) -> i64 {
    if let Some(delay) = retry_after {
        return delay.as_secs().max(1) as i64;
    }
    let shift = attempts.min(10);
    let seconds = 2_i64.saturating_pow(shift).min(3600);
    let jitter = (i64::from(attempts) * 7919) % (seconds + 1);
    seconds + jitter
}

pub fn item_id(source: &str, external_id: &str, page: u32) -> String {
    let digest = Sha1::digest(format!("kanalib:item:{source}:{external_id}:{page}").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn all_pages_known(db: &Db, source: &str, work: &SourceWork) -> bool {
    (0..work.page_count.max(1)).all(|page| {
        db.item_exists(&item_id(source, &work.external_id, page))
            .unwrap_or(false)
    })
}
