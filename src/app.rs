//! GUI：工程列表 -> 序列选择 -> 字幕轨道分析 -> 批量导出队列。

use crate::ass;
use crate::prproj::{self, Caption, ProjectNode};
use crate::settings::{self, Settings};
use crate::srt;
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

const GREEN: Color32 = Color32::from_rgb(34, 139, 87);
const GRAY: Color32 = Color32::from_rgb(140, 145, 152);
const RED: Color32 = Color32::from_rgb(196, 62, 62);
const BLUE: Color32 = Color32::from_rgb(45, 110, 200);
const AMBER: Color32 = Color32::from_rgb(176, 128, 24);

/// 合并稿在队列里的「轨道」列显示名
pub const MERGED_TRACK_LABEL: &str = "合并";
/// 合并稿追加到文件名尾部的标记（位于用户自定义后缀之前）
pub const MERGED_FILE_MARK: &str = "_合并";

// ---------------------------------------------------------------- 数据模型

struct Project {
    id: usize,
    path: PathBuf,
    name: String,
    state: PState,
    /// 工程面板的目录树（与 seqs 通过 uid 关联）
    tree: Vec<ProjectNode>,
    seqs: Vec<SeqEntry>,
}

enum PState {
    Loading,
    Ready,
    Error(String),
}

struct SeqEntry {
    uid: String,
    name: String,
    fps: f64,
    selected: bool,
    status: SStatus,
    tracks: Vec<TrackEntry>,
}

#[derive(PartialEq, Clone)]
enum SStatus {
    Idle,
    Analyzing,
    Done,
    Error(String),
}

struct TrackEntry {
    info: prproj::TrackInfo,
    selected: bool,
}

struct Task {
    id: usize,
    project: String,
    seq: String,
    /// 轨道名（C1/C2…）。文件名已按规则决定是否带它，这里只用于界面展示。
    track: String,
    count: usize,
    /// 输出文件主名（不含扩展名）
    base_path: PathBuf,
    write_srt: bool,
    write_ass: bool,
    status: TStatus,
    items: Vec<Caption>,
}

enum TStatus {
    Pending,
    Running,
    Done { files: usize, subs: usize },
    Failed(String),
}

struct Preview {
    title: String,
    srt_text: String,
    ass_text: String,
    show_ass: bool,
}

enum Msg {
    Scanned {
        id: usize,
        res: Result<Vec<ProjectNode>, String>,
    },
    Analyzed {
        id: usize,
        res: Result<Vec<prproj::SeqDetail>, String>,
    },
    Exported(Vec<(usize, Result<(usize, usize), String>)>),
}

enum Act {
    RemoveProject(usize),
    Analyze(usize),
    SelectAllSeqs(bool),
    Preview {
        title: String,
        project: String,
        seq: String,
        items: Vec<Caption>,
    },
    Enqueue,
    Export,
    ClearDone,
    ClearAll,
    /// 勾选变化后，若出现「已勾选但还没分析」的序列，安排一次自动分析
    ScheduleAnalyze(usize),
}

// ---------------------------------------------------------------- App

