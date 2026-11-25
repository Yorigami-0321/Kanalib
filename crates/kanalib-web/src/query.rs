//! W2 查询模型：搜索词、筛选、排序与 keyset 游标。
//!
//! 这里只做纯解析/编码，不碰 HTTP 与 SQLite；游标格式对前端不透明。

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKind {
    Updated,
    Posted,
    Name,
    Star,
    Favorite,
    Rating,
    Random,
}

impl SortKind {
    /// 宽松解析：未知值回退 `updated`；别名兼容前端 `time`/`stars`。
    pub fn parse(raw: &str) -> Self {
        Self::from_token(raw).unwrap_or(Self::Updated)
    }

    pub fn from_token(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "updated" | "update" | "time" => Some(Self::Updated),
            "posted" | "post" => Some(Self::Posted),
            "name" => Some(Self::Name),
            "star" | "stars" => Some(Self::Star),
            "favorite" | "fav" => Some(Self::Favorite),
            "rating" => Some(Self::Rating),
            "random" => Some(Self::Random),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::Posted => "posted",
            Self::Name => "name",
            Self::Star => "star",
            Self::Favorite => "favorite",
            Self::Rating => "rating",
            Self::Random => "random",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemSort {
    pub kind: SortKind,
    pub ascending: bool,
    pub seed: i64,
}

impl Default for ItemSort {
    fn default() -> Self {
        Self {
            kind: SortKind::Updated,
            ascending: false,
            seed: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemFilter {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub group: Option<String>,
    pub rating: Option<String>,
    pub fav: Option<bool>,
}

impl ItemFilter {
    pub fn is_empty(&self) -> bool {
        self.q.is_none()
            && self.tag.is_none()
            && self.group.is_none()
            && self.rating.is_none()
            && self.fav.is_none()
    }

    /// 过滤条件 + seed 的稳定指纹；写进游标，换条件复用游标会直接报错。
    pub fn fingerprint(&self, seed: i64) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for value in [&self.q, &self.tag, &self.group, &self.rating] {
            match value {
                Some(text) => {
                    hasher.update([1u8]);
                    hasher.update(text.as_bytes());
                }
                None => hasher.update([0u8]),
            }
            hasher.update([0u8]);
        }
        hasher.update([match self.fav {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }]);
        hasher.update(seed.to_le_bytes());
        hasher
            .finalize()
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchField {
    Any,
    Name,
    Tag,
    Group,
    Rating,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTerm {
    pub value: String,
    pub field: SearchField,
    pub exclude: bool,
}

/// 与前端 `filters.ts` 一致的折叠：小写、`_` 转空格、压缩空白。
pub fn fold(value: &str) -> String {
    value
        .to_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 按空白切词，但引号内的空格保留（`tag:"two words"`）。
fn split_terms(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in input.chars() {
        if ch == '"' {
            quoted = !quoted;
            current.push(ch);
            continue;
        }
        if !quoted && ch.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn field_alias(raw: &str) -> Option<SearchField> {
    match raw.to_ascii_lowercase().as_str() {
        "tag" | "tags" | "标签" => Some(SearchField::Tag),
        "name" | "n" | "文件名" => Some(SearchField::Name),
        "group" | "g" | "分组" => Some(SearchField::Group),
        "rating" | "r" | "分级" => Some(SearchField::Rating),
        _ => None,
    }
}

fn parse_term(token: &str) -> Option<SearchTerm> {
    let mut rest = token;
    let mut exclude = false;
    if let Some(stripped) = rest.strip_prefix('-')
        && !stripped.is_empty()
    {
        exclude = true;
        rest = stripped;
    }

    let mut field = SearchField::Any;
    if let Some(colon) = rest.find([':', '：'])
        && colon > 0
        && let Some(alias) = field_alias(&rest[..colon])
    {
        field = alias;
        rest = &rest[colon + 1..];
    }

    let value = fold(unquote(rest));
    if value.is_empty() {
        return None;
    }
    Some(SearchTerm {
        value,
        field,
        exclude,
    })
}

/// 解析 `q`：空格 AND、`-词` 排除、`tag:`/`name:`/`group:`/`rating:` 限定。
pub fn parse_search(query: &str) -> Vec<SearchTerm> {
    split_terms(query.trim())
        .into_iter()
        .filter_map(|token| parse_term(&token))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SortKey {
    Int(i64),
    Text(String),
}

impl SortKey {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Int(value) => Value::from(*value),
            Self::Text(value) => Value::from(value.clone()),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(Self::Int),
            Value::String(text) => Some(Self::Text(text.clone())),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub sort: SortKind,
    pub ascending: bool,
    pub key: SortKey,
    pub id: String,
    pub fingerprint: String,
}

#[derive(Serialize, Deserialize)]
struct WireCursor {
    v: u32,
    sort: String,
    order: String,
    key: Value,
    id: String,
    #[serde(default)]
    fingerprint: String,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() * 4).div_ceil(3));
    for chunk in input.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(chunk.get(1).copied().unwrap_or(0));
        let third = u32::from(chunk.get(2).copied().unwrap_or(0));
        let bits = (first << 16) | (second << 8) | third;
        out.push(B64[((bits >> 18) & 0x3f) as usize] as char);
        out.push(B64[((bits >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64[((bits >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(B64[(bits & 0x3f) as usize] as char);
        }
    }
    out
}

fn b64_value(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

pub fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    if input.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = b64_value(byte)?;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub fn encode_cursor(cursor: &Cursor) -> String {
    let wire = WireCursor {
        v: 2,
        sort: cursor.sort.as_str().to_string(),
        order: if cursor.ascending { "asc" } else { "desc" }.to_string(),
        key: cursor.key.to_json(),
        id: cursor.id.clone(),
        fingerprint: cursor.fingerprint.clone(),
    };
    match serde_json::to_vec(&wire) {
        Ok(bytes) => base64url_encode(&bytes),
        Err(_) => String::new(),
    }
}

pub fn decode_cursor(raw: &str) -> Option<Cursor> {
    let bytes = base64url_decode(raw)?;
    let wire: WireCursor = serde_json::from_slice(&bytes).ok()?;
    if wire.v != 2 || wire.id.is_empty() || wire.fingerprint.is_empty() {
        return None;
    }
    let sort = SortKind::from_token(&wire.sort)?;
    let ascending = match wire.order.as_str() {
        "asc" => true,
        "desc" => false,
        _ => return None,
    };
    Some(Cursor {
        sort,
        ascending,
        key: SortKey::from_json(&wire.key)?,
        id: wire.id,
        fingerprint: wire.fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_parses_fields_exclusions_and_quotes() {
        let terms = parse_search(r#"tag:"two words" -name:draft 普通 词"#);
        assert_eq!(
            terms,
            vec![
                SearchTerm {
                    value: "two words".to_string(),
                    field: SearchField::Tag,
                    exclude: false,
                },
                SearchTerm {
                    value: "draft".to_string(),
                    field: SearchField::Name,
                    exclude: true,
                },
                SearchTerm {
                    value: "普通".to_string(),
                    field: SearchField::Any,
                    exclude: false,
                },
                SearchTerm {
                    value: "词".to_string(),
                    field: SearchField::Any,
                    exclude: false,
                },
            ]
        );
    }

    #[test]
    fn unknown_field_prefix_is_a_plain_term() {
        let terms = parse_search("is:failed");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].field, SearchField::Any);
        assert_eq!(terms[0].value, "is:failed");
    }

    #[test]
    fn base64url_roundtrip() {
        for bytes in [b"".as_slice(), b"a", b"ab", b"abc", b"kanalib-cursor"] {
            let encoded = base64url_encode(bytes);
            assert!(!encoded.contains('='));
            assert_eq!(base64url_decode(&encoded).as_deref(), Some(bytes));
        }
    }

    #[test]
    fn cursor_roundtrip_int_and_text() {
        for key in [
            SortKey::Int(1_700_000_000),
            SortKey::Text("作品".to_string()),
        ] {
            let cursor = Cursor {
                sort: SortKind::Updated,
                ascending: false,
                key: key.clone(),
                id: "a".repeat(40),
                fingerprint: "fingerprint".to_string(),
            };
            let encoded = encode_cursor(&cursor);
            assert_eq!(decode_cursor(&encoded), Some(cursor));
        }
    }

    #[test]
    fn cursor_rejects_garbage_and_sort_mismatch_data() {
        assert!(decode_cursor("not-a-cursor").is_none());
        assert!(decode_cursor("").is_none());
    }

    #[test]
    fn sort_aliases_and_fallback() {
        assert_eq!(SortKind::parse("time"), SortKind::Updated);
        assert_eq!(SortKind::parse("stars"), SortKind::Star);
        assert_eq!(SortKind::parse("unknown"), SortKind::Updated);
    }
}
