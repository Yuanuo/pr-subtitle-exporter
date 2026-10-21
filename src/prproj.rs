//! 解析 Premiere Pro 工程文件（.prproj）中的序列 / 字幕轨道 / 字幕文本。
//!
//! 关键实现要点（依据真实 prproj 结构逆向确认）：
//! 1. .prproj 是 gzip 压缩的 XML（极少数为未压缩 XML）。
//! 2. 所有对象以扁平列表挂在 <PremiereData> 下，通过 ObjectID / ObjectUID 互相引用。
//! 3. 字幕轨道是 DataTrackGroup 下的 CaptionDataClipTrack；
//!    轨道的“输出启用”状态由 <IsMuted>true</IsMuted> 表示（与视频轨的眼镜图标同一标记）。
//! 4. 字幕文本存放在 Block/FormattedTextData，是一个 base64 编码的二进制块
//!    （Adobe 自定义 FlatBuffer），正文是该块内最后一个“长度前缀 + UTF-8”字符串。
//! 5. Premiere 会对完全相同的文本块做去重：重复出现的 Block 其 FormattedTextData 内容为空，
//!    只保留 BinaryHash。必须建立 BinaryHash -> 文本 的索引，否则会丢失大量字幕。
//! 6. 时间单位是 tick，1 秒 = 254016000000 tick。

use base64::Engine;
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;

/// Premiere 的 tick 精度：每秒 254016000000 tick。
pub const TICKS_PER_SECOND: u64 = 254_016_000_000;

pub type Res<T> = Result<T, String>;

#[derive(Clone, Debug, Default)]
pub struct Caption {
    pub start: u64,
    pub end: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct TrackInfo {
    /// 轨道在 DataTrackGroup 中的序号（0 基），UI 显示为 C1、C2 …
    pub index: usize,
    /// 轨道是否启用（未静音）
    pub enabled: bool,
    pub items: Vec<Caption>,
}

impl TrackInfo {
    pub fn label(&self) -> String {
        format!("C{}", self.index + 1)
    }
    pub fn text_count(&self) -> usize {
        self.items
            .iter()
            .filter(|c| !c.text.trim().is_empty())
            .count()
    }
}

#[derive(Clone, Debug, Default)]
pub struct SeqMeta {
    pub uid: String,
    pub name: String,
    pub fps: f64,
    /// 字幕轨道总数（未分析前的快速概览）
    pub caption_tracks: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SeqDetail {
    pub uid: String,
    pub name: String,
    pub fps: f64,
    pub tracks: Vec<TrackInfo>,
}

/// 工程面板里的一个节点：素材箱（目录）或序列。
#[derive(Clone, Debug)]
pub enum ProjectNode {
    Folder {
        name: String,
        children: Vec<ProjectNode>,
    },
    Seq(SeqMeta),
}

impl ProjectNode {
    pub fn name(&self) -> &str {
        match self {
            ProjectNode::Folder { name, .. } => name,
            ProjectNode::Seq(m) => &m.name,
        }
    }
}

/// 递归取出所有序列（含嵌套在素材箱里的）。
pub fn flatten(nodes: &[ProjectNode]) -> Vec<SeqMeta> {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            ProjectNode::Folder { children, .. } => out.extend(flatten(children)),
            ProjectNode::Seq(m) => out.push(m.clone()),
        }
    }
    out
}

