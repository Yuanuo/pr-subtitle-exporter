//! 命令行批量导出：prsrt-cli <输出目录> <工程.prproj> [更多工程…]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(pr_subtitle_exporter::cli::run(&args));
}
