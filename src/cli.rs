//! 无界面模式：`prsrt-cli <输出目录> <工程1.prproj> [工程2.prproj …] [选项]`
//!
//! 选项：
//!   --formats srt,ass   要导出的格式（默认两种都导）
//!   --suffix _v2        追加到文件名尾部的字符串
//!   --template 路径      指定 ASS 模板文件（默认用配置目录里保存的模板）
//!   --no-merged         不为多轨道序列额外输出合并稿
//!   -V, --version       打印版本号
//!
//! 只导出各序列中「已启用」的字幕轨道；某序列只导出一条轨道时文件名不带轨道名。
//! 某序列有多条轨道时，除逐轨输出外还会额外输出一份合并去重的 SRT。

use crate::app::{merged_base, normalize_suffix};
use crate::ass;
use crate::prproj;
use crate::srt;
use std::path::PathBuf;

fn usage() {
    println!(
        "用法：prsrt-cli <输出目录> <工程.prproj> [更多工程…] [--formats srt,ass] [--suffix _v2] [--template 模板.ass] [--no-merged]"
    );
    println!("版本：{}（编译日期版本号 YY.MM.DD）", crate::VERSION);
    println!("  -h, --help       显示本说明\n  -V, --version    显示版本号");
}

fn parse_formats(v: &str) -> (bool, bool) {
    let mut s = false;
    let mut a = false;
    for part in v.split([',', '+', ' ']) {
        match part.trim().to_ascii_lowercase().as_str() {
            "srt" => s = true,
            "ass" => a = true,
            _ => {}
        }
    }
    if !s && !a {
        (true, true)
    } else {
        (s, a)
    }
}

pub fn run(args: &[String]) -> i32 {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut out_dir: Option<PathBuf> = None;
    let mut write_srt = true;
    let mut write_ass = true;
    let mut suffix = String::new();
    let mut template: Option<String> = None;
    let mut merged_srt = true;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => {
                usage();
                return 0;
            }
            "-V" | "--version" => {
                println!("prsrt-cli {}", crate::version_tag());
                return 0;
            }
            "--formats" | "--format" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    let (s, as_) = parse_formats(v);
                    write_srt = s;
                    write_ass = as_;
                }
            }
            "--suffix" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    suffix = v.clone();
                }
            }
            "--template" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    match std::fs::read_to_string(v) {
                        Ok(t) => template = Some(t.trim_start_matches('\u{feff}').to_string()),
                        Err(e) => {
                            eprintln!("读取模板失败 {v}：{e}");
                            return 2;
                        }
                    }
                }
            }
            "--no-merged" => merged_srt = false,
            _ if a.starts_with("--") => {
                eprintln!("未知参数：{a}");
                usage();
                return 2;
            }
            _ => {
                if out_dir.is_none() {
                    out_dir = Some(PathBuf::from(a));
                } else {
                    files.push(PathBuf::from(a));
                }
            }
        }
        i += 1;
    }

    let Some(out_dir) = out_dir else {
        usage();
        return 2;
    };
    if files.is_empty() {
        usage();
        return 2;
    }
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("无法创建输出目录 {}：{e}", out_dir.display());
        return 2;
    }
    let template = template.unwrap_or_else(crate::load_template);
    let suffix = normalize_suffix(&suffix);

    let mut n_tasks = 0usize;
    let mut n_subs = 0usize;
    let mut failed = 0usize;

    for path in &files {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".into());
        let nodes = match prproj::list_sequences(path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("✗ {}：{e}", path.display());
                failed += 1;
                continue;
            }
        };
        let metas = prproj::flatten(&nodes);
        let uids: Vec<String> = metas.iter().map(|m| m.uid.clone()).collect();
        println!("{} → {} 个序列", stem, metas.len());
        let details = match prproj::analyze(path, &uids) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("✗ 分析 {} 失败：{e}", path.display());
                failed += 1;
                continue;
            }
        };

        for seq in details {
            let live: Vec<&prproj::TrackInfo> = seq
                .tracks
                .iter()
                .filter(|t| t.enabled && !t.items.is_empty())
                .collect();
            let with_track = live.len() > 1;

            for track in &live {
                let mut base = format!(
                    "{}_{}",
                    srt::sanitize_filename(&stem),
                    srt::sanitize_filename(&seq.name)
                );
                if with_track {
                    base.push('_');
                    base.push_str(&track.label());
                }
                base.push_str(&suffix);

                let mut subs = 0usize;
                let mut exts: Vec<&str> = Vec::new();
                let mut ok = true;

                if write_srt {
                    let (text, n) = srt::render(&track.items, true);
                    subs = subs.max(n);
                    let target = out_dir.join(format!("{base}.srt"));
                    match std::fs::write(&target, text.as_bytes()) {
                        Ok(_) => exts.push("srt"),
                        Err(e) => {
                            eprintln!("  ✗ 写入 {} 失败：{e}", target.display());
                            ok = false;
                            failed += 1;
                        }
                    }
                }
                if write_ass {
                    let (text, n) = ass::render(&track.items, &stem, &seq.name, &template, true);
                    subs = subs.max(n);
                    let target = out_dir.join(format!("{base}.ass"));
                    match std::fs::write(&target, text.as_bytes()) {
                        Ok(_) => exts.push("ass"),
                        Err(e) => {
                            eprintln!("  ✗ 写入 {} 失败：{e}", target.display());
                            ok = false;
                            failed += 1;
                        }
                    }
                }
                if ok {
                    println!(
                        "  ✓ {} · {} · {} 条 → {}.{}",
                        seq.name,
                        track.label(),
                        subs,
                        base,
                        exts.join("/")
                    );
                    n_tasks += 1;
                    n_subs += subs;
                }
            }

            // 多轨道合并稿：只出 SRT，按时间合并所有选中轨道，完全重复的只留一条
            if merged_srt && write_srt && live.len() > 1 {
                let merged = srt::merge(live.iter().map(|t| t.items.as_slice()));
                let (text, n) = srt::render(&merged, true);
                let base = merged_base(true, &stem, &seq.name, &suffix);
                let target = out_dir.join(format!("{base}.srt"));
                match std::fs::write(&target, text.as_bytes()) {
                    Ok(_) => {
                        println!("  ✓ {} · 合并 · {} 条 → {base}.srt", seq.name, n);
                        n_tasks += 1;
                        n_subs += n;
                    }
                    Err(e) => {
                        eprintln!("  ✗ 写入 {} 失败：{e}", target.display());
                        failed += 1;
                    }
                }
            }
        }
    }

    println!("完成：{n_tasks} 个任务 / {n_subs} 条字幕，失败 {failed}。");
    if failed > 0 {
        1
    } else {
        0
    }
}
