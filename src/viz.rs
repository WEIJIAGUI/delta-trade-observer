//! 行情可视化模块：解析观察任务持久化的 `records.jsonl`，提供
//! 1）最新一轮价格概览（当前数值 + 涨跌状态）
//! 2）价格波动趋势图（一天 / 一周 / 一个月 时间维度切换，红涨绿跌，关键节点标注）
//!
//! 设计取舍（产品经理视角）：
//! - 涨跌色彩严格遵循 A 股习惯：**涨 = 红，跌 = 绿**（与游戏内货币数值监控一致）。
//! - 概览以「口径 → 子弹卡片」层级呈现，卡片含大字当前价、▲/▼ 差值与幅度、迷你火花线，
//!   信息层级：汇总（全局涨跌分布）→ 分组 → 单品，自上而下收敛。
//! - 趋势图手动用 `Painter` 绘制（不依赖 egui plot 特性），每段连线按方向着色，
//!   标注区间最高（红）/ 最低（绿）与最新点，坐标轴给出价格刻度与时间刻度。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{
    Align2, Color32, FontId, Frame, Margin, Painter, RichText, ScrollArea, Sense, Stroke, Ui,
};

/// 数据文件相对保存目录的路径（与 observe.rs 写入路径一致）。
const RECORDS_REL: &str = "prices/records.jsonl";

/// 涨（红）/ 跌（绿）配色，遵循 A 股习惯。
pub const UP: Color32 = Color32::from_rgb(230, 72, 77);
pub const DOWN: Color32 = Color32::from_rgb(48, 164, 108);
/// 持平 / 中性色。
const FLAT: Color32 = Color32::from_rgb(140, 150, 165);
/// 图表坐标轴与网格色。
fn axis_c() -> Color32 {
    Color32::from_rgba_unmultiplied(120, 130, 150, 120)
}
fn grid_c() -> Color32 {
    Color32::from_rgba_unmultiplied(150, 160, 180, 50)
}
const INK: Color32 = Color32::from_rgb(40, 48, 64);

/// 时间窗口跨度（毫秒）。
const DAY_MS: u64 = 86_400_000;
const WEEK_MS: u64 = 604_800_000;
const MONTH_MS: u64 = 2_592_000_000;

/// 可视化 Tab 的本地交互状态（选择项），持久于 App 生命周期。
#[derive(Default)]
pub struct VizView {
    /// 概览筛选：0 = 全部口径，否则 = 口径序号 + 1。
    pub overview_filter: usize,
    /// 趋势图选中的口径序号。
    pub chart_cal: usize,
    /// 趋势图选中的子弹序号（在该口径内）。
    pub chart_bullet: usize,
    /// 趋势图时间维度：0 = 一天，1 = 一周，2 = 一个月。
    pub range: usize,
}

/// 单条记录解析结果（已抽净价格数字）。
#[derive(Clone)]
struct Rec {
    caliber: String,
    bullet: String,
    price: u64,
    ts_ms: u64,
}

/// 全部解析后的可视化数据。
pub struct VizData {
    /// 口径列表（保持 config 顺序）。
    pub calibers: Vec<String>,
    /// 每个口径下的子弹名列表。
    pub bullets_by_cal: HashMap<String, Vec<String>>,
    /// 全局子弹列表（caliber, bullet）。
    pub bullets: Vec<(String, String)>,
    /// 子弹 key -> 按时间升序的价格序列 (ts_ms, price)。
    series: HashMap<String, Vec<(u64, u64)>>,
    /// 子弹 key -> 最新 (price, ts_ms)。
    pub latest: HashMap<String, (u64, u64)>,
    /// 子弹 key -> 次新 (price, ts_ms)（用于计算涨跌）。
    pub prev: HashMap<String, (u64, u64)>,
    /// 全局最近一次采集时间戳。
    pub last_ts: Option<u64>,
    /// 有效记录条数。
    pub count: usize,
}

