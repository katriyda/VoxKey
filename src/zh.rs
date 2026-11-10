//! Lightweight Simplified -> Traditional character conversion used only for
//! the `zh-TW` language option. The table is derived from OpenCC's
//! `TSCharacters.txt` (Apache-2.0) and contains one best-effort mapping per
//! character. Phrase-level regional conversion is intentionally out of scope.

use std::collections::HashMap;
use std::sync::OnceLock;

fn table() -> &'static HashMap<char, char> {
    static TABLE: OnceLock<HashMap<char, char>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let raw = include_str!("../assets/zh_hans_to_hant.tsv");
        let mut map = HashMap::with_capacity(5000);
        for line in raw.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let (Some(from), Some(to)) = (parts.next(), parts.next()) else {
                continue;
            };
            let mut from_chars = from.chars();
            let mut to_chars = to.chars();
            if let (Some(from), Some(to), None, None) = (
                from_chars.next(),
                to_chars.next(),
                from_chars.next(),
                to_chars.next(),
            ) {
                map.insert(from, to);
            }
        }
        map
    })
}

pub fn to_traditional(text: &str) -> String {
    let mappings = table();
    text.chars()
        .map(|ch| mappings.get(&ch).copied().unwrap_or(ch))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_characters() {
        assert_eq!(to_traditional("语音输入，软件测试"), "語音輸入，軟件測試");
        assert_eq!(to_traditional("VoxKey 2026"), "VoxKey 2026");
    }
}
