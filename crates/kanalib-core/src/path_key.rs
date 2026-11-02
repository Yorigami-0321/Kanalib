//! 无损路径键：数据库主键与缓存键不能使用 `to_string_lossy`，
//! 否则含未配对 UTF-16 代理（Windows）或非 UTF-8 字节（Unix）的不同路径
//! 会塌缩成同一个 `U+FFFD` 字符串。
//!
//! 普通 UTF-8 路径原样返回；只有真正需要转义时才加 `\u{1}w:`（UTF-16LE）
//! 或 `\u{1}b:`（原始字节）前缀，旧库中的普通路径读取语义不变。

use std::path::{Path, PathBuf};

const ESCAPE: char = '\u{1}';
const WIDE_PREFIX: &str = "\u{1}w:";
#[cfg(unix)]
const BYTES_PREFIX: &str = "\u{1}b:";

/// 把路径编码成可逆的数据库/缓存键。正常路径不做任何改写。
pub fn encode(path: &Path) -> String {
    #[cfg(windows)]
    {
        encode_windows(path)
    }
    #[cfg(unix)]
    {
        encode_unix(path)
    }
    #[cfg(not(any(windows, unix)))]
    {
        path.to_string_lossy().into_owned()
    }
}

/// 把数据库/缓存键还原为路径；普通值按原字符串处理。
pub fn decode(value: &str) -> PathBuf {
    #[cfg(windows)]
    {
        decode_windows(value)
    }
    #[cfg(unix)]
    {
        decode_unix(value)
    }
    #[cfg(not(any(windows, unix)))]
    {
        PathBuf::from(value)
    }
}

#[cfg(windows)]
fn encode_windows(path: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::windows::ffi::OsStrExt;

    if let Some(text) = path.to_str()
        && !text.starts_with(ESCAPE)
    {
        return text.to_string();
    }
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let mut encoded = String::with_capacity(WIDE_PREFIX.len() + units.len() * 4);
    encoded.push_str(WIDE_PREFIX);
    for unit in units {
        let _ = write!(encoded, "{unit:04x}");
    }
    encoded
}

#[cfg(windows)]
fn decode_windows(value: &str) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;

    let Some(hex) = value.strip_prefix(WIDE_PREFIX) else {
        return PathBuf::from(value);
    };
    if hex.len() % 4 != 0 {
        return PathBuf::from(value);
    }
    let mut units = Vec::with_capacity(hex.len() / 4);
    for chunk in hex.as_bytes().chunks_exact(4) {
        let Ok(text) = std::str::from_utf8(chunk) else {
            return PathBuf::from(value);
        };
        let Ok(unit) = u16::from_str_radix(text, 16) else {
            return PathBuf::from(value);
        };
        units.push(unit);
    }
    PathBuf::from(std::ffi::OsString::from_wide(&units))
}

#[cfg(unix)]
fn encode_unix(path: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt;

    if let Some(text) = path.to_str()
        && !text.starts_with(ESCAPE)
    {
        return text.to_string();
    }
    let mut encoded = String::with_capacity(BYTES_PREFIX.len() + path.as_os_str().len() * 2);
    encoded.push_str(BYTES_PREFIX);
    for byte in path.as_os_str().as_bytes() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(unix)]
fn decode_unix(value: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;

    let Some(hex) = value.strip_prefix(BYTES_PREFIX) else {
        return PathBuf::from(value);
    };
    if hex.len() % 2 != 0 {
        return PathBuf::from(value);
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks_exact(2) {
        let Ok(text) = std::str::from_utf8(chunk) else {
            return PathBuf::from(value);
        };
        let Ok(byte) = u8::from_str_radix(text, 16) else {
            return PathBuf::from(value);
        };
        bytes.push(byte);
    }
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    use std::path::PathBuf;

    #[test]
    fn ordinary_paths_round_trip_unchanged() {
        let path = PathBuf::from("C:\\lib\\photo.png");
        assert_eq!(encode(&path), "C:\\lib\\photo.png");
        assert_eq!(decode(&encode(&path)), path);
    }

    #[cfg(windows)]
    #[test]
    fn unpaired_surrogate_paths_stay_distinct_and_reversible() {
        use std::os::windows::ffi::OsStringExt;

        let first = PathBuf::from(std::ffi::OsString::from_wide(&[
            'C' as u16,
            ':' as u16,
            '\\' as u16,
            0xD800,
            '.' as u16,
            'p' as u16,
            'n' as u16,
            'g' as u16,
        ]));
        let second = PathBuf::from(std::ffi::OsString::from_wide(&[
            'C' as u16,
            ':' as u16,
            '\\' as u16,
            0xD801,
            '.' as u16,
            'p' as u16,
            'n' as u16,
            'g' as u16,
        ]));
        assert_ne!(encode(&first), encode(&second));
        assert!(!encode(&first).contains('\u{FFFD}'));
        assert_eq!(decode(&encode(&first)), first);
        assert_eq!(decode(&encode(&second)), second);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_stay_distinct_and_reversible() {
        use std::os::unix::ffi::OsStringExt;

        let first = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\xff.png".to_vec()));
        let second = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\xfe.png".to_vec()));
        assert_ne!(encode(&first), encode(&second));
        assert_eq!(decode(&encode(&first)), first);
        assert_eq!(decode(&encode(&second)), second);
    }
}
