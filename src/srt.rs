//! SRT 字幕格式化。

use crate::prproj::{Caption, TICKS_PER_SECOND};

/// tick -> "HH:MM:SS,mmm"，四舍五入到毫秒。
pub fn format_timestamp(ticks: u64) -> String {
    let total_ms =
        ((ticks as u128 * 1000 + (TICKS_PER_SECOND as u128 / 2)) / TICKS_PER_SECOND as u128) as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

/// 时长（tick）转成便于阅读的 "mm:ss" / "h:mm:ss"。
pub fn format_duration(ticks: u64) -> String {
    let total_s = ticks / TICKS_PER_SECOND;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 生成 SRT 文本，返回 (内容, 实际写入的字幕条数)。
/// `skip_empty` 为 true 时忽略没有文字的条目（空字幕在 SRT 中无意义）。
pub fn render(items: &[Caption], skip_empty: bool) -> (String, usize) {
    let mut out = String::with_capacity(items.len() * 96);
    let mut n = 0usize;
    for c in items {
        let text = c.text.trim();
        if text.is_empty() && skip_empty {
            continue;
        }
        n += 1;
        out.push_str(&n.to_string());
        out.push_str("\r\n");
        out.push_str(&format_timestamp(c.start));
        out.push_str(" --> ");
        out.push_str(&format_timestamp(c.end));
        out.push_str("\r\n");
        if text.is_empty() {
            out.push_str(" ");
        } else {
            out.push_str(&c.text);
        }
        out.push_str("\r\n\r\n");
    }
    (out, n)
}

/// 把多条轨道的字幕合并成一条时间线。
///
/// 规则：
/// - 按开始时间排序（开始相同再比结束），同一条时间码上的多条按原轨道顺序排列；
/// - 起止时间与文本都相同的条目视为重复，只保留第一条；
/// - 不改动条目的时间与文本，交给调用方决定是否过滤空字幕。
pub fn merge<'a, I>(sources: I) -> Vec<Caption>
where
    I: IntoIterator<Item = &'a [Caption]>,
{
    let mut all: Vec<Caption> = Vec::new();
    for src in sources {
        all.extend(src.iter().cloned());
    }
    all.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    all.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.text.trim() == b.text.trim());
    all
}

/// Windows 文件名非法字符替换。
pub fn sanitize_filename(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            '\r' | '\n' | '\t' => ' ',
            c => c,
        })
        .collect();
    s = s.trim().trim_end_matches('.').trim().to_string();
    if s.is_empty() {
        s = "字幕".into();
    }
    if s.chars().count() > 120 {
        s = s.chars().take(120).collect();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(start: u64, end: u64, text: &str) -> Caption {
        Caption {
            start,
            end,
            text: text.to_string(),
        }
    }

    #[test]
    fn merge_sorts_and_dedups_identical_entries() {
        let c1 = [cap(200, 300, "丙"), cap(100, 200, "乙")];
        let c2 = [cap(100, 200, "乙"), cap(50, 90, "甲")];
        let c3 = [cap(50, 90, " 甲 "), cap(100, 200, "乙的改写")];
        let m = merge([&c1[..], &c2[..], &c3[..]]);
        let texts: Vec<&str> = m.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            ["甲", "乙", "乙的改写", "丙"],
            "同时排查序并去掉重复"
        );
    }

    #[test]
    fn merge_keeps_entries_with_same_text_but_different_time() {
        let a = [cap(0, 10, "同一句")];
        let b = [cap(100, 110, "同一句")];
        assert_eq!(merge([&a[..], &b[..]]).len(), 2);
    }
}