/// 生成子弹唯一键（口径与子弹名用 NUL 分隔，二者均不含该字符）。
fn bkey(cal: &str, bullet: &str) -> String {
    let mut k = String::with_capacity(cal.len() + bullet.len() + 1);
    k.push_str(cal);
    k.push('\u{0}');
    k.push_str(bullet);
    k
}

/// 从 JSONL 行中极简提取字段（避免引入 serde 依赖）。
/// 仅依赖 observe.rs 写入时固定的键名顺序与转义。
fn parse_line(line: &str) -> Option<Rec> {
    let caliber = field(line, "caliber")?;
    let bullet = field(line, "bullet")?;
    let raw = field(line, "price")?;
    // 价格已为纯数字（observe 写入前已剔除千分位分隔符），仍做一次过滤兜底。
    let price: u64 = raw.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().ok()?;
    let ts_ms: u64 = field(line, "ts_ms")?.parse().ok()?;
    if price == 0 {
        return None;
    }
    Some(Rec { caliber, bullet, price, ts_ms })
}

/// 提取 `"key":"value"` 中的 value（支持内部转义 \" \\ \n \r \t）。
fn field(line: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    // 跳过可能的空白与：对于数字直接读，对于字符串读引号
    let rest_t = rest.trim_start();
    if rest_t.starts_with('"') {
        let mut out = String::new();
        let mut chars = rest_t[1..].chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => {
                    if let Some(n) = chars.next() {
                        match n {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'r' => out.push('\r'),
                            other => out.push(other),
                        }
                    }
                }
                other => out.push(other),
            }
        }
        Some(out)
    } else {
        // 数字：读到 , 或 } 或空白
        let end = rest_t
            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .unwrap_or(rest_t.len());
        Some(rest_t[..end].to_string())
    }
}

/// 从保存目录加载并构建可视化数据（缺失文件时返回空数据 + 提示）。
pub fn load(save_path: &str) -> VizData {
    let mut data = VizData {
        calibers: Vec::new(),
        bullets_by_cal: HashMap::new(),
        bullets: Vec::new(),
        series: HashMap::new(),
        latest: HashMap::new(),
        prev: HashMap::new(),
        last_ts: None,
        count: 0,
    };

    let path = format!("{save_path}/{RECORDS_REL}");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return data;
    };

    // 保持口径顺序：首次出现时入列（与观察遍历顺序一致）。
    let mut cal_order: Vec<String> = Vec::new();
    let mut bullets_by_cal: HashMap<String, Vec<String>> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(rec) = parse_line(line) else {
            continue;
        };
        let key = bkey(&rec.caliber, &rec.bullet);
        data.series.entry(key.clone()).or_default().push((rec.ts_ms, rec.price));
        data.count += 1;
        if data.last_ts.map(|t| rec.ts_ms > t).unwrap_or(true) {
            data.last_ts = Some(rec.ts_ms);
        }

        if !cal_order.contains(&rec.caliber) {
            cal_order.push(rec.caliber.clone());
            bullets_by_cal.insert(rec.caliber.clone(), Vec::new());
        }
        let bvec = bullets_by_cal.get_mut(&rec.caliber).unwrap();
        if !bvec.contains(&rec.bullet) {
            bvec.push(rec.bullet.clone());
            data.bullets.push((rec.caliber.clone(), rec.bullet.clone()));
        }
    }

    // 每个序列按时间升序排序，并计算 latest / prev。
    for (key, pts) in data.series.iter_mut() {
        pts.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(&last) = pts.last() {
            data.latest.insert(key.clone(), last);
        }
        if pts.len() >= 2 {
            data.prev.insert(key.clone(), pts[pts.len() - 2]);
        }
    }

    data.calibers = cal_order;
    data.bullets_by_cal = bullets_by_cal;
    data
}

/// 取某口径下子弹序号对应的 (caliber, bullet) 与 key。
fn bullet_at(data: &VizData, cal_idx: usize, bullet_idx: usize) -> Option<(String, String)> {
    let cal = data.calibers.get(cal_idx)?;
    let b = data.bullets_by_cal.get(cal)?.get(bullet_idx)?;
    Some((cal.clone(), b.clone()))
}