pub struct App {
    projects: Vec<Project>,
    tasks: Vec<Task>,
    output_dir: PathBuf,
    output_dir_touched: bool,
    include_project_in_name: bool,
    skip_empty: bool,
    write_bom: bool,
    export_srt: bool,
    export_ass: bool,
    /// 多轨道时额外输出一份合并去重 SRT
    export_merged_srt: bool,
    /// 追加到文件名尾部的自定义字符串
    suffix: String,
    ass_template: String,
    show_template_editor: bool,
    template_dirty: bool,
    log: Vec<String>,
    preview: Option<Preview>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    busy: usize,
    next_project_id: usize,
    next_task_id: usize,
    cjk_font: bool,
    /// 折叠的目录（key = "工程id:路径"）
    collapsed: std::collections::HashSet<String>,
    /// 待自动分析的工程 id
    pending_analyze: std::collections::HashSet<usize>,
    pending_since: Option<std::time::Instant>,
    /// 上次写盘的设置（用来判断是否有改动）
    saved_settings: Settings,
    settings_dirty_since: Option<std::time::Instant>,
    /// 当前界面缩放（同步自 egui，Ctrl+/- 也会改它）
    zoom: f32,
    /// 底栏「③ 导出」上一帧的实际高度（用来给队列区预留空间）
    export_bar_h: f32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cjk_font = install_fonts(&cc.egui_ctx);
        apply_style(&cc.egui_ctx);
        let (tx, rx) = channel();
        let loaded = settings::load();
        cc.egui_ctx.set_zoom_factor(loaded.zoom);
        let mut app = Self {
            projects: Vec::new(),
            tasks: Vec::new(),
            output_dir: loaded
                .output_dir
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(default_output_dir),
            output_dir_touched: loaded.output_dir.is_some(),
            include_project_in_name: loaded.include_project_in_name,
            skip_empty: loaded.skip_empty,
            write_bom: loaded.write_bom,
            export_srt: loaded.export_srt,
            export_ass: loaded.export_ass,
            export_merged_srt: loaded.export_merged_srt,
            suffix: loaded.suffix.clone(),
            ass_template: crate::load_template(),
            show_template_editor: false,
            template_dirty: false,
            log: Vec::new(),
            preview: None,
            tx,
            rx,
            busy: 0,
            next_project_id: 1,
            next_task_id: 1,
            cjk_font,
            collapsed: std::collections::HashSet::new(),
            pending_analyze: std::collections::HashSet::new(),
            pending_since: None,
            saved_settings: loaded,
            settings_dirty_since: None,
            zoom: cc.egui_ctx.zoom_factor(),
            export_bar_h: 0.0,
        };
        // 原来窗口顶部的标题改成第一行日志（不带「·」前缀，当横幅用）
        app.log.push(format!(
            "PR 字幕导出器 {} · Premiere 序列字幕 → SRT / ASS",
            crate::version_tag()
        ));
        app.log("就绪：把 .prproj 拖进窗口，或点左上角「＋ 添加工程」。");
        if !cjk_font {
            app.log("⚠ 未找到系统中文字体，界面中文可能显示异常。");
        }
        app
    }

    /// 收集当前界面上的设置值
    fn current_settings(&self) -> Settings {
        let mut s = Settings {
            skip_empty: self.skip_empty,
            include_project_in_name: self.include_project_in_name,
            write_bom: self.write_bom,
            export_srt: self.export_srt,
            export_ass: self.export_ass,
            export_merged_srt: self.export_merged_srt,
            suffix: self.suffix.clone(),
            output_dir: if self.output_dir_touched {
                Some(self.output_dir.display().to_string())
            } else {
                None
            },
            zoom: self.zoom,
        };
        s.normalize();
        s
    }

    /// 有改动时延迟 0.6 秒写盘，避免每帧都写文件
    fn persist_settings_if_needed(&mut self, ctx: &egui::Context) {
        let cur = self.current_settings();
        if cur == self.saved_settings {
            self.settings_dirty_since = None;
            return;
        }
        let now = std::time::Instant::now();
        let first = self.settings_dirty_since.get_or_insert(now);
        if now.duration_since(*first) >= std::time::Duration::from_millis(600) {
            match settings::save(&cur) {
                Ok(_) => {
                    self.saved_settings = cur;
                    self.settings_dirty_since = None;
                }
                Err(e) => {
                    self.log(e);
                    self.saved_settings = cur;
                    self.settings_dirty_since = None;
                }
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn log(&mut self, msg: impl Into<String>) {
        self.log.push(format!("· {}", msg.into()));
        if self.log.len() > 500 {
            self.log.remove(0);
        }
    }

    // ------------------------------------------------------------ 文件与任务

    pub fn add_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for path in paths {
            let is_pr = path
                .extension()
                .map(|e| e.eq_ignore_ascii_case("prproj"))
                .unwrap_or(false);
            if !is_pr {
                self.log(format!("已忽略（不是 .prproj 文件）：{}", path.display()));
                continue;
            }
            if self.projects.iter().any(|p| p.path == path) {
                self.log(format!("已存在于列表中：{}", file_name(&path)));
                continue;
            }
            if !self.output_dir_touched {
                let parent = path
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .map(|d| d.to_path_buf())
                    .or_else(|| std::env::current_dir().ok());
                if let Some(dir) = parent {
                    self.output_dir = dir;
                }
            }
            let id = self.next_project_id;
            self.next_project_id += 1;
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "未命名工程".into());
            self.projects.push(Project {
                id,
                path: path.clone(),
                name: name.clone(),
                state: PState::Loading,
                tree: Vec::new(),
                seqs: Vec::new(),
            });
            self.log(format!("正在解析工程：{}", path.display()));
            self.spawn_scan(id, path, ctx);
        }
    }

    fn spawn_scan(&mut self, id: usize, path: PathBuf, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.busy += 1;
        std::thread::spawn(move || {
            let res = prproj::list_sequences(&path);
            let _ = tx.send(Msg::Scanned { id, res });
            ctx.request_repaint();
        });
    }

    fn spawn_analyze(&mut self, id: usize, ctx: &egui::Context, only_missing: bool) {
        let Some(p) = self.projects.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if matches!(p.state, PState::Loading) {
            return;
        }
        // only_missing：只分析「已勾选但还没分析过」的序列（自动触发时用）
        let uids: Vec<String> = p
            .seqs
            .iter()
            .filter(|s| s.selected && (!only_missing || matches!(s.status, SStatus::Idle)))
            .map(|s| s.uid.clone())
            .collect();
        if uids.is_empty() {
            if !only_missing {
                self.log("请先勾选要分析的序列。");
            }
            return;
        }
        let path = p.path.clone();
        let name = p.name.clone();
        for s in p.seqs.iter_mut() {
            if s.selected && (!only_missing || uids.iter().any(|u| *u == s.uid)) {
                s.status = SStatus::Analyzing;
                s.tracks.clear();
            }
        }
        self.log(format!("正在分析《{}》的 {} 个序列…", name, uids.len()));
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.busy += 1;
        std::thread::spawn(move || {
            let res = prproj::analyze(&path, &uids);
            let _ = tx.send(Msg::Analyzed { id, res });
            ctx.request_repaint();
        });
    }

    fn enqueue(&mut self) {
        let include_proj = self.include_project_in_name;
        let skip_empty = self.skip_empty;
        let write_srt = self.export_srt;
        let write_ass = self.export_ass;
        // 合并稿只能是 SRT，所以 SRT 没勾时也就不生成
        let want_merged = self.export_merged_srt && write_srt;
        let suffix = normalize_suffix(&self.suffix);
        let mut out_dir = self.output_dir.clone();
        if out_dir.as_os_str().is_empty() {
            out_dir = default_output_dir();
        }
        let mut nid = self.next_task_id;
        let mut added = 0usize;
        let mut merged_added = 0usize;
        let mut skipped = 0usize;
        let mut empty = 0usize;
        let mut new_tasks: Vec<Task> = Vec::new();

        for p in self.projects.iter() {
            for s in p.seqs.iter() {
                if !s.selected {
                    continue;
                }
                // 该序列里真正会被导出的轨道：已勾选且有字幕条目
                let picked: Vec<&TrackEntry> = s
                    .tracks
                    .iter()
                    .filter(|t| t.selected && !t.info.items.is_empty())
                    .collect();
                // 只有 1 条时文件名不带轨道名，多条时才加，避免同名互相覆盖。
                let with_track = picked.len() > 1;

                for t in s.tracks.iter() {
                    if !t.selected {
                        continue;
                    }
                    if t.info.items.is_empty() {
                        empty += 1;
                        continue;
                    }
                    let label = t.info.label();
                    let dup = self
                        .tasks
                        .iter()
                        .any(|x| x.project == p.name && x.seq == s.name && x.track == label)
                        || new_tasks
                            .iter()
                            .any(|x| x.project == p.name && x.seq == s.name && x.track == label);
                    if dup {
                        skipped += 1;
                        continue;
                    }
                    let mut base = String::new();
                    if include_proj {
                        base.push_str(&srt::sanitize_filename(&p.name));
                        base.push('_');
                    }
                    base.push_str(&srt::sanitize_filename(&s.name));
                    if with_track {
                        base.push('_');
                        base.push_str(&label);
                    }
                    base.push_str(&suffix);

                    let mut stem = out_dir.join(&base);
                    let mut k = 2;
                    while new_tasks.iter().any(|x| x.base_path == stem)
                        || self.tasks.iter().any(|x| x.base_path == stem)
                    {
                        stem = out_dir.join(format!("{base} ({k})"));
                        k += 1;
                    }

                    let count = if skip_empty {
                        t.info.text_count()
                    } else {
                        t.info.items.len()
                    };
                    new_tasks.push(Task {
                        id: nid,
                        project: p.name.clone(),
                        seq: s.name.clone(),
                        track: label,
                        count,
                        base_path: stem,
                        write_srt,
                        write_ass,
                        status: TStatus::Pending,
                        items: t.info.items.clone(),
                    });
                    nid += 1;
                    added += 1;
                }

                // 一个序列勾了多条轨道时，额外给一份「合并稿」：
                // 所有选中轨道的条目排成一条时间线，同起止时间 + 同文本的只留一条。
                if want_merged && picked.len() > 1 {
                    let label = MERGED_TRACK_LABEL.to_string();
                    let dup = self
                        .tasks
                        .iter()
                        .any(|x| x.project == p.name && x.seq == s.name && x.track == label)
                        || new_tasks
                            .iter()
                            .any(|x| x.project == p.name && x.seq == s.name && x.track == label);
                    if dup {
                        skipped += 1;
                    } else {
                        let merged = srt::merge(picked.iter().map(|t| t.info.items.as_slice()));
                        let count = if skip_empty {
                            merged.iter().filter(|c| !c.text.trim().is_empty()).count()
                        } else {
                            merged.len()
                        };
                        let base = merged_base(include_proj, &p.name, &s.name, &suffix);

                        let mut stem = out_dir.join(&base);
                        let mut k = 2;
                        while new_tasks.iter().any(|x| x.base_path == stem)
                            || self.tasks.iter().any(|x| x.base_path == stem)
                        {
                            stem = out_dir.join(format!("{base} ({k})"));
                            k += 1;
                        }

                        new_tasks.push(Task {
                            id: nid,
                            project: p.name.clone(),
                            seq: s.name.clone(),
                            track: label,
                            count,
                            base_path: stem,
                            write_srt: true,
                            write_ass: false,
                            status: TStatus::Pending,
                            items: merged,
                        });
                        nid += 1;
                        added += 1;
                        merged_added += 1;
                    }
                }
            }
        }

        self.next_task_id = nid;
        self.tasks.extend(new_tasks);
        if added > 0 {
            let mut extra = String::new();
            if merged_added > 0 {
                extra = format!("，其中 {merged_added} 个为多轨合并稿（仅 SRT）");
            }
            self.log(format!(
                "已加入 {added} 个导出任务（{}）{extra}，点「开始导出」才会真正写文件。",
                format_list(write_srt, write_ass)
            ));
        }
        if skipped > 0 {
            self.log(format!("跳过 {skipped} 个已在队列中的任务。"));
        }
        if empty > 0 {
            self.log(format!("跳过 {empty} 条没有任何字幕条目的轨道。"));
        }
        if added == 0 && skipped == 0 && empty == 0 {
            self.log("没有已勾选的字幕轨道，请先在左侧勾选。");
        }
    }

    fn spawn_export(&mut self, ctx: &egui::Context) {
        let skip_empty = self.skip_empty;
        let bom = self.write_bom;
        let template = self.ass_template.clone();
        let mut payload: Vec<(usize, Vec<(PathBuf, String)>, usize)> = Vec::new();
        for t in self.tasks.iter_mut() {
            let ready =
                matches!(t.status, TStatus::Pending) || matches!(t.status, TStatus::Failed(_));
            if !ready {
                continue;
            }
            let mut files: Vec<(PathBuf, String)> = Vec::new();
            let mut subs = 0usize;
            if t.write_srt {
                let (mut text, n) = srt::render(&t.items, skip_empty);
                if bom {
                    text.insert(0, '\u{feff}');
                }
                subs = subs.max(n);
                files.push((with_ext(&t.base_path, "srt"), text));
            }
            if t.write_ass {
                let (text, n) = ass::render(&t.items, &t.project, &t.seq, &template, skip_empty);
                subs = subs.max(n);
                files.push((with_ext(&t.base_path, "ass"), text));
            }
            if files.is_empty() {
                continue;
            }
            t.status = TStatus::Running;
            payload.push((t.id, files, subs));
        }
        if payload.is_empty() {
            self.log("没有待导出的任务。");
            return;
        }
        let total = payload.len();
        self.log(format!(
            "开始导出 {total} 个任务 → {}",
            self.output_dir.display()
        ));
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.busy += 1;
        std::thread::spawn(move || {
            let mut results = Vec::new();
            for (id, files, subs) in payload {
                let mut written = 0usize;
                let mut err: Option<String> = None;
                for (path, text) in &files {
                    if let Some(dir) = path.parent() {
                        if let Err(e) = std::fs::create_dir_all(dir) {
                            err = Some(format!("无法创建目录：{e}"));
                            break;
                        }
                    }
                    if let Err(e) = std::fs::write(path, text.as_bytes()) {
                        err = Some(format!("写入 {} 失败：{e}", path.display()));
                        break;
                    }
                    written += 1;
                }
                match err {
                    Some(e) => results.push((id, Err(e))),
                    None => results.push((id, Ok((written, subs)))),
                }
            }
            let _ = tx.send(Msg::Exported(results));
            ctx.request_repaint();
        });
    }

    fn poll_messages(&mut self, ctx: &egui::Context) {
        let mut notes: Vec<String> = Vec::new();
        let mut auto_analyze: Vec<usize> = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match msg {
                Msg::Scanned { id, res } => {
                    let Some(p) = self.projects.iter_mut().find(|p| p.id == id) else {
                        continue;
                    };
                    match res {
                        Ok(tree) => {
                            let flat = prproj::flatten(&tree);
                            let n = flat.len();
                            let seqs = flat
                                .into_iter()
                                .map(|m| SeqEntry {
                                    uid: m.uid,
                                    name: m.name,
                                    fps: m.fps,
                                    // 载入后默认全选，随后立即自动分析
                                    selected: true,
                                    status: SStatus::Idle,
                                    tracks: Vec::new(),
                                })
                                .collect();
                            p.tree = tree;
                            p.seqs = seqs;
                            p.state = PState::Ready;
                            auto_analyze.push(p.id);
                            notes.push(format!(
                                "《{}》解析完成，共 {n} 个序列，开始分析字幕轨道…",
                                p.name
                            ));
                        }
                        Err(e) => {
                            p.state = PState::Error(e.clone());
                            notes.push(format!("《{}》解析失败：{e}", p.name));
                        }
                    }
                }
                Msg::Analyzed { id, res } => {
                    let Some(p) = self.projects.iter_mut().find(|p| p.id == id) else {
                        continue;
                    };
                    match res {
                        Ok(details) => {
                            let mut track_total = 0usize;
                            for d in details {
                                if let Some(s) = p.seqs.iter_mut().find(|s| s.uid == d.uid) {
                                    track_total += d.tracks.len();
                                    s.tracks = d
                                        .tracks
                                        .into_iter()
                                        .map(|t| TrackEntry {
                                            selected: t.enabled && !t.items.is_empty(),
                                            info: t,
                                        })
                                        .collect();
                                    s.status = SStatus::Done;
                                }
                            }
                            for s in p.seqs.iter_mut() {
                                if matches!(s.status, SStatus::Analyzing) {
                                    s.status = SStatus::Idle;
                                }
                            }
                            notes.push(format!(
                                "《{}》分析完成，找到 {track_total} 条字幕轨道。",
                                p.name
                            ));
                        }
                        Err(e) => {
                            for s in p.seqs.iter_mut() {
                                if matches!(s.status, SStatus::Analyzing) {
                                    s.status = SStatus::Error(e.clone());
                                }
                            }
                            notes.push(format!("《{}》分析失败：{e}", p.name));
                        }
                    }
                }
                Msg::Exported(results) => {
                    let mut ok_tasks = 0usize;
                    let mut ok_files = 0usize;
                    let mut ok_subs = 0usize;
                    let mut errs = 0usize;
                    for (id, res) in results {
                        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                            match res {
                                Ok((files, subs)) => {
                                    t.status = TStatus::Done { files, subs };
                                    ok_tasks += 1;
                                    ok_files += files;
                                    ok_subs += subs;
                                }
                                Err(e) => {
                                    t.status = TStatus::Failed(e);
                                    errs += 1;
                                }
                            }
                        }
                    }
                    notes.push(format!(
                        "导出完成：{ok_tasks} 个任务 / {ok_files} 个文件 / {ok_subs} 条字幕{}。",
                        if errs > 0 {
                            format!("，{errs} 个失败")
                        } else {
                            String::new()
                        }
                    ));
                }
            }
        }
        for n in notes {
            self.log(n);
        }
        for id in auto_analyze {
            self.spawn_analyze(id, ctx, true);
        }
    }

    fn apply(&mut self, ctx: &egui::Context, act: Act) {
        match act {
            Act::RemoveProject(id) => {
                if let Some(pos) = self.projects.iter().position(|p| p.id == id) {
                    let p = self.projects.remove(pos);
                    self.log(format!("已从列表移除《{}》。", p.name));
                }
            }
            Act::Analyze(id) => self.spawn_analyze(id, ctx, false),
            Act::SelectAllSeqs(v) => {
                for p in self.projects.iter_mut() {
                    for s in p.seqs.iter_mut() {
                        s.selected = v;
                    }
                }
            }
            Act::Preview {
                title,
                project,
                seq,
                items,
            } => {
                let (srt_text, n) = srt::render(&items, self.skip_empty);
                let (ass_text, _) =
                    ass::render(&items, &project, &seq, &self.ass_template, self.skip_empty);
                self.preview = Some(Preview {
                    title: format!("{title} · {n} 条"),
                    srt_text,
                    ass_text,
                    show_ass: false,
                });
            }
            Act::Enqueue => self.enqueue(),
            Act::Export => self.spawn_export(ctx),
            Act::ClearDone => {
                self.tasks
                    .retain(|t| !matches!(t.status, TStatus::Done { .. }));
            }
            Act::ClearAll => self.tasks.clear(),
            Act::ScheduleAnalyze(id) => {
                let need = self
                    .projects
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| {
                        p.seqs
                            .iter()
                            .any(|s| s.selected && matches!(s.status, SStatus::Idle))
                    })
                    .unwrap_or(false);
                if need {
                    self.pending_analyze.insert(id);
                    self.pending_since = Some(std::time::Instant::now());
                }
            }
        }
    }
}

