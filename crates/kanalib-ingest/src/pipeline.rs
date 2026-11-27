//! 决策流水线：取待决策条目、套规则、标签不足补标一次后复判，并落库。
//!
//! 失败语义固定在 rules 层：补标失败、无后端、返回空标签，一律 review，绝不 fail-open。
//! 单个条目的读取/判定/写库异常只影响该条：转 review 后继续下一条，不阻断整轮。

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use futures::StreamExt;
use tracing::warn;

use crate::{
    db::{Db, PendingDecisionItem},
    metrics::Metrics,
    rules::{self, Decision, DecisionInput, RuleConfig, RuleOutcome, RuleSubject},
    source::{AiType, Rating},
    tagging::TaggingBackend,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecisionReport {
    pub decided: usize,
    pub allowed: usize,
    pub blocked: usize,
    pub reviewed: usize,
    pub tagged: usize,
}

struct ItemDecision {
    outcome: RuleOutcome,
    tag_source: &'static str,
    tag_count: usize,
    tagged: bool,
}

/// 对 `items.decision = 'pending'` 且已下载完成的条目逐条判定，写入 items 与 decisions。
pub async fn run_decisions(
    db: &Db,
    config: &Arc<RuleConfig>,
    tagging: Option<&Arc<dyn TaggingBackend>>,
    metrics: &Metrics,
) -> Result<DecisionReport> {
    run_decisions_with_concurrency(db, config, tagging, metrics, 1).await
}

/// 同上，但最多同时处理 `concurrency` 条；每个并发任务使用独立读连接。
///
/// tagger 本身串行推理，这里的并发用于避免单条慢请求阻塞整轮决策。
pub async fn run_decisions_with_concurrency(
    db: &Db,
    config: &Arc<RuleConfig>,
    tagging: Option<&Arc<dyn TaggingBackend>>,
    metrics: &Metrics,
    concurrency: usize,
) -> Result<DecisionReport> {
    let pending = db.pending_decision_items()?;
    let mut report = DecisionReport::default();
    if pending.is_empty() {
        return Ok(report);
    }
    if concurrency <= 1 {
        for item in &pending {
            let scan = process_one(db, config, tagging, metrics, item).await;
            tally(&mut report, scan, metrics);
        }
        return Ok(report);
    }
    let Some(path) = db
        .connection()
        .path()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
    else {
        for item in &pending {
            let scan = process_one(db, config, tagging, metrics, item).await;
            tally(&mut report, scan, metrics);
        }
        return Ok(report);
    };
    // 每个 worker 复用一条连接，避免并发时每条都重开库、跑一遍迁移。
    let workers = concurrency.clamp(1, 16).min(pending.len());
    let queue = Arc::new(Mutex::new(pending.into_iter().collect::<VecDeque<_>>()));
    let scans = futures::stream::iter(0..workers)
        .map(|_| {
            let queue = queue.clone();
            let path = path.clone();
            async move {
                let Ok(db) = Db::open(&path) else {
                    warn!("open decision connection failed");
                    return Vec::new();
                };
                let mut scans = Vec::new();
                loop {
                    let item = queue.lock().expect("decision queue").pop_front();
                    let Some(item) = item else {
                        break;
                    };
                    scans.push(process_one(&db, config, tagging, metrics, &item).await);
                }
                scans
            }
        })
        .buffer_unordered(workers)
        .collect::<Vec<_>>()
        .await;
    for scan in scans.into_iter().flatten() {
        tally(&mut report, scan, metrics);
    }
    Ok(report)
}

enum ItemScan {
    Applied(ItemDecision),
    FallbackReview,
    Failed,
}

async fn process_one(
    db: &Db,
    config: &RuleConfig,
    tagging: Option<&Arc<dyn TaggingBackend>>,
    metrics: &Metrics,
    pending: &PendingDecisionItem,
) -> ItemScan {
    let decision = match decide_item(db, config, tagging, metrics, pending).await {
        Ok(decision) => decision,
        Err(error) => {
            warn!(item = %pending.id, %error, "decision failed; recording review");
            return if record_fallback_review(db, config, &pending.id, metrics) {
                ItemScan::FallbackReview
            } else {
                ItemScan::Failed
            };
        }
    };
    if let Err(error) = db.apply_decision(
        &pending.id,
        decision.outcome.decision.as_str(),
        &decision.outcome.reason,
        &decision.outcome.matched,
        &config.rules.version,
        decision.tag_source,
        decision.tag_count,
    ) {
        warn!(item = %pending.id, %error, "decision write failed; recording review");
        return if record_fallback_review(db, config, &pending.id, metrics) {
            ItemScan::FallbackReview
        } else {
            ItemScan::Failed
        };
    }
    ItemScan::Applied(decision)
}

fn tally(report: &mut DecisionReport, scan: ItemScan, metrics: &Metrics) {
    match scan {
        ItemScan::Applied(decision) => {
            report.decided += 1;
            if decision.tagged {
                report.tagged += 1;
            }
            match decision.outcome.decision {
                Decision::Allow => report.allowed += 1,
                Decision::Block => report.blocked += 1,
                Decision::Review => report.reviewed += 1,
            }
            metrics.record_decision(decision.outcome.decision.as_str());
            metrics.record_reason(&decision.outcome.reason);
        }
        ItemScan::FallbackReview => {
            report.decided += 1;
            report.reviewed += 1;
        }
        ItemScan::Failed => {}
    }
}

async fn decide_item(
    db: &Db,
    config: &RuleConfig,
    tagging: Option<&Arc<dyn TaggingBackend>>,
    metrics: &Metrics,
    pending: &PendingDecisionItem,
) -> Result<ItemDecision> {
    let mut subject = RuleSubject {
        source: pending.source.clone(),
        source_enabled: db.source_enabled(&pending.source)?,
        author_external_id: db.item_author(&pending.id)?,
        rating: Rating::parse(&pending.rating),
        x_restrict: pending.x_restrict,
        sanity_level: pending.sanity_level,
        restriction_attributes: serde_json::from_str(&pending.restriction_attributes)
            .unwrap_or_default(),
        ai: match pending.ai_type.as_str() {
            "ai" => AiType::Ai,
            "no" => AiType::No,
            _ => AiType::Unknown,
        },
        tags: db.item_tag_names(&pending.id)?,
        // 已补过标的条目重跑规则时不再调用模型：同一张图不会因为 review 重跑而反复烧 GPU。
        tagged: db.has_model_tags(&pending.id)?,
        tag_count: pending.tag_count,
        width: pending.width,
        height: pending.height,
        bytes: pending.bytes,
        sha1: pending.sha1.clone(),
        item_id: pending.id.clone(),
    };
    let mut tag_source = "source";
    let mut tagged = false;
    let mut outcome = rules::evaluate(DecisionInput {
        db,
        config,
        subject: &subject,
    })?;
    if outcome.needs_tagging {
        metrics.record_tag_budget_hit(&subject.source);
        let Some(backend) = tagging else {
            metrics.record_model_error();
            outcome = RuleOutcome::review_public("tag budget short; tagger unavailable", vec![]);
            return Ok(ItemDecision {
                outcome,
                tag_source,
                tag_count: subject.tags.len(),
                tagged,
            });
        };
        let image_path = pending.original_path.as_deref().unwrap_or_default();
        match backend
            .tag(&pending.id, std::path::Path::new(image_path))
            .await
        {
            Ok(tags) => {
                metrics.record_model_success();
                db.record_model_tags(&pending.id, &tags.tags)?;
                subject.tags = db.item_tag_names(&pending.id)?;
                subject.tagged = true;
                subject.tag_count = subject.tags.len();
                tag_source = if pending.tag_count == 0 {
                    "model"
                } else {
                    "merged"
                };
                tagged = true;
                outcome = rules::evaluate(DecisionInput {
                    db,
                    config,
                    subject: &subject,
                })?;
                if outcome.needs_tagging {
                    metrics.record_tag_budget_short(&subject.source);
                }
            }
            Err(error) => {
                metrics.record_model_error();
                warn!(item = %pending.id, %error, "model tagging failed; review");
                outcome = RuleOutcome::review_public("model tagging failed", vec![]);
            }
        }
    }

    Ok(ItemDecision {
        outcome,
        tag_source,
        tag_count: if tag_source == "source" {
            subject.tag_count
        } else {
            subject.tags.len()
        },
        tagged,
    })
}

fn record_fallback_review(db: &Db, config: &RuleConfig, item_id: &str, metrics: &Metrics) -> bool {
    if let Err(error) = db.apply_decision(
        item_id,
        "review",
        "decision error",
        &[],
        &config.rules.version,
        "source",
        0,
    ) {
        warn!(item = %item_id, %error, "review fallback write failed; item stays pending");
        return false;
    }
    metrics.record_decision("review");
    metrics.record_reason("decision error");
    true
}
