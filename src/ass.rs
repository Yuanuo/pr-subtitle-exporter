//! ASS 字幕生成。
//!
//! 模板是一整份 ASS 文本，其中 `{{EVENTS}}` 会被替换成全部 `Dialogue:` 行；
//! 若模板里没有该标记，字幕行追加到文件末尾。
//! 另外支持 `{{TITLE}}` / `{{PROJECT}}` / `{{SEQUENCE}}` 三个占位符。

use crate::prproj::{Caption, TICKS_PER_SECOND};

pub const TEMPLATE_MARKER: &str = "{{EVENTS}}";

pub const DEFAULT_TEMPLATE: &str = r#"[Script Info]
; ===== PR 字幕导出器 · ASS 模板 =====
; 这份模板可以随意修改，改完点「保存」，之后导出的 ASS 都用它。
; 占位符：{{EVENTS}} = 全部字幕行；{{TITLE}} = 工程 - 序列；{{PROJECT}} / {{SEQUENCE}}
; 字幕行的样式名会自动取下面 [V4+ Styles] 里第一条样式的名字。
Title: {{TITLE}}
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080
WrapStyle: 0
ScaledBorderAndShadow: yes
YCbCr Matrix: TV.709

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Microsoft YaHei,64,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,3,0,2,60,60,48,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
{{EVENTS}}
"#;

/// ASS 时间：`H:MM:SS.cc`（百分秒），四舍五入。
pub fn format_time(ticks: u64) -> String {
    let cs =
        ((ticks as u128 * 100 + (TICKS_PER_SECOND as u128 / 2)) / TICKS_PER_SECOND as u128) as u64;
    let centi = cs % 100;
    let total_s = cs / 100;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h}:{m:02}:{s:02}.{centi:02}")
}

/// 取模板里第一条样式的名字，缺省 `Default`。
pub fn style_name(template: &str) -> String {
    for line in template.lines() {
        let t = line.trim_start();
        // 用 get() 按 UTF-8 边界判断前缀，避免中文注释行被按字节切断
        let is_style = t
            .get(..6)
            .map(|p| p.eq_ignore_ascii_case("style:"))
            .unwrap_or(false);
        if is_style {
            let name = t[6..].split(',').next().unwrap_or("").trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    "Default".to_string()
}

fn escape_text(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end()
        .replace('\n', "\\N")
}

/// 生成 ASS 文本，返回 (内容, 字幕行数)。
pub fn render(
    items: &[Caption],
    project: &str,
    sequence: &str,
    template: &str,
    skip_empty: bool,
) -> (String, usize) {
    let style = style_name(template);
    let mut events = String::with_capacity(items.len() * 80);
    let mut n = 0usize;

    for c in items {
        let text = c.text.trim();
        if text.is_empty() && skip_empty {
            continue;
        }
        n += 1;
        let body = if text.is_empty() {
            " ".to_string()
        } else {
            escape_text(&c.text)
        };
        events.push_str("Dialogue: 0,");
        events.push_str(&format_time(c.start));
        events.push(',');
        events.push_str(&format_time(c.end));
        events.push(',');
        events.push_str(&style);
        events.push_str(",,0,0,0,,");
        events.push_str(&body);
        events.push('\n');
    }

    let title = if project.is_empty() {
        sequence.to_string()
    } else {
        format!("{project} - {sequence}")
    };
    // 只有「整行就是 {{EVENTS}}」的行才被替换，避免注释里提到它时被误替换
    let subst = |line: &str| -> String {
        line.replace("{{TITLE}}", &title)
            .replace("{{PROJECT}}", project)
            .replace("{{SEQUENCE}}", sequence)
    };

    let mut out = String::with_capacity(template.len() + events.len());
    let mut inserted = false;
    for line in template.lines() {
        let trimmed = line.trim_start();
        if trimmed == TEMPLATE_MARKER {
            out.push_str(&events);
            inserted = true;
        } else if trimmed.starts_with(';') {
            // 注释行原样保留，不做占位符替换
            out.push_str(line);
            out.push('\n');
        } else {
            out.push_str(&subst(line));
            out.push('\n');
        }
    }
    if !inserted {
        out.push_str(&events);
    }
    (out, n)
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
    fn time_format_rounds_to_centiseconds() {
        assert_eq!(format_time(0), "0:00:00.00");
        assert_eq!(format_time(TICKS_PER_SECOND), "0:00:01.00");
        // 5.967 秒 -> 5.97
        assert_eq!(format_time(1_515_628_800_000), "0:00:05.97");
        assert_eq!(format_time(3_600 * TICKS_PER_SECOND), "1:00:00.00");
    }

    #[test]
    fn style_name_reads_first_style_line() {
        assert_eq!(style_name(DEFAULT_TEMPLATE), "Default");
        let t =
            "[V4+ Styles]\nFormat: Name, Fontname\nStyle: 正文,SimHei,64\nStyle: 注释,Arial,40\n";
        assert_eq!(style_name(t), "正文");
        assert_eq!(style_name("no styles here"), "Default");
        // 中文注释/空行不应被误判
        assert_eq!(
            style_name("; 这是一行中文注释，长度超过六个字节\n"),
            "Default"
        );
    }

    #[test]
    fn render_replaces_marker_only_on_own_line() {
        let items = vec![
            cap(0, TICKS_PER_SECOND, "第一行\n第二行"),
            cap(2 * TICKS_PER_SECOND, 3 * TICKS_PER_SECOND, "b"),
        ];
        let (out, n) = render(&items, "工程P", "序列S", DEFAULT_TEMPLATE, true);
        assert_eq!(n, 2);
        // 注释行里的 {{EVENTS}} 保持原样
        assert!(out.contains("; 占位符：{{EVENTS}} = 全部字幕行"));
        // 只插入一次
        assert_eq!(out.matches("Dialogue: 0,").count(), 2);
        // 换行转 \N
        assert!(out.contains("第一行\\N第二行"));
        // 占位符替换
        assert!(out.contains("Title: 工程P - 序列S"));
    }

    #[test]
    fn render_without_marker_appends_events() {
        let items = vec![cap(0, TICKS_PER_SECOND, "x")];
        let (out, n) = render(&items, "P", "S", "[Events]\nFormat: Layer, Start\n", true);
        assert_eq!(n, 1);
        assert!(out.trim_end().ends_with("x"));
    }

    #[test]
    fn empty_captions_can_be_skipped() {
        let items = vec![
            cap(0, TICKS_PER_SECOND, "  "),
            cap(TICKS_PER_SECOND, 2 * TICKS_PER_SECOND, "有字"),
        ];
        assert_eq!(render(&items, "P", "S", DEFAULT_TEMPLATE, true).1, 1);
        assert_eq!(render(&items, "P", "S", DEFAULT_TEMPLATE, false).1, 2);
    }
}