// ---------------------------------------------------------------- UI

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_messages(ctx);
        // egui 内置的 Ctrl + +/-/0 也会改缩放，这里同步回来
        self.zoom = ctx.zoom_factor();

        // 拖放文件
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped, ctx);
        }

        let mut acts: Vec<Act> = Vec::new();

        // 快捷键：F5 分析所选序列 / F6 加入队列 / F7 开始导出 / Ctrl+O 添加工程
        let (hot_add, hot_analyze, hot_enqueue, hot_export) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::O) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::F5),
                i.key_pressed(egui::Key::F6),
                i.key_pressed(egui::Key::F7),
            )
        });
        if hot_add {
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Premiere 工程 (*.prproj)", &["prproj"])
                .pick_files()
            {
                self.add_paths(files, ctx);
            }
        }
        if hot_analyze {
            let ids: Vec<usize> = self
                .projects
                .iter()
                .filter(|p| matches!(p.state, PState::Ready) && p.seqs.iter().any(|s| s.selected))
                .map(|p| p.id)
                .collect();
            for id in ids {
                self.spawn_analyze(id, ctx, false);
            }
        }
        if hot_enqueue {
            self.enqueue();
        }
        if hot_export {
            self.spawn_export(ctx);
        }

        // ---- 顶部工具栏：只放界面缩放与状态；工程的添加/清空归到左侧 ① 导入
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("界面字号").weak().size(11.0));
                if ui
                    .small_button("A−")
                    .on_hover_text("缩小界面（Ctrl + -）")
                    .clicked()
                {
                    egui::gui_zoom::zoom_out(ctx);
                }
                if ui
                    .small_button(format!("{:.0}%", self.zoom * 100.0))
                    .on_hover_text("点击恢复 100%（Ctrl + 0）")
                    .clicked()
                {
                    ctx.set_zoom_factor(1.0);
                }
                if ui
                    .small_button("A+")
                    .on_hover_text("放大界面（Ctrl + +）")
                    .clicked()
                {
                    egui::gui_zoom::zoom_in(ctx);
                }
                ui.separator();
                if !self.cjk_font {
                    ui.colored_label(RED, RichText::new("⚠ 未找到中文字体").size(12.0));
                }
                ui.label(
                    RichText::new("F5 分析 · F6 入队 · F7 导出")
                        .weak()
                        .size(11.0),
                );
                ui.add_space(6.0);
                let hint = if self.projects.is_empty() {
                    "支持把多个 .prproj 直接拖进窗口".to_string()
                } else {
                    format!(
                        "已载入 {} 个工程 · {} 个导出任务",
                        self.projects.len(),
                        self.tasks.len()
                    )
                };
                ui.label(RichText::new(hint).weak().size(12.0));
            });
            ui.add_space(6.0);
        });

        // ---- 底部日志
        egui::TopBottomPanel::bottom("log_panel")
            .resizable(true)
            .default_height(120.0)
            .min_height(64.0)
            .show(ctx, |ui| {
                let mut clear = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("日志").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("清空日志").clicked() {
                            clear = true;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.label(RichText::new(line).monospace().size(12.0));
                        }
                    });
                if clear {
                    self.log.clear();
                }
            });

        // ---- 左侧：工程 / 序列 / 轨道
        // 侧栏宽度按窗口自适应，保证中央的队列区始终有足够空间
        let win_w = ctx.screen_rect().width();
        let side_max = (win_w * 0.52).clamp(320.0, 720.0);
        egui::SidePanel::left("projects")
            .resizable(true)
            .default_width((win_w * 0.40).clamp(340.0, 520.0))
            .width_range(320.0..=side_max)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("① 导入 · 工程 / 序列 / 字幕轨道").strong());
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("＋ 添加工程")
                        .on_hover_text("选择 .prproj 文件（也可以直接把文件拖进窗口）")
                        .clicked()
                    {
                        if let Some(files) = rfd::FileDialog::new()
                            .add_filter("Premiere 工程 (*.prproj)", &["prproj"])
                            .pick_files()
                        {
                            self.add_paths(files, ctx);
                        }
                    }
                    if ui
                        .button("清空工程")
                        .on_hover_text("清空已载入的工程列表（不会动磁盘上的文件）")
                        .clicked()
                    {
                        self.projects.clear();
                        self.log("已清空工程列表。");
                    }
                    if ui.button("全选序列").clicked() {
                        acts.push(Act::SelectAllSeqs(true));
                    }
                    if ui.button("全不选").clicked() {
                        acts.push(Act::SelectAllSeqs(false));
                    }
                });
                ui.label(
                    RichText::new(
                        "Shift+点击：目录/序列 = 同一层级整批切换；字幕轨道 = 本工程同名轨道一起切换",
                    )
                    .weak()
                    .size(11.5),
                );
                ui.separator();

                if self.projects.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("把 .prproj 拖到这里").weak().size(16.0));
                        ui.label(RichText::new("或点上面的「＋ 添加工程」").weak().size(12.0));
                    });
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let collapsed = &mut self.collapsed;
                        for p in self.projects.iter_mut() {
                            project_card(ui, p, &mut acts, collapsed);
                            ui.add_space(8.0);
                        }
                        ui.add_space(6.0);
                    });
            });

        // ---- 中央：三段式 —— 上「② 加入队列」 / 中「队列」 / 下「③ 导出」
        // 三段都画在同一个 ui 里（下面用底对齐子布局），宽度天然一致。
        egui::CentralPanel::default().show(ctx, |ui| {
            let pending = self
                .tasks
                .iter()
                .filter(|t| {
                    matches!(t.status, TStatus::Pending) || matches!(t.status, TStatus::Failed(_))
                })
                .count();
            let done = self
                .tasks
                .iter()
                .filter(|t| matches!(t.status, TStatus::Done { .. }))
                .count();
            let total_items: usize = self
                .tasks
                .iter()
                .filter_map(|t| match t.status {
                    TStatus::Done { subs, .. } => Some(subs),
                    _ => None,
                })
                .sum();

            // ==== 上：② 队列 —— 这些选项决定队列里出哪些任务、文件叫什么
            ui.add_space(6.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("② 队列").strong())
                        .on_hover_text("这一块的选项决定队列里有哪些任务、文件叫什么名字");
                    let selected_tracks = self
                        .projects
                        .iter()
                        .flat_map(|p| p.seqs.iter())
                        .filter(|s| s.selected)
                        .flat_map(|s| s.tracks.iter())
                        .filter(|t| t.selected)
                        .count();
                    if ui
                        .add_enabled(
                            selected_tracks > 0,
                            egui::Button::new(format!(
                                "＋ 把已勾选轨道加入队列（{selected_tracks}）"
                            )),
                        )
                        .on_hover_text(
                            "勾了多条轨道的序列，还会额外生成 1 个「合并」任务：\
                             各轨道按时间合并、完全重复的条目只留一条（仅 SRT）",
                        )
                        .clicked()
                    {
                        acts.push(Act::Enqueue);
                    }
                    if ui
                        .add_enabled(!self.tasks.is_empty(), egui::Button::new("清除已完成"))
                        .on_hover_text("只从队列里移除已成功导出的任务")
                        .clicked()
                    {
                        acts.push(Act::ClearDone);
                    }
                    if ui
                        .add_enabled(!self.tasks.is_empty(), egui::Button::new("清空队列"))
                        .on_hover_text("移除队列里的全部任务（不影响已经写出的文件）")
                        .clicked()
                    {
                        acts.push(Act::ClearAll);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("格式").weak().size(12.0));
                    ui.checkbox(&mut self.export_srt, "SRT")
                        .on_hover_text("SubRip 字幕，兼容性最好");
                    ui.checkbox(&mut self.export_ass, "ASS")
                        .on_hover_text("带样式的字幕，样式取自 ASS 模板");
                    ui.add_enabled(
                        self.export_srt,
                        egui::Checkbox::new(&mut self.export_merged_srt, "多轨合并 SRT"),
                    )
                    .on_hover_text(
                        "同一个序列勾了多条轨道时，额外导出一份合并稿：\
                         各轨道按时间排成一条线，起止时间与文本都相同的条目只留一条（只出 SRT）",
                    );
                    if !self.export_srt && !self.export_ass {
                        ui.colored_label(RED, RichText::new("至少选一种").size(11.5));
                    }
                    ui.separator();
                    ui.checkbox(&mut self.include_project_in_name, "文件名带工程名")
                        .on_hover_text("关掉则只按序列名命名");
                    ui.label(RichText::new("后缀").weak().size(12.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.suffix)
                            .desired_width(110.0)
                            .hint_text("留空 / 如 _v2"),
                    )
                    .on_hover_text("追加到文件名尾部；不以 _ - . 开头时会自动补一个下划线");
                });
            });

            // ==== 中：队列（高度 = 剩余高度 - 底栏预留）
            ui.add_space(6.0);
            let rest_h = ui.available_height();
            let bar_h = if self.export_bar_h > 0.0 {
                self.export_bar_h
            } else {
                100.0
            };
            let queue_h = (rest_h - bar_h - 6.0).max(80.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(queue_h - 12.0);
                ui.horizontal_wrapped(|ui| {
                    // 标题已经在 ② 里了，这里只放统计，避免两个「队列」上下重复
                    ui.label(
                        RichText::new(format!(
                            "{} 个任务 · 待导出 {} · 已完成 {}/{} 条字幕",
                            self.tasks.len(),
                            pending,
                            done,
                            total_items
                        ))
                        .weak()
                        .size(12.0),
                    );
                });
                ui.add_space(4.0);
                let list_h = (queue_h - 52.0).max(40.0);
                egui::ScrollArea::both()
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.tasks.is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("队列还是空的").weak().size(15.0));
                                ui.label(
                                    RichText::new(
                                        "在左侧勾选序列和轨道 → 点上方「加入队列」→ 再点下方「开始导出」",
                                    )
                                    .weak()
                                    .size(12.0),
                                );
                            });
                        } else {
                            egui::Grid::new("queue")
                                .num_columns(7)
                                .striped(true)
                                .spacing([16.0, 6.0])
                                .show(ui, |ui| {
                                    header(ui, "工程");
                                    header(ui, "序列");
                                    header(ui, "轨道");
                                    header(ui, "条数");
                                    header(ui, "格式");
                                    header(ui, "输出文件");
                                    header(ui, "状态");
                                    ui.end_row();

                                    for t in self.tasks.iter() {
                                        let merged = t.track == MERGED_TRACK_LABEL;
                                        ui.label(RichText::new(&t.project).size(12.5));
                                        ui.label(RichText::new(&t.seq).size(12.5));
                                        if merged {
                                            ui.label(
                                                RichText::new(&t.track).color(BLUE).size(12.5),
                                            )
                                            .on_hover_text("各勾选轨道合并去重后的字幕稿");
                                        } else {
                                            ui.label(
                                                RichText::new(&t.track).monospace().size(12.5),
                                            );
                                        }
                                        ui.label(RichText::new(t.count.to_string()).size(12.5));
                                        ui.label(
                                            RichText::new(format_list(t.write_srt, t.write_ass))
                                                .size(11.5)
                                                .monospace(),
                                        );
                                        let fname = t
                                            .base_path
                                            .file_name()
                                            .map(|s| s.to_string_lossy().to_string())
                                            .unwrap_or_default();
                                        ui.label(RichText::new(fname).size(12.0).monospace())
                                            .on_hover_text(t.base_path.display().to_string());
                                        match &t.status {
                                            TStatus::Pending => ui.label(
                                                RichText::new("待导出").color(AMBER).size(12.0),
                                            ),
                                            TStatus::Running => ui.label(
                                                RichText::new("导出中…").color(BLUE).size(12.0),
                                            ),
                                            TStatus::Done { files, subs } => ui
                                                .label(
                                                    RichText::new(format!("完成 · {subs} 条"))
                                                        .color(GREEN)
                                                        .size(12.0),
                                                )
                                                .on_hover_text(format!("已写出 {files} 个文件")),
                                            TStatus::Failed(e) => ui
                                                .label(RichText::new("失败").color(RED).size(12.0))
                                                .on_hover_text(e),
                                        };
                                        ui.end_row();
                                    }
                                });
                        }
                    });
            });

            // ==== 下：③ 导出 —— 贴在这一栏的最下面
            ui.add_space(6.0);
            let bar = egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("③ 导出").strong())
                        .on_hover_text("输出目录与内容处理选项在写文件时才生效");
                    // 主操作按钮：加宽 + 蓝底白字，让它在底栏里最显眼
                    let can_export = pending > 0;
                    let label = RichText::new("开始导出").strong().size(15.0);
                    let label = if can_export {
                        label.color(Color32::WHITE)
                    } else {
                        label
                    };
                    let mut export_btn = egui::Button::new(label).min_size(egui::vec2(170.0, 30.0));
                    if can_export {
                        export_btn = export_btn
                            .fill(BLUE)
                            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(28, 78, 150)));
                    }
                    if ui
                        .add_enabled(can_export, export_btn)
                        .on_hover_text(format!("只写出队列中待导出的 {pending} 个任务（F7）"))
                        .clicked()
                    {
                        acts.push(Act::Export);
                    }
                    if self.busy > 0 {
                        ui.spinner();
                        ui.label(RichText::new("处理中…").weak().size(12.0));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("输出目录").weak().size(12.0));
                    let full = self.output_dir.display().to_string();
                    let shown = if self.output_dir_touched {
                        tail_path(&full, 30)
                    } else {
                        format!("（自动：{}）", tail_path(&full, 24))
                    };
                    ui.label(RichText::new(shown).monospace().size(11.5))
                        .on_hover_text(self.output_dir.display().to_string());
                    if ui.small_button("选择…").clicked() {
                        let start = self.output_dir.clone();
                        if let Some(dir) = rfd::FileDialog::new().set_directory(&start).pick_folder() {
                            self.output_dir = dir;
                            self.output_dir_touched = true;
                        }
                    }
                    if ui.small_button("打开").clicked() {
                        let dir = self.output_dir.clone();
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.skip_empty, "跳过空字幕")
                        .on_hover_text("忽略没有任何文字的条目（空字幕没有意义）");
                    ui.checkbox(&mut self.write_bom, "SRT 写 BOM").on_hover_text(
                        "让 SRT 以 UTF-8 BOM 开头；部分老旧播放器/剪辑软件识别中文需要 BOM",
                    );
                    if ui
                        .button("ASS 模板…")
                        .on_hover_text("编辑、保存 ASS 模板（改动后需点保存）")
                        .clicked()
                    {
                        self.show_template_editor = true;
                    }
                    if self.template_dirty {
                        ui.label(RichText::new("模板有未保存的修改").color(AMBER).size(11.0));
                    }
                });
            });
            // 记下底栏实际高度，下一帧按它给队列留位
            self.export_bar_h = bar.response.rect.height();
        });

        // ---- ASS 模板编辑窗口
        if self.show_template_editor {
            let mut open = true;
            let mut save = false;
            let mut reset = false;
            let mut load_file = false;
            let mut save_as = false;
            egui::Window::new("ASS 模板")
                .open(&mut open)
                .default_size([880.0, 680.0])
                .resizable(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button("保存")
                            .on_hover_text("保存到配置目录，之后导出都用这份模板")
                            .clicked()
                        {
                            save = true;
                        }
                        if ui.button("恢复默认").clicked() {
                            reset = true;
                        }
                        if ui.button("从文件载入…").clicked() {
                            load_file = true;
                        }
                        if ui.button("另存为…").clicked() {
                            save_as = true;
                        }
                        if self.template_dirty {
                            ui.label(RichText::new("未保存").color(AMBER).size(11.0));
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "{{EVENTS}} 处插入全部字幕行；也可用 {{TITLE}} / {{PROJECT}} / {{SEQUENCE}}。\
                                 字幕行的样式名自动取 [V4+ Styles] 里第一条样式的名字。",
                            )
                            .weak()
                            .size(11.5),
                        )
                        .wrap(),
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let resp = ui.add(
                                egui::TextEdit::multiline(&mut self.ass_template)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(34)
                                    .font(egui::TextStyle::Monospace),
                            );
                            if resp.changed() {
                                self.template_dirty = true;
                            }
                        });
                    ui.separator();
                    ui.label(
                        RichText::new(format!("模板文件：{}", crate::template_path().display()))
                            .weak()
                            .size(11.0),
                    );
                });

            if reset {
                self.ass_template = ass::DEFAULT_TEMPLATE.to_string();
                self.template_dirty = true;
            }
            if load_file {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ASS 模板", &["ass", "txt", "ssa"])
                    .pick_file()
                {
                    match std::fs::read_to_string(&path) {
                        Ok(t) => {
                            self.ass_template = t.trim_start_matches('\u{feff}').to_string();
                            self.template_dirty = true;
                        }
                        Err(e) => self.log(format!("读取模板失败：{e}")),
                    }
                }
            }
            if save_as {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ASS 模板", &["ass"])
                    .set_file_name("ass_template.ass")
                    .save_file()
                {
                    match std::fs::write(&path, self.ass_template.as_bytes()) {
                        Ok(_) => self.log(format!("模板已另存为：{}", path.display())),
                        Err(e) => self.log(format!("另存模板失败：{e}")),
                    }
                }
            }
            if save {
                match crate::save_template(&self.ass_template) {
                    Ok(_) => {
                        self.template_dirty = false;
                        self.log(format!(
                            "ASS 模板已保存：{}",
                            crate::template_path().display()
                        ));
                    }
                    Err(e) => self.log(e),
                }
            }
            if !open {
                self.show_template_editor = false;
            }
        }

        // ---- 预览窗口
        if self.preview.is_some() {
            let mut open = true;
            let mut copy_text: Option<String> = None;
            let mut switch: Option<bool> = None;
            {
                let p = self.preview.as_mut().unwrap();
                egui::Window::new(format!("预览 · {}", p.title))
                    .open(&mut open)
                    .default_size([780.0, 560.0])
                    .resizable(true)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("格式：");
                            if ui.radio(!p.show_ass, "SRT").clicked() {
                                switch = Some(false);
                            }
                            if ui.radio(p.show_ass, "ASS").clicked() {
                                switch = Some(true);
                            }
                            ui.separator();
                            if ui.button("复制全部").clicked() {
                                copy_text = Some(if p.show_ass {
                                    p.ass_text.clone()
                                } else {
                                    p.srt_text.clone()
                                });
                            }
                            ui.label(RichText::new("（只显示前 400 行）").weak().size(11.5));
                        });
                        ui.separator();
                        let body = if p.show_ass { &p.ass_text } else { &p.srt_text };
                        let text = truncate_lines(body, 400);
                        egui::ScrollArea::both()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(text).monospace().size(12.5))
                                        .wrap(),
                                );
                            });
                    });
            }
            if let Some(v) = switch {
                if let Some(p) = self.preview.as_mut() {
                    p.show_ass = v;
                }
            }
            if let Some(t) = copy_text {
                ctx.copy_text(t);
            }
            if !open {
                self.preview = None;
            }
        }

        for a in acts {
            self.apply(ctx, a);
        }

        // 勾选变化后延迟一小会儿再解析，避免连续点击触发多次全量解析
        if !self.pending_analyze.is_empty() {
            let due = self
                .pending_since
                .map(|t| t.elapsed() >= std::time::Duration::from_millis(350))
                .unwrap_or(true);
            if due && self.busy == 0 {
                let ids: Vec<usize> = self.pending_analyze.drain().collect();
                self.pending_since = None;
                for id in ids {
                    self.spawn_analyze(id, ctx, true);
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        self.persist_settings_if_needed(ctx);

        if self.busy > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let cur = self.current_settings();
        if cur != self.saved_settings {
            let _ = settings::save(&cur);
        }
    }
}

fn header(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).weak().size(12.0).strong());
}