impl VizData {
    /// 该序列在指定时间窗口内的点（ts >= now - span）。
    fn window(&self, key: &str, span_ms: u64, now: u64) -> Vec<(u64, u64)> {
        let Some(pts) = self.series.get(key) else {
            return Vec::new();
        };
        let tmin = now.saturating_sub(span_ms);
        pts.iter().copied().filter(|(t, _)| *t >= tmin).collect()
    }
}

// ============================ 数值 / 时间格式化 ============================

/// 千分位格式化价格：1920 -> "1,920"。
fn fmt_price(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, &b) in bytes.iter().enumerate() {
        out.push(b as char);
        let from_end = len - i;
        if from_end > 1 && from_end % 3 == 1 {
            out.push(',');
        }
    }
    out
}

/// Unix 毫秒 -> UTC (年,月,日,时,分,秒)（Howard Hinnant 算法）。
fn unix_to_utc(ts_ms: u64) -> (i32, u32, u32, u32, u32, u32) {
    let z = (ts_ms as i64 / 1000) + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0,146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0,399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0,365]
    let mp = (5 * doy + 2) / 153; // [0,11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1,31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1,12]
    let y = if m <= 2 { y + 1 } else { y };
    let secs = ts_ms as i64 / 1000;
    let ss = secs.rem_euclid(60);
    let mi = (secs / 60).rem_euclid(60);
    let hh = (secs / 3600).rem_euclid(24);
    (y as i32, m as u32, d as u32, hh as u32, mi as u32, ss as u32)
}

/// 时间刻度标签：一天/一周显示 HH:MM，一个月显示 MM-DD。
fn fmt_time(ts_ms: u64, range: usize) -> String {
    let (_, mo, d, h, mi, _) = unix_to_utc(ts_ms);
    if range >= 2 {
        format!("{mo:02}-{d:02}")
    } else {
        format!("{h:02}:{mi:02}")
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ============================ 渲染入口 ============================

/// 渲染整个「行情看板」标签页。
pub fn show(ui: &mut Ui, app: &mut crate::ObserverApp) {
    app.viz_ensure();
    let data: &VizData = match &app.viz_cache {
        Some((_, d)) => d,
        None => return,
    };
    // data 借自 app.viz_cache（不可变），view 借自 app.viz_view（可变）—— 两个不同字段，可并存。
    let view: &mut VizView = &mut app.viz_view;

    // 用局部标志承接刷新请求，避免头部闭包同时可变借用 app（与下方 data/view 借用冲突）。
    let mut reload_req = false;
    ui.vertical_centered(|ui| {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.heading("行情数据看板");
                ui.label(
                    RichText::new("识别结果可视化 · 涨红跌绿 · 数据为观察任务持久化记录")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 210)),
                );
            });
            ui.with_layout(
                eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                |ui| {
                    if ui.button("↻ 刷新").clicked() {
                        reload_req = true;
                    }
                },
            );
        });
        ui.add_space(12.0);
    });

    ScrollArea::vertical().show(ui, |ui| {
        summary_card(ui, data);
        ui.add_space(12.0);
        overview_section(ui, view, data);
        ui.add_space(16.0);
        trend_section(ui, view, data);
        ui.add_space(16.0);
    });

    if reload_req {
        app.viz_reload_requested = true;
    }
}