/// 读取工程文件并解压为 XML 文本。
fn load_xml_text(path: &Path) -> Res<String> {
    let raw = fs::read(path).map_err(|e| format!("无法读取文件：{e}"))?;
    if raw.is_empty() {
        return Err("文件为空".into());
    }
    let data = if raw[0] == 0x1f && raw[1] == 0x8b {
        let mut dec = flate2::read::MultiGzDecoder::new(&raw[..]);
        let mut out = Vec::with_capacity(raw.len() * 6);
        dec.read_to_end(&mut out)
            .map_err(|e| format!("gzip 解压失败（文件可能损坏或不是有效的 prproj）：{e}"))?;
        out
    } else {
        raw
    };
    let text = String::from_utf8(data).map_err(|_| "工程内容不是有效的 UTF-8 XML".to_string())?;
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

struct Idx<'a, 'i> {
    by_id: HashMap<&'a str, Node<'a, 'i>>,
    by_uid: HashMap<&'a str, Node<'a, 'i>>,
    /// BinaryHash -> 解码后的字幕文本（用于处理 Premiere 的文本块去重）
    text_by_hash: HashMap<String, String>,
}

impl<'a, 'i> Idx<'a, 'i> {
    fn build(doc: &'a Document<'i>, with_text: bool) -> Self {
        let mut by_id = HashMap::new();
        let mut by_uid = HashMap::new();
        let mut text_by_hash = HashMap::new();

        for n in doc.root_element().children().filter(|n| n.is_element()) {
            if let Some(id) = n.attribute("ObjectID") {
                by_id.insert(id, n);
            }
            if let Some(uid) = n.attribute("ObjectUID") {
                by_uid.insert(uid, n);
            }
        }

        if with_text {
            for n in doc
                .descendants()
                .filter(|n| n.has_tag_name("FormattedTextData"))
            {
                let raw = n.text().map(str::trim).unwrap_or("");
                if raw.is_empty() {
                    continue;
                }
                let Some(hash) = n.attribute("BinaryHash") else {
                    continue;
                };
                if !text_by_hash.contains_key(hash) {
                    if let Some(t) = decode_text_blob(raw) {
                        text_by_hash.insert(hash.to_string(), t);
                    }
                }
            }
        }

        Self {
            by_id,
            by_uid,
            text_by_hash,
        }
    }

    fn id(&self, id: &str) -> Option<Node<'a, 'i>> {
        self.by_id.get(id).copied()
    }
    fn uid(&self, uid: &str) -> Option<Node<'a, 'i>> {
        self.by_uid.get(uid).copied()
    }
}

fn child<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.has_tag_name(tag))
}

fn text_of(n: Node) -> String {
    n.text().unwrap_or("").trim().to_string()
}

/// 按工程面板的目录层级列出序列（只做轻量解析，不读取字幕内容）。
///
/// 层级来源：`Project/RootProjectItem → BinProjectItem → ProjectItemContainer/Items/Item`。
/// 没有挂到任何素材箱下的序列（Premiere 有时会这样存）会放在根层级。
pub fn list_sequences(path: &Path) -> Res<Vec<ProjectNode>> {
    let text = load_xml_text(path)?;
    parse_project_tree(&text)
}

/// 从 XML 文本构建工程面板的序列树（便于单测）。
pub fn parse_project_tree(text: &str) -> Res<Vec<ProjectNode>> {
    let doc = Document::parse(text).map_err(|e| format!("XML 解析失败：{e}"))?;
    let idx = Idx::build(&doc, false);

    // 1) 所有序列的轻量元信息
    let mut metas: HashMap<String, SeqMeta> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for seq in sequence_nodes(&doc) {
        let name = child(seq, "Name")
            .map(text_of)
            .unwrap_or_else(|| "（未命名序列）".into());
        let uid = seq.attribute("ObjectUID").unwrap_or("").to_string();
        if uid.is_empty() || metas.contains_key(&uid) {
            continue;
        }
        let groups = caption_groups(seq, &idx);
        let fps = groups
            .first()
            .and_then(|g| group_frame_ticks(*g))
            .map(|ft| TICKS_PER_SECOND as f64 / ft as f64)
            .unwrap_or(0.0);
        let tracks = groups.iter().map(|g| count_tracks(*g, &idx)).sum();
        metas.insert(
            uid.clone(),
            SeqMeta {
                uid: uid.clone(),
                name,
                fps,
                caption_tracks: tracks,
            },
        );
        order.push(uid);
    }

    // 2) MasterClip -> 序列：工程面板里的「序列项」很多时候是
    //    ClipProjectItem → MasterClip → AudioClip/VideoClip → *SequenceSource → <Sequence>，
    //    这里先把这层映射解开，否则放不进素材箱。
    let mc_to_seq = masterclip_sequence_map(&doc, &idx);

    // 3) 从根项目项递归展开素材箱
    let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut root: Vec<ProjectNode> = Vec::new();
    if let Some(root_uid) = root_project_item(&doc) {
        collect_bin(
            root_uid,
            &idx,
            &metas,
            &mc_to_seq,
            &mut placed,
            &mut root,
            0,
        );
    }

    // 4) 没有归入任何素材箱的序列放到根层级
    for uid in &order {
        if placed.contains(uid) {
            continue;
        }
        if let Some(m) = metas.get(uid) {
            root.push(ProjectNode::Seq(m.clone()));
        }
    }

    sort_nodes(&mut root);
    Ok(root)
}