struct TreeCtx<'a> {
    project_id: usize,
    project_name: &'a str,
    collapsed: &'a mut std::collections::HashSet<String>,
    acts: &'a mut Vec<Act>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tri {
    None,
    Partial,
    All,
}

enum FolderClick {
    /// 只改这个目录的子树
    Children(bool),
    /// 改同一层级的所有目录/序列（每个目录仍含自己的子树）
    Level(bool),
}

fn project_card(
    ui: &mut egui::Ui,
    p: &mut Project,
    acts: &mut Vec<Act>,
    collapsed: &mut std::collections::HashSet<String>,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(&p.name).strong().size(14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("从列表移除").clicked() {
                    acts.push(Act::RemoveProject(p.id));
                }
            });
        });
        ui.label(
            RichText::new(shorten(&p.path.display().to_string(), 60))
                .weak()
                .size(10.5),
        );

        match &p.state {
            PState::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("正在解析工程…").weak().size(12.0));
                });
            }
            PState::Error(e) => {
                ui.colored_label(RED, RichText::new(format!("解析失败：{e}")).size(12.0));
            }
            PState::Ready => {
                if p.seqs.is_empty() {
                    ui.label(RichText::new("该工程没有序列").weak().size(12.0));
                    return;
                }
                let total = p.seqs.len();
                let n_sel = p.seqs.iter().filter(|s| s.selected).count();
                let busy_any = p
                    .seqs
                    .iter()
                    .any(|s| matches!(s.status, SStatus::Analyzing));
                ui.horizontal(|ui| {
                    if ui.small_button("全选").clicked() {
                        for s in p.seqs.iter_mut() {
                            s.selected = true;
                        }
                        acts.push(Act::ScheduleAnalyze(p.id));
                    }
                    if ui.small_button("全不选").clicked() {
                        for s in p.seqs.iter_mut() {
                            s.selected = false;
                        }
                    }
                    let text = if busy_any {
                        "重新分析".to_string()
                    } else {
                        "分析所选".to_string()
                    };
                    if ui
                        .add_enabled(n_sel > 0 && !busy_any, egui::Button::new(text).small())
                        .on_hover_text("重新解析所选序列的字幕轨道")
                        .clicked()
                    {
                        acts.push(Act::Analyze(p.id));
                    }
                    ui.label(
                        RichText::new(format!(
                            "{n_sel}/{total} 个序列 · Shift+点击 = 同层级/同名轨道批量切换"
                        ))
                        .weak()
                        .size(10.5),
                    );
                });
                ui.add_space(2.0);

                let tree = &p.tree;
                let seqs = &mut p.seqs;
                let mut ctx = TreeCtx {
                    project_id: p.id,
                    project_name: &p.name,
                    collapsed,
                    acts,
                };
                render_nodes(ui, tree, seqs, 0, "", &mut ctx);
            }
        }
    });
}