/// 全局汇总卡片：跟踪数、涨跌分布、最近更新、刷新按钮。
fn summary_card(ui: &mut Ui, data: &VizData) {
    let accent = Color32::from_rgb(90, 110, 200);
    Frame::default()
        .fill(Color32::from_rgba_unmultiplied(247, 249, 252, 255))
        .rounding(12.0)
        .stroke(Stroke::new(1.5, accent))
        .inner_margin(Margin::same(14.0))
        .show(ui, |ui| {
            let mut up = 0usize;
            let mut down = 0usize;
            let mut flat = 0usize;
            for (key, _) in &data.latest {
                match (data.latest.get(key), data.prev.get(key)) {
                    (Some((a, _)), Some((b, _))) if a > b => up += 1,
                    (Some((a, _)), Some((b, _))) if a < b => down += 1,
                    _ => flat += 1,
                }
            }

            ui.horizontal(|ui| {
                ui.label(RichText::new("全局概览").strong());
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                stat(ui, "跟踪子弹", &data.bullets.len().to_string(), INK);
                stat(ui, "上涨", &up.to_string(), UP);
                stat(ui, "下跌", &down.to_string(), DOWN);
                stat(ui, "持平/新", &flat.to_string(), FLAT);
                let upd = data
                    .last_ts
                    .map(|t| fmt_time_full(t))
                    .unwrap_or_else(|| "—".into());
                stat(ui, "最近更新", &upd, INK);
            });
            if data.count == 0 {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("尚未发现记录：请先运行「开始观察」，或确认保存目录正确。")
                        .small()
                        .color(Color32::from_rgb(200, 120, 60)),
                );
            }
        });
}

/// 单个统计小项。
fn stat(ui: &mut Ui, label: &str, value: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(
            RichText::new(label)
                .small()
                .color(Color32::from_rgba_unmultiplied(110, 120, 140, 220)),
        );
        ui.label(RichText::new(value).strong().color(color).size(16.0));
    });
    ui.add_space(14.0);
}

/// 完整时间戳（MM-DD HH:MM）。
fn fmt_time_full(ts_ms: u64) -> String {
    let (_, mo, d, h, mi, _) = unix_to_utc(ts_ms);
    format!("{mo:02}-{d:02} {h:02}:{mi:02}")
}

// ============================ 概览视图 ============================

fn overview_section(ui: &mut Ui, view: &mut VizView, data: &VizData) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("最新价格概览").strong().size(16.0));
        ui.with_layout(
            eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
            |ui| {
                let opts: Vec<String> =
                    std::iter::once("全部口径".to_string()).chain(data.calibers.iter().cloned()).collect();
                let cur = view.overview_filter.min(opts.len().saturating_sub(1));
                let mut sel = cur;
                eframe::egui::ComboBox::from_id_source("ov_filter")
                    .selected_text(opts[cur].clone())
                    .show_ui(ui, |ui| {
                        for (i, o) in opts.iter().enumerate() {
                            if ui.selectable_label(cur == i, o).clicked() {
                                sel = i;
                            }
                        }
                    });
                view.overview_filter = sel;
            },
        );
    });
    ui.add_space(6.0);

    let calibers: Vec<String> = if view.overview_filter == 0 {
        data.calibers.clone()
    } else {
        vec![data
            .calibers
            .get(view.overview_filter - 1)
            .cloned()
            .unwrap_or_default()]
    };

    for cal in calibers.iter().filter(|c| !c.is_empty()) {
        let Some(bullets) = data.bullets_by_cal.get(cal) else {
            continue;
        };
        if bullets.is_empty() {
            continue;
        }
        ui.add_space(4.0);
        ui.label(RichText::new(cal).strong().color(Color32::from_rgb(56, 135, 255)));
        ui.separator();

        // 响应式网格：按可用宽度决定列数。
        let ncols = ((ui.available_width() / 240.0).floor().max(1.0) as usize).min(4).max(1);
        eframe::egui::Grid::new(format!("ov_grid_{cal}"))
            .min_col_width(228.0)
            .max_col_width(260.0)
            .show(ui, |ui| {
                for (i, bullet) in bullets.iter().enumerate() {
                    bullet_card(ui, data, cal, bullet);
                    if (i + 1) % ncols == 0 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(8.0);
    }
}

/// 单枚子弹的概览卡片：大字当前价、涨跌差值/幅度、迷你火花线。
fn bullet_card(ui: &mut Ui, data: &VizData, cal: &str, bullet: &str) {
    let key = bkey(cal, bullet);
    let (price, ts) = match data.latest.get(&key) {
        Some(v) => *v,
        None => {
            Frame::default()
                .rounding(10.0)
                .fill(Color32::from_rgba_unmultiplied(245, 246, 248, 255))
                .inner_margin(Margin::same(10.0))
                .show(ui, |ui| {
                    ui.label(RichText::new(bullet).small());
                    ui.label(RichText::new("暂无数据").small().color(FLAT));
                });
            return;
        }
    };

    let (delta, pct, color) = match data.prev.get(&key) {
        Some((prev_p, _)) if *prev_p != 0 => {
            let d = price as i64 - *prev_p as i64;
            let p = (d as f64 / *prev_p as f64) * 100.0;
            let c = if d > 0 { UP } else if d < 0 { DOWN } else { FLAT };
            (Some(d), Some(p), c)
        }
        _ => (None, None, FLAT),
    };

    Frame::default()
        .rounding(10.0)
        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 255))
        .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(225, 228, 235, 255)))
        .inner_margin(Margin::same(10.0))
        .show(ui, |ui| {
            ui.label(RichText::new(bullet).small().color(Color32::from_rgb(70, 80, 100)));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(fmt_price(price)).strong().size(20.0).color(INK));
                ui.with_layout(
                    eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                    |ui| {
                        if let (Some(d), Some(p)) = (delta, pct) {
                            let arrow = if d > 0 { "▲" } else if d < 0 { "▼" } else { "—" };
                            let sign = if d > 0 { "+" } else { "" };
                            ui.label(
                                RichText::new(format!("{arrow} {sign}{} ({sign}{:.1}%)", fmt_price(d.unsigned_abs()), p))
                                    .small()
                                    .color(color)
                                    .strong(),
                            );
                        } else {
                            ui.label(RichText::new("新").small().color(FLAT));
                        }
                    },
                );
            });
            ui.add_space(4.0);
            // 迷你火花线（最近 30 个采集点）
            let pts = data.series.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);
            let tail: Vec<(u64, u64)> = pts.iter().copied().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
            sparkline(ui, &tail, color);
            ui.add_space(2.0);
            ui.label(
                    RichText::new(format!("更新 {}", fmt_time_full(ts)))
                        .small()
                        .color(Color32::from_rgba_unmultiplied(120, 130, 150, 200)),
            );
        });
}