fn root_project_item<'a, 'i>(doc: &'a Document<'i>) -> Option<&'a str> {
    for p in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("Project"))
    {
        if let Some(c) = p.children().find(|c| c.has_tag_name("RootProjectItem")) {
            if let Some(u) = c.attribute("ObjectURef") {
                return Some(u);
            }
        }
    }
    None
}

/// 素材箱名（`ProjectItem/Name`，兼容直接是 `<Name>` 的情况）。
fn bin_name(bin: Node) -> String {
    bin.descendants()
        .find(|n| n.has_tag_name("Name"))
        .map(text_of)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "（未命名素材箱）".into())
}

/// 一个工程项如果是序列，返回序列的 ObjectUID。
fn sequence_uid_of(item: Node, mc_to_seq: &HashMap<String, String>) -> Option<String> {
    if item.has_tag_name("Sequence") {
        return item.attribute("ObjectUID").map(str::to_string);
    }
    // 直接内嵌 <Sequence ObjectURef="…">
    if let Some(u) = item
        .descendants()
        .find(|n| n.has_tag_name("Sequence"))
        .and_then(|n| n.attribute("ObjectURef"))
    {
        return Some(u.to_string());
    }
    // 经由 MasterClip 间接引用（序列在面板里通常就是这样存的项目项）
    if let Some(mc_uid) = child(item, "MasterClip").and_then(|m| m.attribute("ObjectURef")) {
        if let Some(seq) = mc_to_seq.get(mc_uid) {
            return Some(seq.clone());
        }
    }
    None
}

/// 建立 MasterClip(ObjectUID) -> 序列(ObjectUID) 的映射。
fn masterclip_sequence_map<'a, 'i>(
    doc: &'a Document<'i>,
    idx: &Idx<'a, 'i>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for mc in doc.descendants().filter(|n| n.has_tag_name("MasterClip")) {
        let Some(uid) = mc.attribute("ObjectUID") else {
            continue;
        };
        if let Some(seq) = find_sequence_ref(mc, idx) {
            out.insert(uid.to_string(), seq);
        }
    }
    out
}

