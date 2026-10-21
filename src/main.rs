#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use pr_subtitle_exporter::app;
use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    // 支持把 .prproj 拖到 exe 图标上 / 用命令行带上工程路径直接打开
    let files: Vec<PathBuf> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();

    // 窗口标题带上编译期版本号（YY.MM.DD），方便用户区分新旧
    let title = format!("PR 字幕导出器 {}", pr_subtitle_exporter::version_tag());

    // 窗口/任务栏图标（exe 文件图标由 build.rs 里的 windres 资源提供）
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1240.0, 820.0])
        .with_min_inner_size([960.0, 640.0])
        .with_title(&title);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            let mut app = app::App::new(cc);
            if !files.is_empty() {
                app.add_paths(files.clone(), &cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}