/// 迷你火花线：在固定高度的小画布内绘制最近价格走势。
fn sparkline(ui: &mut Ui, pts: &[(u64, u64)], color: Color32) {
    if pts.len() < 2 {
        ui.add_space(26.0);
        return;
    }
    let (lo, hi) = pts.iter().map(|(_, p)| *p).fold((u64::MAX, 0u64), |(a, b), p| (a.min(p), b.max(p)));
    let span = (hi - lo).max(1);
    let desired = eframe::egui::vec2(ui.available_width().max(80.0), 26.0);
    let (resp, painter) = ui.allocate_painter(desired, Sense::hover());
    let rect = resp.rect;
    let pad = 2.0;
    let w = rect.width() - pad * 2.0;
    let h = rect.height() - pad * 2.0;
    let n = pts.len();
    let screen = |i: usize, p: u64| -> eframe::egui::Pos2 {
        let x = rect.left() + pad + if n == 1 { w / 2.0 } else { w * (i as f32 / (n - 1) as f32) };
        let y = rect.top() + pad + h * (1.0 - (p - lo) as f32 / span as f32);
        eframe::egui::pos2(x, y)
    };
    // 基线
    painter.line_segment(
        [eframe::egui::pos2(rect.left() + pad, rect.bottom() - pad), eframe::egui::pos2(rect.right() - pad, rect.bottom() - pad)],
        Stroke::new(1.0, grid_c()),
    );
    for i in 1..n {
        let a = screen(i - 1, pts[i - 1].1);
        let b = screen(i, pts[i].1);
        let seg_c = if pts[i].1 >= pts[i - 1].1 { UP } else { DOWN };
        painter.line_segment([a, b], Stroke::new(1.5, seg_c));
    }
    // 末点
    let last = screen(n - 1, pts[n - 1].1);
    painter.circle(last, 2.5, color, Stroke::new(0.0, color));
}

