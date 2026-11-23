//! 进程内指标：Prometheus 文本格式，供管理页/健康检查抓取。
//!
//! 计数器随进程重启归零；队列深度与 review 账龄由管理接口在抓取时从 DB 计算。

use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Debug, Default)]
pub struct Metrics {
    allow: AtomicU64,
    block: AtomicU64,
    review: AtomicU64,
    model_errors: AtomicU64,
    push_failures: AtomicU64,
    source_errors: AtomicU64,
    reasons: Mutex<BTreeMap<&'static str, u64>>,
    budget_hits: Mutex<BTreeMap<String, u64>>,
    budget_short: Mutex<BTreeMap<String, u64>>,
}

impl Metrics {
    pub fn record_decision(&self, decision: &str) {
        match decision {
            "allow" => self.allow.fetch_add(1, Ordering::Relaxed),
            "block" => self.block.fetch_add(1, Ordering::Relaxed),
            "review" => self.review.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
    }

    pub fn record_reason(&self, reason: &str) {
        increment(&self.reasons, reason_kind(reason));
    }

    pub fn record_tag_budget_hit(&self, source: &str) {
        increment(&self.budget_hits, source.to_string());
    }

    pub fn record_tag_budget_short(&self, source: &str) {
        increment(&self.budget_short, source.to_string());
    }

    pub fn record_model_error(&self) {
        self.model_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_push_failures(&self, count: u64) {
        if count > 0 {
            self.push_failures.fetch_add(count, Ordering::Relaxed);
        }
    }

    pub fn record_source_error(&self) {
        self.source_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn render(&self, gauges: &str) -> String {
        let mut out = String::new();
        out.push_str(
            "# HELP decision_total Decisions produced by the rules engine.\n\
             # TYPE decision_total counter\n",
        );
        for (decision, value) in [
            ("allow", self.allow.load(Ordering::Relaxed)),
            ("block", self.block.load(Ordering::Relaxed)),
            ("review", self.review.load(Ordering::Relaxed)),
        ] {
            out.push_str(&format!(
                "decision_total{{decision=\"{decision}\"}} {value}\n"
            ));
        }

        out.push_str(
            "# HELP decision_reason_total Decisions by reason kind.\n\
             # TYPE decision_reason_total counter\n",
        );
        for (kind, value) in read(&self.reasons) {
            out.push_str(&format!(
                "decision_reason_total{{kind=\"{}\"}} {value}\n",
                escape_label(kind)
            ));
        }

        out.push_str(
            "# HELP tag_budget_hit_total Items sent to the tagger because source tags were short.\n\
             # TYPE tag_budget_hit_total counter\n",
        );
        for (source, value) in read(&self.budget_hits) {
            out.push_str(&format!(
                "tag_budget_hit_total{{source=\"{}\"}} {value}\n",
                escape_label(&source)
            ));
        }

        out.push_str(
            "# HELP tag_budget_short_total Items still short after one model pass.\n\
             # TYPE tag_budget_short_total counter\n",
        );
        for (source, value) in read(&self.budget_short) {
            out.push_str(&format!(
                "tag_budget_short_total{{source=\"{}\"}} {value}\n",
                escape_label(&source)
            ));
        }

        out.push_str(&format!(
            "# TYPE model_errors_total counter\nmodel_errors_total {}\n\
             # TYPE push_failures_total counter\npush_failures_total {}\n\
             # TYPE source_errors_total counter\nsource_errors_total {}\n",
            self.model_errors.load(Ordering::Relaxed),
            self.push_failures.load(Ordering::Relaxed),
            self.source_errors.load(Ordering::Relaxed),
        ));
        out.push_str(gauges);
        out
    }
}

pub fn reason_kind(reason: &str) -> &'static str {
    if reason.starts_with("watch:") {
        return "watch";
    }
    match reason {
        "source disabled" => "source_disabled",
        "author blacklist" => "author_blacklist",
        "r18g" => "r18g",
        "tag blacklist" => "tag_blacklist",
        "duplicate hash" => "dedup",
        "watch list" => "watch",
        "size below minimum" | "empty image" => "size",
        "tag budget short" => "tag_budget",
        "rating sanity review" => "sanity",
        "model tagging failed" | "tag budget short; tagger unavailable" => "model_error",
        "review approved" | "review rejected" => "human",
        "decision error" => "decision_error",
        _ => "other",
    }
}

pub fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn increment<K: Ord>(map: &Mutex<BTreeMap<K, u64>>, key: K) {
    let mut map = map.lock().unwrap_or_else(|poison| poison.into_inner());
    *map.entry(key).or_insert(0) += 1;
}

fn read<K: Clone + Ord>(map: &Mutex<BTreeMap<K, u64>>) -> Vec<(K, u64)> {
    map.lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_bucketed_for_dashboards() {
        assert_eq!(reason_kind("tag blacklist"), "tag_blacklist");
        assert_eq!(reason_kind("duplicate hash"), "dedup");
        assert_eq!(reason_kind("model tagging failed"), "model_error");
        assert_eq!(reason_kind("watch:tentacle"), "watch");
        assert_eq!(reason_kind("decision error"), "decision_error");
        assert_eq!(reason_kind("undefined"), "other");
    }

    #[test]
    fn render_contains_counters_and_gauges() {
        let metrics = Metrics::default();
        metrics.record_decision("allow");
        metrics.record_decision("review");
        metrics.record_reason("watch list");
        metrics.record_tag_budget_hit("pixiv");
        metrics.record_model_error();
        let text = metrics.render("review_age_seconds 12\n");
        assert!(text.contains("decision_total{decision=\"allow\"} 1"));
        assert!(text.contains("decision_total{decision=\"review\"} 1"));
        assert!(text.contains("decision_reason_total{kind=\"watch\"} 1"));
        assert!(text.contains("tag_budget_hit_total{source=\"pixiv\"} 1"));
        assert!(text.contains("model_errors_total 1"));
        assert!(text.ends_with("review_age_seconds 12\n"));
    }

    #[test]
    fn label_escaping_handles_quotes_and_newlines() {
        assert_eq!(escape_label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