/// 在元素的引用图里找 `<Sequence ObjectURef="…">`（有界 BFS，避免把整个工程走一遍）。
fn find_sequence_ref<'a, 'i>(start: Node<'a, 'i>, idx: &Idx<'a, 'i>) -> Option<String> {
    if let Some(u) = inline_sequence(start, 4) {
        return Some(u);
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut frontier: Vec<Node<'a, 'i>> = vec![start];
    for _ in 0..6 {
        let mut next: Vec<Node<'a, 'i>> = Vec::new();
        for n in frontier {
            for c in n.descendants() {
                for attr in ["ObjectRef", "ObjectURef"] {
                    let Some(v) = c.attribute(attr) else { continue };
                    if !seen.insert(v.to_string()) {
                        continue;
                    }
                    let target = if attr == "ObjectRef" {
                        idx.id(v)
                    } else {
                        idx.uid(v)
                    };
                    let Some(t) = target else { continue };
                    // 不去遍历序列自身的内容（可能很大）
                    if t.has_tag_name("Sequence") {
                        continue;
                    }
                    if let Some(u) = inline_sequence(t, 4) {
                        return Some(u);
                    }
                    next.push(t);
                    if seen.len() > 4000 {
                        return None;
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    None
}

/// 只看元素自身附近（depth 层内）有没有内联的 `<Sequence ObjectURef>`。
fn inline_sequence(n: Node, depth: usize) -> Option<String> {
    if n.has_tag_name("Sequence") {
        if let Some(u) = n.attribute("ObjectURef") {
            return Some(u.to_string());
        }
    }
    if depth == 0 {
        return None;
    }
    for c in n.children() {
        if let Some(u) = inline_sequence(c, depth - 1) {
            return Some(u);
        }
    }
    None
}

fn collect_bin(
    bin_uid: &str,
    idx: &Idx,
    metas: &HashMap<String, SeqMeta>,
    mc_to_seq: &HashMap<String, String>,
    placed: &mut std::collections::HashSet<String>,
    out: &mut Vec<ProjectNode>,
    depth: usize,
) {
    if depth > 64 {
        return;
    }
    let Some(bin) = idx.uid(bin_uid) else { return };

    // Items 按 Index 排序，保证与工程面板顺序一致
    let mut items: Vec<(i64, &str)> = bin
        .descendants()
        .filter(|n| n.has_tag_name("Item"))
        .filter_map(|n| {
            let u = n.attribute("ObjectURef")?;
            let i = n
                .attribute("Index")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0);
            Some((i, u))
        })
        .collect();
    items.sort_by_key(|(i, _)| *i);

    let mut children = Vec::new();
    for (_, uid) in items {
        if placed.contains(uid) {
            continue;
        }
        let Some(item) = idx.uid(uid) else { continue };
        if item.has_tag_name("BinProjectItem") {
            let name = bin_name(item);
            let mut sub = Vec::new();
            collect_bin(uid, idx, metas, mc_to_seq, placed, &mut sub, depth + 1);
            children.push(ProjectNode::Folder {
                name,
                children: sub,
            });
        } else if let Some(seq_uid) = sequence_uid_of(item, mc_to_seq) {
            if let Some(m) = metas.get(&seq_uid) {
                if placed.insert(seq_uid.clone()) {
                    children.push(ProjectNode::Seq(m.clone()));
                }
            }
        }
    }

    sort_nodes(&mut children);
    out.extend(children);
}

/// 自然排序：数字段按数值比较（序列2 < 序列10），其余不区分大小写。
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);

    while i < av.len() && j < bv.len() {
        let ca = av[i];
        let cb = bv[j];
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while i < av.len() && av[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < bv.len() && bv[j].is_ascii_digit() {
                j += 1;
            }
            let na: String = av[si..i].iter().collect();
            let nb: String = bv[sj..j].iter().collect();
            let ta = na.trim_start_matches('0');
            let tb = nb.trim_start_matches('0');
            let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            let la = ca.to_ascii_lowercase();
            let lb = cb.to_ascii_lowercase();
            if la != lb {
                return la.cmp(&lb);
            }
            i += 1;
            j += 1;
        }
    }
    match (i < av.len(), j < bv.len()) {
        (false, false) => a.cmp(b),
        (false, true) => Ordering::Less,
        (true, false) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// 目录优先，同类按名字自然排序；递归处理子级。
pub fn sort_nodes(nodes: &mut [ProjectNode]) {
    for n in nodes.iter_mut() {
        if let ProjectNode::Folder { children, .. } = n {
            sort_nodes(children);
        }
    }
    nodes.sort_by(|a, b| {
        let fa = matches!(a, ProjectNode::Folder { .. });
        let fb = matches!(b, ProjectNode::Folder { .. });
        fb.cmp(&fa).then_with(|| natural_cmp(a.name(), b.name()))
    });
}

/// 解析指定序列（按 ObjectUID 指定）的字幕轨道与字幕条目。
pub fn analyze(path: &Path, want_uids: &[String]) -> Res<Vec<SeqDetail>> {
    let text = load_xml_text(path)?;
    let doc = Document::parse(&text).map_err(|e| format!("XML 解析失败：{e}"))?;
    let idx = Idx::build(&doc, true);
    let want: std::collections::HashSet<&str> = want_uids.iter().map(|s| s.as_str()).collect();

    let mut out = Vec::new();
    for seq in sequence_nodes(&doc) {
        let uid = seq.attribute("ObjectUID").unwrap_or("").to_string();
        if !want.is_empty() && !want.contains(uid.as_str()) {
            continue;
        }
        let name = child(seq, "Name")
            .map(text_of)
            .unwrap_or_else(|| "（未命名序列）".into());
        let groups = caption_groups(seq, &idx);
        let mut fps = 0.0;
        let mut tracks = Vec::new();

        for g in &groups {
            if fps == 0.0 {
                if let Some(ft) = group_frame_ticks(*g) {
                    fps = TICKS_PER_SECOND as f64 / ft as f64;
                }
            }
            let frame_ticks = group_frame_ticks(*g).unwrap_or(TICKS_PER_SECOND / 25);
            for (pos, tref) in track_refs(*g).into_iter().enumerate() {
                let Some(track) = idx.uid(tref) else { continue };
                let index = tref_index(track, pos);
                let enabled = !is_muted(track);
                let mut items = collect_items(track, &idx, frame_ticks);
                items.sort_by_key(|c| (c.start, c.end));
                tracks.push(TrackInfo {
                    index,
                    enabled,
                    items,
                });
            }
        }
        tracks.sort_by_key(|t| t.index);
        out.push(SeqDetail {
            uid,
            name,
            fps,
            tracks,
        });
    }
    Ok(out)
}

fn sequence_nodes<'a, 'i>(doc: &'a Document<'i>) -> Vec<Node<'a, 'i>> {
    doc.root_element()
        .children()
        .filter(|n| n.is_element() && n.has_tag_name("Sequence"))
        .collect()
}

/// 序列中的字幕（Data）轨道组。
fn caption_groups<'a, 'i>(seq: Node<'a, 'i>, idx: &Idx<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let mut out = Vec::new();
    if let Some(tgs) = child(seq, "TrackGroups") {
        for tg in tgs
            .children()
            .filter(|c| c.is_element() && c.has_tag_name("TrackGroup"))
        {
            // 约定结构：<Second ObjectRef="…"/> 指向 DataTrackGroup
            let mut found = None;
            if let Some(second) = child(tg, "Second") {
                if let Some(r) = second.attribute("ObjectRef") {
                    found = idx.id(r);
                }
            }
            if found.is_none() {
                found = tg
                    .children()
                    .find(|c| c.is_element() && c.has_tag_name("DataTrackGroup"));
            }
            if let Some(g) = found {
                if g.has_tag_name("DataTrackGroup") {
                    out.push(g);
                }
            }
        }
    }
    out
}

fn group_frame_ticks(group: Node) -> Option<u64> {
    group
        .descendants()
        .find(|n| n.has_tag_name("FrameRate"))
        .and_then(|n| n.text())
        .and_then(|t| t.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
}

/// 轨道组内所有轨道的 ObjectUID 引用（按轨道顺序）。
fn track_refs<'a, 'i>(group: Node<'a, 'i>) -> Vec<&'a str> {
    group
        .descendants()
        .filter(|n| n.has_tag_name("Track"))
        .filter_map(|n| n.attribute("ObjectURef"))
        .collect()
}

fn count_tracks(group: Node, idx: &Idx) -> usize {
    track_refs(group)
        .into_iter()
        .filter(|u| idx.uid(u).is_some())
        .count()
}

fn tref_index(track: Node, fallback: usize) -> usize {
    track
        .descendants()
        .filter(|n| n.has_tag_name("Track"))
        .find_map(|n| {
            n.attribute("Index")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(fallback)
}

/// 轨道是否被关闭输出。<IsMuted>true</IsMuted> 即 Premiere 中该轨道的“启用/眼睛”开关。
fn is_muted(track: Node) -> bool {
    track
        .descendants()
        .filter(|n| n.has_tag_name("IsMuted"))
        .any(|n| {
            n.text()
                .map(|t| {
                    let v = t.trim().to_ascii_lowercase();
                    v == "true" || v == "1"
                })
                .unwrap_or(true)
        })
}

fn collect_items(track: Node, idx: &Idx, frame_ticks: u64) -> Vec<Caption> {
    let mut out = Vec::new();
    for ti in track.descendants().filter(|n| n.has_tag_name("TrackItem")) {
        let Some(oref) = ti.attribute("ObjectRef") else {
            continue;
        };
        let Some(item) = idx.id(oref) else { continue };
        if !item.has_tag_name("CaptionDataClipTrackItem") && !item.has_tag_name("DataClipTrackItem")
        {
            continue;
        }
        let Some(ti_node) = item.descendants().find(|n| n.has_tag_name("TrackItem")) else {
            continue;
        };
        let start = child(ti_node, "Start")
            .map(text_of)
            .and_then(|t| t.parse::<u64>().ok());
        let end = child(ti_node, "End")
            .map(text_of)
            .and_then(|t| t.parse::<u64>().ok());
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        // 兜底：结束时间不晚于开始时间时，按一帧延长，避免生成非法 SRT
        let end = if end <= start {
            start + frame_ticks
        } else {
            end
        };
        let text = item_text(item, idx);
        out.push(Caption { start, end, text });
    }
    out
}

/// 取出一个字幕条目的文本（一个条目可能有多个 Block，逐行拼接）。
fn item_text(item: Node, idx: &Idx) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(bv) = child(item, "BlockVector") {
        for bvi in bv
            .children()
            .filter(|c| c.is_element() && c.has_tag_name("BlockVectorItem"))
        {
            let Some(r) = bvi.attribute("ObjectRef") else {
                continue;
            };
            let Some(blk) = idx.id(r) else { continue };
            let Some(ftd) = child(blk, "FormattedTextData") else {
                continue;
            };
            let raw = ftd.text().map(str::trim).unwrap_or("");
            let hash = ftd.attribute("BinaryHash").unwrap_or("");

            let text = if raw.is_empty() {
                // 被去重的块：靠 BinaryHash 找回原文
                idx.text_by_hash.get(hash).cloned().unwrap_or_default()
            } else {
                decode_text_blob(raw)
                    .or_else(|| idx.text_by_hash.get(hash).cloned())
                    .unwrap_or_default()
            };
            for line in clean_text(&text).split('\n') {
                if !line.trim().is_empty() {
                    parts.push(line.trim_end().to_string());
                }
            }
        }
    }
    parts.join("\n")
}

/// 规范化换行、去除首尾空行，保留内部换行。
pub fn clean_text(s: &str) -> String {
    let norm = s.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = norm.split('\n').map(|l| l.trim()).collect();
    while lines.first().map_or(false, |l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// 解码 base64 的 FormattedTextData 二进制块，取出其中的字幕正文。
///
/// 该块是 Adobe 自定义的 FlatBuffer：正文作为最后一个“长度前缀 + UTF-8”字符串保存，
/// 此前还会出现 "AnimationType"、字体名等字符串，因此取最后一个合法候选。
pub fn decode_text_blob(b64: &str) -> Option<String> {
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let engine = base64::engine::general_purpose::STANDARD;
    let bytes = engine
        .decode(cleaned.as_bytes())
        .or_else(|_| {
            let padded = format!("{cleaned}{}", "=".repeat((4 - cleaned.len() % 4) % 4));
            engine.decode(padded.as_bytes())
        })
        .ok()?;
    last_flatbuffer_string(&bytes)
}

fn last_flatbuffer_string(buf: &[u8]) -> Option<String> {
    let mut found: Option<String> = None;
    let n = buf.len();
    let mut i = 0usize;
    while i + 4 <= n {
        let len = i32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
        if len > 0 && len <= 16384 {
            let end = i + 4 + len as usize;
            if end <= n {
                if let Ok(s) = std::str::from_utf8(&buf[i + 4..end]) {
                    if !s.is_empty()
                        && s.chars()
                            .all(|c| c == '\r' || c == '\n' || c == '\t' || !c.is_control())
                    {
                        found = Some(s.to_string());
                        i = end;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<PremiereData Version="3">
  <Project ObjectRef="1"/>
  <Project ObjectID="1">
    <RootProjectItem ObjectURef="root"/>
  </Project>
  <RootProjectItem ObjectUID="root">
    <ProjectItemContainer><Items>
      <Item Index="0" ObjectURef="binA"/>
      <Item Index="1" ObjectURef="seq3"/>
    </Items></ProjectItemContainer>
  </RootProjectItem>
  <BinProjectItem ObjectUID="binA">
    <ProjectItem Version="1"><Name>素材箱</Name></ProjectItem>
    <ProjectItemContainer><Items>
      <Item Index="0" ObjectURef="itemS10"/>
      <Item Index="1" ObjectURef="binB"/>
      <Item Index="2" ObjectURef="itemS2"/>
    </Items></ProjectItemContainer>
  </BinProjectItem>
  <BinProjectItem ObjectUID="binB">
    <ProjectItem Version="1"><Name>子箱</Name></ProjectItem>
    <ProjectItemContainer><Items>
      <Item Index="0" ObjectURef="itemS1"/>
    </Items></ProjectItemContainer>
  </BinProjectItem>
  <ClipProjectItem ObjectUID="itemS1"><ProjectItem Version="1"><Name>序列1</Name></ProjectItem><Sequence ObjectURef="seq1"/></ClipProjectItem>
  <ClipProjectItem ObjectUID="itemS2"><ProjectItem Version="1"><Name>序列2</Name></ProjectItem><Sequence ObjectURef="seq2"/></ClipProjectItem>
  <ClipProjectItem ObjectUID="itemS10"><ProjectItem Version="1"><Name>序列10</Name></ProjectItem><Sequence ObjectURef="seq10"/></ClipProjectItem>
  <Sequence ObjectUID="seq1"><Name>序列1</Name></Sequence>
  <Sequence ObjectUID="seq2"><Name>序列2</Name></Sequence>
  <Sequence ObjectUID="seq10"><Name>序列10</Name></Sequence>
  <Sequence ObjectUID="seq3"><Name>序列3</Name></Sequence>
</PremiereData>"#;

    fn dump(nodes: &[ProjectNode], depth: usize, out: &mut String) {
        for n in nodes {
            for _ in 0..depth {
                out.push_str("  ");
            }
            match n {
                ProjectNode::Folder { name, children } => {
                    out.push_str(name);
                    out.push_str("/\n");
                    dump(children, depth + 1, out);
                }
                ProjectNode::Seq(m) => {
                    out.push_str(&m.name);
                    out.push('\n');
                }
            }
        }
    }

    const INDIRECT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<PremiereData Version="3">
  <Project ObjectID="1">
    <RootProjectItem ObjectURef="root"/>
  </Project>
  <RootProjectItem ObjectUID="root">
    <ProjectItemContainer><Items><Item Index="0" ObjectURef="binA"/></Items></ProjectItemContainer>
  </RootProjectItem>
  <BinProjectItem ObjectUID="binA">
    <ProjectItem Version="1"><Name>成片</Name></ProjectItem>
    <ProjectItemContainer><Items>
      <Item Index="0" ObjectURef="itemSeq2"/>
      <Item Index="1" ObjectURef="itemMedia"/>
    </Items></ProjectItemContainer>
  </BinProjectItem>
  <ClipProjectItem ObjectUID="itemSeq2"><ProjectItem Version="1"><Name>旧名字</Name></ProjectItem><MasterClip ObjectURef="mc1"/></ClipProjectItem>
  <MasterClip ObjectUID="mc1"><Clips><Clip Index="0" ObjectRef="11"/></Clips><Name>序列一</Name></MasterClip>
  <AudioClip ObjectID="11"><SecondaryContents><SecondaryContent ObjectRef="12"/></SecondaryContents></AudioClip>
  <AudioSequenceSource ObjectID="12"><SequenceSource><Sequence ObjectURef="seq1"/></SequenceSource></AudioSequenceSource>
  <ClipProjectItem ObjectUID="itemMedia"><ProjectItem Version="1"><Name>素材.mp4</Name></ProjectItem><MasterClip ObjectURef="mc2"/></ClipProjectItem>
  <MasterClip ObjectUID="mc2"><Clips><Clip Index="0" ObjectRef="13"/></Clips><Name>素材.mp4</Name></MasterClip>
  <VideoClip ObjectID="13"/>
  <Sequence ObjectUID="seq1"><Name>序列一</Name></Sequence>
</PremiereData>"#;

    #[test]
    fn panel_item_backed_by_master_clip_maps_to_sequence() {
        let nodes = parse_project_tree(INDIRECT_XML).expect("parse");
        let mut out = String::new();
        dump(&nodes, 0, &mut out);
        // 素材箱里应出现「序列一」（用序列自己的名字），素材项被忽略
        assert_eq!(out, "成片/\n  序列一\n");
    }

    #[test]
    fn tree_keeps_bin_hierarchy_and_sorts_naturally() {
        let nodes = parse_project_tree(TREE_XML).expect("parse");
        let mut out = String::new();
        dump(&nodes, 0, &mut out);
        assert_eq!(
            out,
            "素材箱/\n  子箱/\n    序列1\n  序列2\n  序列10\n序列3\n"
        );
        // 扁平化后应包含全部 4 个序列
        assert_eq!(flatten(&nodes).len(), 4);
    }

    #[test]
    fn natural_order_handles_digits() {
        let mut v = vec!["序列10", "序列2", "序列1", "序列02", "ABC", "abd"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["ABC", "abd", "序列1", "序列02", "序列2", "序列10"]);
    }

    #[test]
    fn folders_sort_before_sequences() {
        let mut nodes = vec![
            ProjectNode::Seq(SeqMeta {
                name: "序列2".into(),
                ..Default::default()
            }),
            ProjectNode::Folder {
                name: "乙箱".into(),
                children: vec![],
            },
            ProjectNode::Seq(SeqMeta {
                name: "序列1".into(),
                ..Default::default()
            }),
            ProjectNode::Folder {
                name: "甲箱".into(),
                children: vec![],
            },
        ];
        sort_nodes(&mut nodes);
        let names: Vec<&str> = nodes.iter().map(|n| n.name()).collect();
        assert_eq!(names, vec!["乙箱", "甲箱", "序列1", "序列2"]);
    }
}