fn render_nodes(
    ui: &mut egui::Ui,
    nodes: &[ProjectNode],
    seqs: &mut Vec<SeqEntry>,
    depth: usize,
    path: &str,
    ctx: &mut TreeCtx,
) {
    for node in nodes {
        match node {
            ProjectNode::Folder { name, children } => {
                let (total, sel) = folder_stats(children, seqs);
                // 素材箱里如果没有序列（纯媒体素材箱），界面上不展示
                if total == 0 {
                    continue;
                }
                let node_path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}/{name}")
                };
                let key = format!("{}:{}", ctx.project_id, node_path);
                let open = !ctx.collapsed.contains(&key);
                let tri = if total == 0 || sel == 0 {
                    Tri::None
                } else if sel == total {
                    Tri::All
                } else {
                    Tri::Partial
                };

                let mut toggle = false;
                let mut click: Option<FolderClick> = None;
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 14.0);
                    if collapse_arrow(ui, open).clicked() {
                        toggle = true;
                    }
                    if tri_checkbox(ui, tri)
                        .on_hover_text(
                            "勾选/取消该目录下全部序列；按住 Shift 则为「同一层级」的\
                             所有目录和序列一起勾选/取消（每个目录仍连同自己的子级）",
                        )
                        .clicked()
                    {
                        let target = tri != Tri::All;
                        let shift = ui.input(|i| i.modifiers.shift);
                        click = Some(if shift {
                            FolderClick::Level(target)
                        } else {
                            FolderClick::Children(target)
                        });
                    }
                    ui.label(RichText::new(name).strong().size(12.5));
                    ui.label(RichText::new(format!("{sel}/{total}")).weak().size(10.5));
                });

                if toggle {
                    if open {
                        ctx.collapsed.insert(key.clone());
                    } else {
                        ctx.collapsed.remove(&key);
                    }
                }
                match click {
                    Some(FolderClick::Children(v)) => {
                        set_subtree_selected(children, seqs, v);
                        ctx.acts.push(Act::ScheduleAnalyze(ctx.project_id));
                    }
                    // Shift：只作用于同一层级的兄弟目录/序列（含自己），
                    // 其中目录依然连同自己的整棵子树一起改。
                    Some(FolderClick::Level(v)) => {
                        set_level_selected(nodes, seqs, v);
                        ctx.acts.push(Act::ScheduleAnalyze(ctx.project_id));
                    }
                    None => {}
                }

                if open {
                    render_nodes(ui, children, seqs, depth + 1, &node_path, ctx);
                }
            }
            ProjectNode::Seq(meta) => {
                let Some(pos) = seqs.iter().position(|s| s.uid == meta.uid) else {
                    continue;
                };
                let mut new_state = seqs[pos].selected;
                let mut cascade: Option<bool> = None;
                let busy = matches!(seqs[pos].status, SStatus::Analyzing);

                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 14.0 + 22.0);
                    let resp = ui
                        .add_enabled(!busy, egui::Checkbox::new(&mut new_state, ""))
                        .on_hover_text("按住 Shift 点击 = 同一层级的序列/目录一起勾选/取消");
                    if resp.clicked() && ui.input(|i| i.modifiers.shift) {
                        cascade = Some(new_state);
                    }
                    let s = &seqs[pos];
                    let title = RichText::new(&s.name).size(13.0);
                    ui.label(if s.selected { title } else { title.weak() });
                    if s.fps > 0.0 {
                        ui.label(RichText::new(format!("{:.2} fps", s.fps)).weak().size(10.5));
                    }
                    match &s.status {
                        SStatus::Idle => {
                            ui.label(RichText::new("未分析").weak().size(10.5));
                        }
                        SStatus::Analyzing => {
                            ui.spinner();
                        }
                        SStatus::Done => {
                            if s.tracks.is_empty() {
                                ui.label(RichText::new("无字幕轨道").color(AMBER).size(10.5));
                            } else {
                                ui.label(RichText::new("已分析").color(GREEN).size(10.5));
                            }
                        }
                        SStatus::Error(e) => {
                            ui.label(RichText::new("分析失败").color(RED).size(10.5))
                                .on_hover_text(e);
                        }
                    }
                });

                if let Some(v) = cascade {
                    // Shift：同一层级的兄弟目录/序列一起改（目录连同子树）
                    set_level_selected(nodes, seqs, v);
                    ctx.acts.push(Act::ScheduleAnalyze(ctx.project_id));
                } else if new_state != seqs[pos].selected {
                    seqs[pos].selected = new_state;
                    ctx.acts.push(Act::ScheduleAnalyze(ctx.project_id));
                }

                if seqs[pos].tracks.is_empty() {
                    continue;
                }
                let seq_name = seqs[pos].name.clone();
                let seq_on = seqs[pos].selected;
                // 轨道上 Shift+点击：把本工程里所有同名轨道（C1/C2…）设成同一状态。
                // 循环里借用了 seqs，所以先记下来，循环结束后再统一改。
                let mut track_sync: Option<(usize, bool)> = None;
                for t in seqs[pos].tracks.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 14.0 + 54.0);
                        let resp = ui
                            .add_enabled(seq_on, egui::Checkbox::new(&mut t.selected, ""))
                            .on_hover_text(
                                "按住 Shift 点击 = 本工程所有序列里同名字幕轨道一起勾选/取消",
                            );
                        if resp.clicked() && ui.input(|i| i.modifiers.shift) {
                            track_sync = Some((t.info.index, t.selected));
                        }
                        let lab = RichText::new(t.info.label()).monospace().size(12.5);
                        ui.label(if seq_on { lab } else { lab.weak() });
                        if t.info.enabled {
                            ui.label(RichText::new("启用").color(GREEN).size(10.5));
                        } else {
                            ui.label(RichText::new("已禁用").color(GRAY).size(10.5));
                        }
                        ui.label(
                            RichText::new(format!("{} 条", t.info.text_count()))
                                .weak()
                                .size(11.0),
                        );
                        if ui.small_button("预览").clicked() {
                            ctx.acts.push(Act::Preview {
                                title: format!(
                                    "{} / {} / {}",
                                    ctx.project_name,
                                    seq_name,
                                    t.info.label()
                                ),
                                project: ctx.project_name.to_string(),
                                seq: seq_name.clone(),
                                items: t.info.items.clone(),
                            });
                        }
                    });
                }
                if let Some((index, v)) = track_sync {
                    set_track_selected_everywhere(seqs, index, v);
                }
            }
        }
    }
}