// ============================ 趋势图表 ============================

fn trend_section(ui: &mut Ui, view: &mut VizView, data: &VizData) {
    if data.bullets.is_empty() {
        ui.label(RichText::new("暂无数据可绘制趋势图。").small().color(FLAT));
        return;
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new("价格波动趋势").strong().size(16.0));
    });
    ui.add_space(6.0);

    // —— 选择器：口径 + 子弹 + 时间维度 ——
    ui.horizontal_wrapped(|ui| {
        // 口径
        let mut cal = view.chart_cal.min(data.calibers.len().saturating_sub(1));
        let mut new_cal = cal;
        eframe::egui::ComboBox::from_id_source("t_cal")
            .selected_text(data.calibers.get(cal).cloned().unwrap_or_default())
            .show_ui(ui, |ui| {
                for (i, c) in data.calibers.iter().enumerate() {
                    if ui.selectable_label(cal == i, c).clicked() {
                        new_cal = i;
                    }
                }
            });
        if new_cal != cal {
            view.chart_cal = new_cal;
            view.chart_bullet = 0; // 切换口径重置子弹选择
        }
        cal = view.chart_cal;

        // 子弹（依赖口径）
        let bullets = data.calibers.get(cal).and_then(|c| data.bullets_by_cal.get(c)).cloned().unwrap_or_default();
        let bidx = view.chart_bullet.min(bullets.len().saturating_sub(1));
        let mut new_b = bidx;
        eframe::egui::ComboBox::from_id_source("t_bul")
            .selected_text(bullets.get(bidx).cloned().unwrap_or_default())
            .show_ui(ui, |ui| {
                for (i, b) in bullets.iter().enumerate() {
                    if ui.selectable_label(bidx == i, b).clicked() {
                        new_b = i;
                    }
                }
            });
        if new_b != bidx {
            view.chart_bullet = new_b;
        }

        ui.separator();

        // 时间维度分段控件
        let ranges = ["一天", "一周", "一个月"];
        let mut r = view.range.min(2);
        for (i, name) in ranges.iter().enumerate() {
            let on = r == i;
            let resp = ui.selectable_label(on, *name);
            if resp.clicked() {
                view.range = i;
                r = i;
            }
        }
    });
    ui.add_space(8.0);

    let Some((cal, bullet)) = bullet_at(data, view.chart_cal, view.chart_bullet) else {
        return;
    };
    let key = bkey(&cal, &bullet);
    let span = match view.range {
        0 => DAY_MS,
        1 => WEEK_MS,
        _ => MONTH_MS,
    };
    let now = now_ms();
    let pts = data.window(&key, span, now);

    // 统计卡（区间）
    trend_stats(ui, data, &key, &pts, span, now);
    ui.add_space(8.0);

    // 图表区
    let height = 300.0;
    let desired = eframe::egui::vec2(ui.available_width().max(320.0), height);
    let (resp, painter) = ui.allocate_painter(desired, Sense::hover());
    let rect = resp.rect;
    draw_trend(&painter, rect, &pts, view.range, now);

    // 图例
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        legend_dot(ui, UP, "上涨段");
        legend_dot(ui, DOWN, "下跌段");
        legend_dot(ui, Color32::from_rgba_unmultiplied(230, 72, 77, 200), "区间最高");
        legend_dot(ui, Color32::from_rgba_unmultiplied(48, 164, 108, 200), "区间最低");
        legend_dot(ui, INK, "最新");
    });
}

fn legend_dot(ui: &mut Ui, color: Color32, label: &str) {
    let (rresp, painter) = ui.allocate_painter(eframe::egui::vec2(12.0, 12.0), Sense::hover());
    let r = rresp.rect;
    painter.circle(r.center(), 5.0, color, Stroke::new(0.0, color));
    ui.label(RichText::new(label).small().color(Color32::from_rgb(80, 90, 110)));
    ui.add_space(10.0);
}

