//! 编译期注入版本号 + 把图标和版本信息编译进 exe（Windows 才有资源那一步）。
//!
//! 版本号规则为 `YY.MM.DD`，每次编译时取本机当前日期算出，
//! 通过 `cargo:rustc-env=PRSRT_VERSION` 传给程序，
//! 同时写进 exe 的资源段（右键属性 → 详细信息里能看到）。
//!
//! 为了让「改了代码重新编译」时版本号一定跟着刷新，
//! 这里给 src 下每个 .rs 都登记了 rerun-if-changed；
//! 也支持用环境变量 `PRSRT_VERSION` 手动指定（调试/复现版本时用）。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // 源码变了就重跑本脚本，否则同一天里编译出来的版本号可能是旧的
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    for f in collect_rs(&manifest().join("src")) {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-env-changed=PRSRT_VERSION");

    let version = version_from_date();
    println!("cargo:rustc-env=PRSRT_VERSION={version}");

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    compile_icon_and_versioninfo(&version);
}

fn manifest() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
}

/// 递归收集 src 下的 .rs 文件（用来登记 rerun-if-changed）。
fn collect_rs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(collect_rs(&p));
        } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
            out.push(p);
        }
    }
    out
}

/// 版本号：优先用环境变量覆盖，否则取本机当前日期的 YY.MM.DD。
fn version_from_date() -> String {
    if let Ok(v) = std::env::var("PRSRT_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    let (y, m, d) = local_ymd();
    format!("{:02}.{:02}.{:02}", y % 100, m, d)
}

/// 本机当前日期（年, 月, 日）。
#[cfg(windows)]
fn local_ymd() -> (i32, u32, u32) {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }
    let mut st = SystemTime::default();
    unsafe { GetLocalTime(&mut st) };
    if st.year >= 1970 {
        (st.year as i32, st.month as u32, st.day as u32)
    } else {
        utc_ymd()
    }
}

#[cfg(not(windows))]
fn local_ymd() -> (i32, u32, u32) {
    utc_ymd()
}

/// 没有本地时间接口时退回 UTC（构建机在东八区，只有半夜编译会差一天）。
fn utc_ymd() -> (i32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_from_days(secs.div_euclid(86_400))
}

/// 自 1970-01-01 起的天数 → (年, 月, 日)，Howard Hinnant 的 civil_from_days。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// 用 mingw 的 windres 把「图标 + 版本信息」编译成 COFF 对象交给链接器。
/// 找不到 windres 时只给警告，不影响编译。
fn compile_icon_and_versioninfo(version: &str) {
    let assets = manifest().join("assets");
    let rc = assets.join("app.rc");
    if !rc.exists() {
        return;
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let obj = out_dir.join("app_res.o");

    // 把 app.rc 里硬编码的版本号替换成本次编译的版本，生成到 OUT_DIR 再编译
    let gen_rc = out_dir.join("app_versioned.rc");
    let stamped = std::fs::read_to_string(&rc)
        .map(|s| stamp_version(&s, version))
        .unwrap_or_default();
    let rc_path = if !stamped.is_empty() && std::fs::write(&gen_rc, &stamped).is_ok() {
        gen_rc
    } else {
        rc
    };

    for bin in ["windres", "x86_64-w64-mingw32-windres", "windres.exe"] {
        let output = Command::new(bin)
            .current_dir(&assets)
            .arg("--codepage=65001")
            .arg("-i")
            .arg(&rc_path)
            .arg("-o")
            .arg(&obj)
            .arg("-O")
            .arg("coff")
            .output();
        match output {
            Ok(o) if o.status.success() => {
                println!("cargo:rustc-link-arg={}", obj.display());
                return;
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                println!("cargo:warning=windres 编译资源失败：{}", err.trim());
            }
            Err(_) => continue,
        }
    }
    println!("cargo:warning=未找到 windres，生成的 exe 将不带自定义图标");
}

/// `26.10.21` → 资源段要的 (26, 10, 21)。
fn parse_version(v: &str) -> (u16, u16, u16) {
    let mut it = v.split('.').map(|s| {
        s.chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u16>()
            .unwrap_or(0)
    });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// 替换 app.rc 里硬编码的版本号，其余内容原样保留。
fn stamp_version(source: &str, version: &str) -> String {
    let (y, m, d) = parse_version(version);
    let numeric = format!("{y},{m},{d},0");
    let mut out = String::with_capacity(source.len() + 64);
    for line in source.lines() {
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        let replacement = if t.starts_with("FILEVERSION ") {
            Some(format!("FILEVERSION {numeric}"))
        } else if t.starts_with("PRODUCTVERSION ") {
            Some(format!("PRODUCTVERSION {numeric}"))
        } else if t.starts_with("VALUE \"FileVersion\"") {
            Some(format!("VALUE \"FileVersion\", \"{version}\""))
        } else if t.starts_with("VALUE \"ProductVersion\"") {
            Some(format!("VALUE \"ProductVersion\", \"{version}\""))
        } else {
            None
        };
        match replacement {
            Some(s) => {
                out.push_str(indent);
                out.push_str(&s);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}
