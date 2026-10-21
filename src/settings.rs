//! 用户偏好设置的持久化（退出时保存、启动时加载）。
//!
//! 存放位置：`%APPDATA%\PRSubtitleExporter\settings.ini`，纯 `key=value` 文本，
//! 可以直接用记事本改。

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub skip_empty: bool,
    pub include_project_in_name: bool,
    pub write_bom: bool,
    pub export_srt: bool,
    pub export_ass: bool,
    /// 一个序列勾选了多条轨道时，额外输出一份合并去重后的 SRT
    pub export_merged_srt: bool,
    pub suffix: String,
    /// None 表示「跟随工程所在目录」
    pub output_dir: Option<String>,
    /// 界面缩放（1.0 = 100%）
    pub zoom: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            skip_empty: true,
            include_project_in_name: true,
            write_bom: false,
            export_srt: true,
            export_ass: true,
            export_merged_srt: true,
            suffix: String::new(),
            output_dir: None,
            zoom: 1.1,
        }
    }
}

impl Settings {
    /// 修正非法组合（例如两种格式都没勾、缩放越界）。
    pub fn normalize(&mut self) {
        if !self.export_srt && !self.export_ass {
            self.export_srt = true;
            self.export_ass = true;
        }
        if !self.zoom.is_finite() {
            self.zoom = 1.0;
        }
        self.zoom = (self.zoom.clamp(0.6, 3.0) * 100.0).round() / 100.0;
        self.suffix = self.suffix.replace(['\r', '\n'], " ");
    }
}

pub fn path() -> PathBuf {
    crate::config_dir().join("settings.ini")
}

fn as_flag(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

fn parse_flag(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub fn load() -> Settings {
    let mut s = Settings::default();
    let Ok(text) = std::fs::read_to_string(path()) else {
        return s;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "skip_empty" => {
                if let Some(b) = parse_flag(value) {
                    s.skip_empty = b;
                }
            }
            "include_project_in_name" => {
                if let Some(b) = parse_flag(value) {
                    s.include_project_in_name = b;
                }
            }
            "write_bom" => {
                if let Some(b) = parse_flag(value) {
                    s.write_bom = b;
                }
            }
            "export_srt" => {
                if let Some(b) = parse_flag(value) {
                    s.export_srt = b;
                }
            }
            "export_ass" => {
                if let Some(b) = parse_flag(value) {
                    s.export_ass = b;
                }
            }
            "export_merged_srt" => {
                if let Some(b) = parse_flag(value) {
                    s.export_merged_srt = b;
                }
            }
            "suffix" => s.suffix = value.to_string(),
            "output_dir" => {
                s.output_dir = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            }
            "zoom" => {
                if let Ok(z) = value.parse::<f32>() {
                    s.zoom = z;
                }
            }
            _ => {}
        }
    }
    s.normalize();
    s
}

pub fn save(s: &Settings) -> Result<(), String> {
    let mut s = s.clone();
    s.normalize();
    std::fs::create_dir_all(crate::config_dir()).map_err(|e| format!("无法创建设置目录：{e}"))?;

    let mut out = String::from("# PR 字幕导出器设置（自动生成，可手动编辑）\n");
    out.push_str(&format!("skip_empty={}\n", as_flag(s.skip_empty)));
    out.push_str(&format!(
        "include_project_in_name={}\n",
        as_flag(s.include_project_in_name)
    ));
    out.push_str(&format!("write_bom={}\n", as_flag(s.write_bom)));
    out.push_str(&format!("export_srt={}\n", as_flag(s.export_srt)));
    out.push_str(&format!("export_ass={}\n", as_flag(s.export_ass)));
    out.push_str(&format!(
        "export_merged_srt={}\n",
        as_flag(s.export_merged_srt)
    ));
    out.push_str(&format!("suffix={}\n", s.suffix));
    out.push_str(&format!(
        "output_dir={}\n",
        s.output_dir.clone().unwrap_or_default()
    ));
    out.push_str(&format!("zoom={:.2}\n", s.zoom));

    std::fs::write(path(), out).map_err(|e| format!("保存设置失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k.trim() {
                "skip_empty" => s.skip_empty = parse_flag(v).unwrap_or(s.skip_empty),
                "export_ass" => s.export_ass = parse_flag(v).unwrap_or(s.export_ass),
                "export_merged_srt" => {
                    s.export_merged_srt = parse_flag(v).unwrap_or(s.export_merged_srt)
                }
                "suffix" => s.suffix = v.trim().to_string(),
                "zoom" => s.zoom = v.trim().parse().unwrap_or(s.zoom),
                "output_dir" => {
                    s.output_dir = if v.trim().is_empty() {
                        None
                    } else {
                        Some(v.trim().to_string())
                    }
                }
                _ => {}
            }
        }
        s.normalize();
        s
    }

    #[test]
    fn parses_values() {
        let s =
            round_trip("skip_empty=0\nexport_ass=false\nsuffix=_v2=甲\nzoom=1.3\noutput_dir=\n");
        assert!(!s.skip_empty);
        assert!(!s.export_ass);
        assert!(s.export_srt, "两种都没勾时应自动补一种");
        assert_eq!(s.suffix, "_v2=甲", "值里可以带等号");
        assert!((s.zoom - 1.3).abs() < 1e-6);
        assert_eq!(s.output_dir, None);
        assert!(s.export_merged_srt, "合并稿默认开启");
    }

    #[test]
    fn merged_srt_can_be_disabled() {
        let s = round_trip("export_merged_srt=0\n");
        assert!(!s.export_merged_srt);
    }

    #[test]
    fn normalize_clamps_zoom_and_keeps_one_format() {
        let mut s = Settings {
            zoom: 99.0,
            export_srt: false,
            export_ass: false,
            ..Default::default()
        };
        s.normalize();
        assert!(s.zoom <= 3.0);
        assert!(s.export_srt || s.export_ass);
    }
}
