//! I2 review 边界：审核只作用于 review 条目，R-18G 与黑名单终态不可人工放行。

use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};

use crate::db::{Db, now};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    Allow,
    Reject,
}

type ReviewRow = (String, String, Option<i64>, String, Option<String>);

pub fn decide(db: &mut Db, item_id: &str, action: ReviewAction, reviewer: &str) -> Result<String> {
    let row: Option<ReviewRow> = db
        .connection()
        .query_row(
            "SELECT i.decision, w.rating, w.x_restrict, w.restriction_attributes,
                    i.rules_version
               FROM items i JOIN works w ON w.id = i.work_id
              WHERE i.id = ?1",
            [item_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((current, rating, x_restrict, restriction_attributes, rules_version)) = row else {
        bail!("item {item_id} not found");
    };
    if current != "review" {
        bail!("item {item_id} is {current}; only review items can be moderated");
    }
    if action == ReviewAction::Allow
        && is_r18g_item(db, item_id, &rating, x_restrict, &restriction_attributes)?
    {
        bail!("item {item_id} is r18g; human allow is forbidden");
    }
    let decision = match action {
        ReviewAction::Allow => "allow",
        ReviewAction::Reject => "block",
    };
    let reason = match action {
        ReviewAction::Allow => "review approved",
        ReviewAction::Reject => "review rejected",
    };
    let tx = db.transaction()?;
    let previous_matched: Option<String> = tx
        .query_row(
            "SELECT matched_rules_json FROM decisions WHERE item_id = ?1
              ORDER BY id DESC LIMIT 1",
            [item_id],
            |row| row.get(0),
        )
        .optional()?;
    let matched = previous_matched
        .filter(|value| serde_json::from_str::<serde_json::Value>(value).is_ok())
        .unwrap_or_else(|| "[]".to_string());
    tx.execute(
        "UPDATE items SET decision=?2, decision_reason=?3, reviewed_by=?4, reviewed_at=?5,
                revision=revision+1, updated_at=?5 WHERE id=?1",
        params![item_id, decision, reason, reviewer, now()],
    )?;
    let rules_version = rules_version.unwrap_or_else(|| "review".to_string());
    tx.execute(
        "INSERT INTO decisions(item_id, decision, reason, rules_version, matched_rules_json,
                               tag_source, tag_count_used)
         VALUES (?1, ?2, ?3, ?4, ?5, 'review', 0)",
        params![item_id, decision, reason, rules_version, matched],
    )?;
    tx.commit()?;
    Ok(decision.to_string())
}

pub fn is_r18g_item(
    db: &Db,
    item_id: &str,
    rating: &str,
    x_restrict: Option<i64>,
    restriction_attributes: &str,
) -> Result<bool> {
    Ok(is_r18g_with_tags(
        rating,
        x_restrict,
        restriction_attributes,
        &db.item_tag_names(item_id)?,
    ))
}

/// 已拿到标签集合时的同判定；审核列表用它避免逐条查库。
pub fn is_r18g_with_tags(
    rating: &str,
    x_restrict: Option<i64>,
    restriction_attributes: &str,
    tags: &[(String, Option<String>)],
) -> bool {
    if rating == "r18g" || x_restrict == Some(2) {
        return true;
    }
    let attributes: Vec<String> = serde_json::from_str(restriction_attributes).unwrap_or_default();
    if attributes
        .iter()
        .any(|attribute| crate::rules::normalize_tag(attribute) == "r18g")
    {
        return true;
    }
    let is_r18g_tag = |value: &str| {
        let tag = crate::rules::normalize_tag(value);
        tag == "r18g" || tag == "r-18g"
    };
    for (name, translated) in tags {
        if is_r18g_tag(name) {
            return true;
        }
        if translated.as_deref().is_some_and(is_r18g_tag) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(db: &Db, rating: &str, decision: &str) -> String {
        db.connection()
            .execute(
                "INSERT INTO authors(id, source, external_id) VALUES ('a', 'mock', 'a')",
                [],
            )
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO works(id, source, external_id, author_id, rating)
                 VALUES ('w', 'mock', 'w', 'a', ?1)",
                [rating],
            )
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO items(id, work_id, source, decision) VALUES ('i', 'w', 'mock', ?1)",
                [decision],
            )
            .unwrap();
        "i".to_string()
    }

    #[test]
    fn only_review_items_can_be_moderated() {
        let mut db = Db::open_in_memory().unwrap();
        let id = seed(&db, "all", "review");
        assert_eq!(
            decide(&mut db, &id, ReviewAction::Allow, "admin").unwrap(),
            "allow"
        );
        assert!(decide(&mut db, &id, ReviewAction::Reject, "admin").is_err());
    }

    #[test]
    fn r18g_cannot_be_allowed() {
        let mut db = Db::open_in_memory().unwrap();
        let id = seed(&db, "r18g", "review");
        assert!(decide(&mut db, &id, ReviewAction::Allow, "admin").is_err());
    }

    #[test]
    fn r18g_tag_or_restriction_attribute_also_blocks_allow() {
        let mut db = Db::open_in_memory().unwrap();
        let id = seed(&db, "all", "review");
        db.connection()
            .execute("INSERT INTO tags(id, name) VALUES (1, 'R-18G')", [])
            .unwrap();
        db.connection()
            .execute("INSERT INTO item_tags(item_id, tag_id) VALUES ('i', 1)", [])
            .unwrap();
        assert!(decide(&mut db, &id, ReviewAction::Allow, "admin").is_err());

        let mut db = Db::open_in_memory().unwrap();
        let id = seed(&db, "all", "review");
        db.connection()
            .execute(
                "UPDATE works SET restriction_attributes = '[\"R18G\"]' WHERE id = 'w'",
                [],
            )
            .unwrap();
        assert!(decide(&mut db, &id, ReviewAction::Allow, "admin").is_err());
    }

    #[test]
    fn human_decision_preserves_matched_rules_json() {
        let mut db = Db::open_in_memory().unwrap();
        let id = seed(&db, "all", "review");
        db.connection()
            .execute(
                "INSERT INTO decisions(item_id, decision, reason, rules_version,
                                       matched_rules_json)
                 VALUES ('i', 'review', 'watch:tentacle', 'v1', '[\"watch:tentacle\"]')",
                [],
            )
            .unwrap();
        decide(&mut db, &id, ReviewAction::Allow, "admin").unwrap();
        let matched: String = db
            .connection()
            .query_row(
                "SELECT matched_rules_json FROM decisions WHERE item_id = 'i'
                  ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(matched, "[\"watch:tentacle\"]");
        assert!(serde_json::from_str::<serde_json::Value>(&matched).is_ok());
    }
}