/// 区间统计：当前 / 最高 / 最低 / 平均 / 区间涨跌幅。
fn trend_stats(ui: &mut Ui, data: &VizData, _key: &str, pts: &[(u64, u64)], _span: u64, _now: u64) {
    Frame::default()
        .rounding(10.0)
        .fill(Color32::from_rgba_unmultiplied(247, 249, 252, 255))
        .inner_margin(Margin::same(12.0))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if pts.is_empty() {
                    ui.label(RichText::new("所选时间窗口内无数据。").small().color(FLAT));
                } else {
                    let cur = pts.last().unwrap().1;
                    let first = pts.first().unwrap().1;
                    let lo = pts.iter().map(|p| p.1).min().unwrap();
                    let hi = pts.iter().map(|p| p.1).max().unwrap();
                    let avg = pts.iter().map(|p| p.1 as f64).sum::<f64>() / pts.len() as f64;
                    let chg = if first != 0 {
                        ((cur as f64 - first as f64) / first as f64) * 100.0
                    } else {
                        0.0
                    };
                    let chg_c = if cur > first { UP } else if cur < first { DOWN } else { FLAT };
                    let _ = data;
                    stat(ui, "当前", &fmt_price(cur), INK);
                    stat(ui, "区间最高", &fmt_price(hi), Color32::from_rgba_unmultiplied(230, 72, 77, 255));
                    stat(ui, "区间最低", &fmt_price(lo), Color32::from_rgba_unmultiplied(48, 164, 108, 255));
                    stat(ui, "平均", &fmt_price(avg.round() as u64), INK);
                    stat(ui, "区间涨跌", &format!("{:+.1}%", chg), chg_c);
                }
            });
        });
}