/// 把本工程所有序列里「序号为 index」的轨道（即同名的 C1/C2…）设成同一状态。
fn set_track_selected_everywhere(seqs: &mut [SeqEntry], index: usize, value: bool) {
    for s in seqs.iter_mut() {
        if let Some(t) = s.tracks.iter_mut().find(|t| t.info.index == index) {
            t.selected = value;
        }
    }
}

/// 目录里（递归）的 (序列总数, 已勾选数)
fn folder_stats(nodes: &[ProjectNode], seqs: &[SeqEntry]) -> (usize, usize) {
    let mut total = 0;
    let mut sel = 0;
    for n in nodes {
        match n {
            ProjectNode::Folder { children, .. } => {
                let (t, s) = folder_stats(children, seqs);
                total += t;
                sel += s;
            }
            ProjectNode::Seq(m) => {
                total += 1;
                if seqs.iter().any(|s| s.uid == m.uid && s.selected) {
                    sel += 1;
                }
            }
        }
    }
    (total, sel)
}

/// Shift+点击目录/序列的语义：只影响**同一层级**的兄弟节点。
/// 每个兄弟目录依旧连同自己的整棵子树一起改，所以根层级就是「整个工程」。
fn set_level_selected(nodes: &[ProjectNode], seqs: &mut [SeqEntry], value: bool) {
    set_subtree_selected(nodes, seqs, value);
}

