use serde_json::json;

use crate::model::Asset;

const CSV_HEADER: &str = "path,name,source_root,width,height,bytes,mtime,rating,group,favorite,star,tags,manual_tags,sha1,blake3,phash,tagged,error";

pub fn to_csv(assets: &[Asset]) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for asset in assets {
        let tags = asset
            .tags
            .iter()
            .map(|tag| tag.tag.as_str())
            .collect::<Vec<_>>()
            .join("|");
        let manual_tags = asset
            .manual_tags
            .iter()
            .map(|tag| tag.tag.as_str())
            .collect::<Vec<_>>()
            .join("|");
        let phash = asset
            .phash
            .map(|value| format!("{value:016x}"))
            .unwrap_or_default();
        let fields = [
            asset.path.to_string_lossy().to_string(),
            asset.title(),
            asset.library_root.to_string_lossy().to_string(),
            asset.width.to_string(),
            asset.height.to_string(),
            asset.bytes.to_string(),
            asset.mtime.to_string(),
            asset.rating.clone(),
            asset.group.clone(),
            asset.favorite.to_string(),
            asset.star.to_string(),
            tags,
            manual_tags,
            asset.sha1.clone().unwrap_or_default(),
            asset.blake3.clone().unwrap_or_default(),
            phash,
            asset.tagged.to_string(),
            asset.error.clone().unwrap_or_default(),
        ];
        out.push_str(
            &fields
                .iter()
                .map(|field| csv_field(field))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

pub fn to_json(assets: &[Asset]) -> serde_json::Result<String> {
    let rows = assets
        .iter()
        .map(|asset| {
            json!({
                "path": asset.path.to_string_lossy(),
                "name": asset.title(),
                "source_root": asset.library_root.to_string_lossy(),
                "width": asset.width,
                "height": asset.height,
                "bytes": asset.bytes,
                "mtime": asset.mtime,
                "rating": asset.rating,
                "group": asset.group,
                "favorite": asset.favorite,
                "star": asset.star,
                "tags": asset.tags.iter().map(|tag| json!({
                    "tag": tag.tag,
                    "category": tag.category,
                    "score": tag.score,
                })).collect::<Vec<_>>(),
                "manual_tags": asset.manual_tags.iter().map(|tag| json!({
                    "tag": tag.tag,
                    "category": tag.category,
                })).collect::<Vec<_>>(),
                "sha1": asset.sha1,
                "blake3": asset.blake3,
                "phash": asset.phash.map(|value| format!("{value:016x}")),
                "tagged": asset.tagged,
                "error": asset.error,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&rows)
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{to_csv, to_json};
    use crate::model::{Asset, TagHit};
    use std::path::PathBuf;

    fn tag(name: &str, category: &str, score: f32) -> TagHit {
        TagHit {
            tag: name.to_string(),
            category: category.to_string(),
            score,
            vocab_index: None,
        }
    }

    fn asset() -> Asset {
        Asset {
            path: PathBuf::from("C:\\lib\\a,\"b\".png"),
            library_root: PathBuf::from("C:\\lib"),
            thumb: None,
            tile_thumb: None,
            width: 100,
            height: 200,
            group: "原创".to_string(),
            rating: "G".to_string(),
            favorite: true,
            star: 4,
            tags: vec![tag("1girl", "general", 0.99)],
            manual_tags: vec![tag("custom_tag", "manual", 1.0)],
            tagged: true,
            tag_attempts: 0,
            error: None,
            sha1: Some("sha".to_string()),
            blake3: Some("b3".to_string()),
            phash: Some(0x1234),
            bytes: 300,
            mtime: 400,
        }
    }

    #[test]
    fn csv_escapes_and_includes_manual_tags() {
        let csv = to_csv(&[asset()]);
        assert!(csv.starts_with("path,name,"));
        assert!(csv.contains("\"C:\\lib\\a,\"\"b\"\".png\""));
        assert!(csv.contains(",1girl,custom_tag,"));
        assert!(csv.contains(",0000000000001234,"));
    }

    #[test]
    fn json_round_trips_expected_fields() {
        let json = to_json(&[asset()]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let row = &parsed[0];
        assert_eq!(row["name"], "a,\"b\".png");
        assert_eq!(row["source_root"], "C:\\lib");
        assert_eq!(row["favorite"], true);
        assert_eq!(row["star"], 4);
        assert_eq!(row["tags"][0]["tag"], "1girl");
        assert_eq!(row["manual_tags"][0]["tag"], "custom_tag");
        assert_eq!(row["phash"], "0000000000001234");
    }
}