/// 绘制趋势图：坐标轴、网格、按方向着色的折线、最高/最低标注、最新点。
fn draw_trend(painter: &Painter, rect: eframe::egui::Rect, pts: &[(u64, u64)], range: usize, now: u64) {
    if pts.len() == 0 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "该时间窗口内暂无数据",
            FontId::proportional(14.0),
            FLAT,
        );
        return;
    }

    let pad_l = 56.0;
    let pad_r = 14.0;
    let pad_t = 14.0;
    let pad_b = 26.0;
    let plot = eframe::egui::Rect::from_min_max(
        eframe::egui::pos2(rect.left() + pad_l, rect.top() + pad_t),
        eframe::egui::pos2(rect.right() - pad_r, rect.bottom() - pad_b),
    );

    // 价格范围（含 8% 留白）
    let lo = pts.iter().map(|p| p.1).min().unwrap();
    let hi = pts.iter().map(|p| p.1).max().unwrap();
    let pad = ((hi - lo).max(1) as f32 * 0.08).max(1.0);
    let y_min = (lo as f32 - pad).max(0.0);
    let y_max = hi as f32 + pad;

    // 时间范围：固定窗口 [now - span, now]
    let span = match range {
        0 => DAY_MS,
        1 => WEEK_MS,
        _ => MONTH_MS,
    } as f64;
    let x_max = now as f64;
    let x_min = x_max - span;

    let x_of = |t: u64| -> f32 {
        if (x_max - x_min).abs() < 1.0 {
            plot.right()
        } else {
            plot.left() + plot.width() * ((t as f64 - x_min) / (x_max - x_min)) as f32
        }
    };
    let y_of = |p: u64| -> f32 {
        plot.bottom() - plot.height() * ((p as f32 - y_min) / (y_max - y_min))
    };

    // 网格 + Y 轴价格刻度（4 段）
    for i in 0..=4 {
        let f = i as f32 / 4.0;
        let y = plot.bottom() - plot.height() * f;
        painter.line_segment(
            [eframe::egui::pos2(plot.left(), y), eframe::egui::pos2(plot.right(), y)],
            Stroke::new(1.0, grid_c()),
        );
        let val = (y_min + (y_max - y_min) * f).round() as u64;
        painter.text(
            eframe::egui::pos2(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            fmt_price(val),
            FontId::proportional(11.0),
            Color32::from_rgba_unmultiplied(90, 100, 120, 220),
        );
    }
    // X 轴时间刻度（5 段）
    for i in 0..=4 {
        let f = i as f32 / 4.0;
        let x = plot.left() + plot.width() * f;
        painter.line_segment(
            [eframe::egui::pos2(x, plot.top()), eframe::egui::pos2(x, plot.bottom())],
            Stroke::new(1.0, grid_c()),
        );
        let t = (x_min + (x_max - x_min) * f as f64).max(0.0) as u64;
        painter.text(
            eframe::egui::pos2(x, plot.bottom() + 12.0),
            Align2::CENTER_CENTER,
            fmt_time(t, range),
            FontId::proportional(10.0),
            Color32::from_rgba_unmultiplied(90, 100, 120, 220),
        );
    }
    // 坐标轴
    painter.line_segment(
        [eframe::egui::pos2(plot.left(), plot.top()), eframe::egui::pos2(plot.left(), plot.bottom())],
        Stroke::new(1.2, axis_c()),
    );
    painter.line_segment(
        [eframe::egui::pos2(plot.left(), plot.bottom()), eframe::egui::pos2(plot.right(), plot.bottom())],
        Stroke::new(1.2, axis_c()),
    );

    // 折线（每段按方向着色）
    for i in 1..pts.len() {
        let a = eframe::egui::pos2(x_of(pts[i - 1].0), y_of(pts[i - 1].1));
        let b = eframe::egui::pos2(x_of(pts[i].0), y_of(pts[i].1));
        let c = if pts[i].1 >= pts[i - 1].1 { UP } else { DOWN };
        painter.line_segment([a, b], Stroke::new(2.0, c));
    }

    // 最高 / 最低 标注
    let (hi_i, hi_v) = pts.iter().enumerate().max_by_key(|(_, p)| p.1).unwrap();
    let (lo_i, lo_v) = pts.iter().enumerate().min_by_key(|(_, p)| p.1).unwrap();
    annotate_hline(painter, plot, y_of(hi_v.1), x_of(hi_v.0), &fmt_price(hi_v.1), Color32::from_rgba_unmultiplied(230, 72, 77, 230));
    annotate_hline(painter, plot, y_of(lo_v.1), x_of(lo_v.0), &fmt_price(lo_v.1), Color32::from_rgba_unmultiplied(48, 164, 108, 230));
    let _ = (hi_i, lo_i);

    // 最新点 + 价格标签
    let last = pts.last().unwrap();
    let lp = eframe::egui::pos2(x_of(last.0), y_of(last.1));
    // 最新点涨跌色（较前一观察点）
    let last_c = if pts.len() >= 2 {
        if last.1 >= pts[pts.len() - 2].1 { UP } else { DOWN }
    } else {
        INK
    };
    painter.circle(lp, 4.0, last_c, Stroke::new(1.5, Color32::WHITE));
    painter.text(
        eframe::egui::pos2(lp.x, lp.y - 16.0),
        Align2::CENTER_BOTTOM,
        fmt_price(last.1),
        FontId::proportional(12.0),
        last_c,
    );
}

/// 在图表内绘制一条水平标注线 + 右端价格文本。
fn annotate_hline(painter: &Painter, plot: eframe::egui::Rect, y: f32, at_x: f32, label: &str, color: Color32) {
    // 虚线效果：分段绘制
    let dash = 6.0;
    let gap = 5.0;
    let mut x = plot.left();
    while x < plot.right() {
        let xe = (x + dash).min(plot.right());
        painter.line_segment(
            [eframe::egui::pos2(x, y), eframe::egui::pos2(xe, y)],
            Stroke::new(1.0, color),
        );
        x = xe + gap;
    }
    painter.circle(eframe::egui::pos2(at_x, y), 3.0, color, Stroke::new(0.0, color));
    painter.text(
        eframe::egui::pos2(plot.right() - 2.0, y),
        Align2::RIGHT_CENTER,
        label,
        FontId::proportional(11.0),
        color,
    );
}