fn set_subtree_selected(nodes: &[ProjectNode], seqs: &mut [SeqEntry], value: bool) {
    for n in nodes {
        match n {
            ProjectNode::Folder { children, .. } => set_subtree_selected(children, seqs, value),
            ProjectNode::Seq(m) => {
                if let Some(s) = seqs.iter_mut().find(|s| s.uid == m.uid) {
                    s.selected = value;
                }
            }
        }
    }
}

/// 目录展开/折叠的小三角（自己画，避免字体缺字形）。
fn collapse_arrow(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
    let c = rect.center();
    let color = ui.visuals().weak_text_color();
    let pts = if open {
        vec![
            egui::pos2(c.x - 4.0, c.y - 2.5),
            egui::pos2(c.x + 4.0, c.y - 2.5),
            egui::pos2(c.x, c.y + 3.0),
        ]
    } else {
        vec![
            egui::pos2(c.x - 2.5, c.y - 4.0),
            egui::pos2(c.x - 2.5, c.y + 4.0),
            egui::pos2(c.x + 3.0, c.y),
        ]
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    resp
}

/// 三态勾选框（全选 / 部分 / 未选）。
fn tri_checkbox(ui: &mut egui::Ui, state: Tri) -> egui::Response {
    let size = egui::vec2(14.0, 14.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&resp);
    let stroke = egui::Stroke::new(1.0_f32, visuals.bg_stroke.color);
    let painter = ui.painter();
    painter.rect(rect, 2.0, visuals.bg_fill, stroke);
    let fg = egui::Stroke::new(2.0_f32, visuals.fg_stroke.color);
    match state {
        Tri::All => {
            let a = egui::pos2(rect.left() + 3.0, rect.center().y);
            let b = egui::pos2(rect.left() + 6.0, rect.bottom() - 3.5);
            let c = egui::pos2(rect.right() - 3.0, rect.top() + 3.5);
            painter.line_segment([a, b], fg);
            painter.line_segment([b, c], fg);
        }
        Tri::Partial => {
            painter.line_segment(
                [
                    egui::pos2(rect.left() + 3.0, rect.center().y),
                    egui::pos2(rect.right() - 3.0, rect.center().y),
                ],
                fg,
            );
        }
        Tri::None => {}
    }
    resp
}

// ---------------------------------------------------------------- 小工具

fn shorten(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let head: String = chars[..max.saturating_sub(12)].iter().collect();
    let tail: String = chars[chars.len() - 10..].iter().collect();
    format!("{head}…{tail}")
}

/// 路径太长时只保留末尾（尽量从目录分隔符处断开），完整路径放到悬停提示里。
fn tail_path(path: &str, max: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max {
        return path.to_string();
    }
    let start = chars.len() - max;
    let cut = chars[start..]
        .iter()
        .position(|c| *c == '\\' || *c == '/')
        .map(|i| start + i)
        .unwrap_or(start);
    let tail: String = chars[cut..].iter().collect();
    format!("…{tail}")
}

fn truncate_lines(s: &str, n: usize) -> String {
    let mut out = String::new();
    for (i, line) in s.split('\n').enumerate() {
        if i >= n {
            out.push_str("…");
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 输出格式标签，如 "SRT+ASS"。
pub fn format_list(srt: bool, ass: bool) -> String {
    match (srt, ass) {
        (true, true) => "SRT+ASS".into(),
        (true, false) => "SRT".into(),
        (false, true) => "ASS".into(),
        (false, false) => "—".into(),
    }
}

/// 规范化用户输入的文件名后缀：清掉非法字符，必要时补一个下划线。
pub fn normalize_suffix(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            '\r' | '\n' | '\t' => ' ',
            c => c,
        })
        .collect();
    let s = cleaned.trim().trim_end_matches('.').trim().to_string();
    if s.is_empty() {
        return String::new();
    }
    match s.chars().next() {
        Some('_') | Some('-') | Some('.') | Some(' ') => s,
        _ => format!("_{s}"),
    }
}

/// 合并稿的文件名主名（不含扩展名）：`[工程_]序列_合并[后缀]`。
/// 合并稿永远不带轨道名，用固定的「_合并」标记区分，避免和单轨输出撞名。
pub fn merged_base(include_project: bool, project: &str, seq: &str, suffix: &str) -> String {
    let mut base = String::new();
    if include_project {
        base.push_str(&srt::sanitize_filename(project));
        base.push('_');
    }
    base.push_str(&srt::sanitize_filename(seq));
    base.push_str(MERGED_FILE_MARK);
    base.push_str(suffix);
    base
}

/// 给不含扩展名的主名补上扩展名。
fn with_ext(base: &std::path::Path, ext: &str) -> PathBuf {
    let mut s = base.as_os_str().to_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

fn default_output_dir() -> PathBuf {
    if let Ok(p) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(p);
        let target = p.join("Documents").join("字幕导出");
        if target.exists() && target.is_dir() {
            return target;
        }
    }
    PathBuf::from(".")
}

fn install_fonts(ctx: &egui::Context) -> bool {
    const CANDIDATES: [&str; 6] = [
        "C:/Windows/Fonts/Deng.ttf",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simsun.ttc",
        "C:/Windows/Fonts/simkai.ttf",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("cjk".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
        ctx.set_fonts(fonts);
        return true;
    }
    false
}

fn apply_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    if let Some(f) = style.text_styles.get_mut(&egui::TextStyle::Body) {
        f.size = 14.0;
    }
    if let Some(f) = style.text_styles.get_mut(&egui::TextStyle::Button) {
        f.size = 13.5;
    }
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_normalization() {
        assert_eq!(normalize_suffix(""), "");
        assert_eq!(normalize_suffix("   "), "");
        assert_eq!(normalize_suffix("2026"), "_2026");
        assert_eq!(normalize_suffix("_v2"), "_v2");
        assert_eq!(normalize_suffix("-重制"), "-重制");
        assert_eq!(normalize_suffix("a/b:c"), "_a_b_c");
        assert_eq!(normalize_suffix("末尾."), "_末尾");
    }

    #[test]
    fn format_labels() {
        assert_eq!(format_list(true, true), "SRT+ASS");
        assert_eq!(format_list(true, false), "SRT");
        assert_eq!(format_list(false, true), "ASS");
        assert_eq!(format_list(false, false), "—");
    }

    #[test]
    fn merged_file_name_rules() {
        assert_eq!(merged_base(true, "工程A", "序列1", ""), "工程A_序列1_合并");
        assert_eq!(merged_base(false, "工程A", "序列1", "_v2"), "序列1_合并_v2");
        // 非法字符照样清洗
        assert_eq!(merged_base(true, "a/b", "序列:2", ""), "a_b_序列_2_合并");
    }

    fn seq(uid: &str) -> SeqEntry {
        SeqEntry {
            uid: uid.into(),
            name: uid.into(),
            fps: 0.0,
            selected: false,
            status: SStatus::Idle,
            tracks: Vec::new(),
        }
    }

    fn seq_node(uid: &str) -> ProjectNode {
        ProjectNode::Seq(prproj::SeqMeta {
            uid: uid.into(),
            name: uid.into(),
            fps: 0.0,
            caption_tracks: 0,
        })
    }

    #[test]
    fn shift_scope_covers_only_the_same_level() {
        // 根：A { s1, B { s2 } }, s3
        let tree = vec![
            ProjectNode::Folder {
                name: "A".into(),
                children: vec![
                    seq_node("s1"),
                    ProjectNode::Folder {
                        name: "B".into(),
                        children: vec![seq_node("s2")],
                    },
                ],
            },
            seq_node("s3"),
        ];
        let mut seqs = vec![seq("s1"), seq("s2"), seq("s3")];

        // 在 A 里面 Shift：只影响 A 这一层（s1、B 及其子级 s2），不动根层的 s3
        let ProjectNode::Folder { children, .. } = &tree[0] else {
            panic!("第一个是目录")
        };
        set_level_selected(children, &mut seqs, true);
        assert!(seqs[0].selected && seqs[1].selected);
        assert!(!seqs[2].selected, "同层级之外的不该被改");

        // 在根这一层 Shift：整个工程一起切换
        set_level_selected(&tree, &mut seqs, false);
        assert!(seqs.iter().all(|s| !s.selected));
    }

    #[test]
    fn shift_on_a_track_syncs_the_same_track_elsewhere() {
        let track = |index: usize, sel: bool| TrackEntry {
            info: prproj::TrackInfo {
                index,
                enabled: true,
                items: vec![Caption::default()],
            },
            selected: sel,
        };
        let mut seqs = vec![seq("a"), seq("b"), seq("c")];
        seqs[0].tracks = vec![track(0, true), track(1, false)];
        seqs[1].tracks = vec![track(0, true), track(1, false)];
        seqs[2].tracks = vec![track(0, true)]; // 这个序列没有 C2

        set_track_selected_everywhere(&mut seqs, 1, true);
        assert!(seqs[0].tracks[1].selected && seqs[1].tracks[1].selected);
        assert!(seqs[0].tracks[0].selected, "另一个轨道不该被改");
        set_track_selected_everywhere(&mut seqs, 1, false);
        assert!(!seqs[0].tracks[1].selected && !seqs[1].tracks[1].selected);
        assert_eq!(seqs[2].tracks.len(), 1, "没有该轨道的序列不该凭空长出来");
    }
}
