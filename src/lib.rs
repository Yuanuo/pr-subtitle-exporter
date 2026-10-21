//! PR 字幕导出器：把 Premiere Pro 工程里的序列字幕导出为 SRT / ASS。

pub mod app;
pub mod ass;
pub mod cli;
pub mod prproj;
pub mod settings;
pub mod srt;

use std::path::PathBuf;

/// 编译期版本号，规则 `YY.MM.DD`。
///
/// 由 `build.rs` 在每次编译时按当天日期写入（可用环境变量 `PRSRT_VERSION` 手动覆盖）；
/// 没经过 build.rs 时退回 `dev`。
pub const VERSION: &str = match option_env!("PRSRT_VERSION") {
    Some(v) => v,
    None => "dev",
};

/// 带 `v` 前缀的版本标签，例如 `v26.10.21`，用于窗口标题 / 日志 / 命令行。
pub fn version_tag() -> String {
    format!("v{VERSION}")
}

/// 配置目录：%APPDATA%\PRSubtitleExporter（取不到时退回程序所在目录）。
pub fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.trim().is_empty() {
            return PathBuf::from(appdata).join("PRSubtitleExporter");
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn template_path() -> PathBuf {
    config_dir().join("ass_template.ass")
}

/// 读取 ASS 模板；不存在时写入默认模板并返回默认值。
pub fn load_template() -> String {
    let path = template_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim_start_matches('\u{feff}');
        if !text.trim().is_empty() {
            return text.to_string();
        }
    }
    let _ = std::fs::create_dir_all(config_dir());
    let _ = std::fs::write(&path, ass::DEFAULT_TEMPLATE);
    ass::DEFAULT_TEMPLATE.to_string()
}

pub fn save_template(text: &str) -> Result<(), String> {
    std::fs::create_dir_all(config_dir()).map_err(|e| format!("无法创建配置目录：{e}"))?;
    std::fs::write(template_path(), text).map_err(|e| format!("保存模板失败：{e}"))
}
