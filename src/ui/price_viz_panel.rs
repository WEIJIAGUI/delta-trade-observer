//! 价格可视化 Tab：口径筛选 → 选择子弹 → 价格折线图。
//!
//! 数据源：观察流程落盘的 `save_path/records.jsonl`（见 [`crate::observe::records_file`]）。
//! 布局（白色主题）：左右分栏——左侧为自适应侧栏（口径下拉 + 子弹单选列表，宽度
//! 随窗口按比例取值且可拖拽分隔条调整），右侧占剩余全部空间，纵向排布标题、
//! 信息带、折线图与底部统计条。图表为 egui Painter 自绘，
//! 不引入额外绘图依赖。时间范围支持「一天 / 一周 / 一个月」三档视图，
//! 窗口起点由数据记录中的最新时间戳动态推导（绝不写死固定值）。记录缓存按文件 mtime 失效重读。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, TimeZone};
use eframe::egui::{self, Align2, Color32, FontId, Margin, RichText, Sense, Stroke, Ui};

use crate::domain::records::{self, PriceRecord};
use crate::observe::records_file;

/// 一天 / 一周 / 一个月对应的窗口时长（毫秒）。
const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

/// 时间范围模式 → 窗口时长（毫秒）。0=一天，1=一周，2=一个月。
/// 窗口起点 = 数据最新时间戳 − 该时长，完全由数据动态推导，不写死固定值。
fn range_duration_ms(range: u8) -> u64 {
    match range {
        1 => 7 * MS_PER_DAY,
        2 => 30 * MS_PER_DAY,
        _ => MS_PER_DAY,
    }
}

/// 时间范围模式 → 中文标签。
fn range_label(range: u8) -> &'static str {
    match range {
        1 => "一周",
        2 => "一个月",
        _ => "一天",
    }
}

/// 依据窗口跨度挑选「整洁」的刻度步长，使标签数落在约 6~10 个。
fn choose_tick_step(span_ms: u64) -> u64 {
    const CANDIDATES: [u64; 7] = [
        3 * 3_600_000,  // 3 小时
        6 * 3_600_000,  // 6 小时
        12 * 3_600_000, // 12 小时
        1 * MS_PER_DAY, // 1 天
        2 * MS_PER_DAY, // 2 天
        7 * MS_PER_DAY, // 1 周
        14 * MS_PER_DAY, // 2 周
    ];
    for &c in CANDIDATES.iter() {
        if span_ms / c <= 10 {
            return c;
        }
    }
    *CANDIDATES.last().unwrap()
}

/// 取 ≥ window_start 的第一个刻度时间戳，并按本地时区对齐到步长边界
/// （整天步长对齐本地 0 点，小时级步长对齐本地 0 点后的整倍数），避免刻度落在任意时刻。
fn first_tick_ms(window_start: u64, step_ms: u64) -> u64 {
    let Some(dt) = Local.timestamp_millis_opt(window_start as i64).single() else {
        return ((window_start + step_ms - 1) / step_ms) * step_ms;
    };
    let day0 = Local
        .with_ymd_and_hms(dt.year(), dt.month(), dt.day(), 0, 0, 0)
        .single()
        .map(|d| d.timestamp_millis() as u64)
        .unwrap_or(window_start);
    if step_ms >= MS_PER_DAY {
        let mut t = day0;
        while t < window_start {
            t += step_ms;
        }
        t
    } else {
        let offset = window_start.saturating_sub(day0);
        day0 + (offset / step_ms + 1) * step_ms
    }
}

/// 把毫秒时间戳格式化为本地时间文本（用于坐标轴刻度与副标题）。
fn fmt_local(ts_ms: u64, with_date: bool) -> String {
    if let Some(dt) = Local.timestamp_millis_opt(ts_ms as i64).single() {
        if with_date {
            dt.format("%m/%d %H:%M").to_string()
        } else {
            dt.format("%H:%M").to_string()
        }
    } else {
        String::new()
    }
}

// —— 白色主题配色 ——
const PANEL: Color32 = Color32::from_rgb(255, 255, 255); // 卡片 / 图表区
const PANEL2: Color32 = Color32::from_rgb(244, 246, 249); // 信息带 / 统计条 / 未选中按钮
const CHART_BG: Color32 = Color32::from_rgb(255, 255, 255); // 绘图区
const ACCENT: Color32 = Color32::from_rgb(236, 140, 64); // 唯一强调色（橙）
const ACCENT_SOFT: Color32 = Color32::from_rgb(255, 241, 230); // 选中项浅橙底
const TEXT: Color32 = Color32::from_rgb(26, 29, 36); // 主文本
const MUTED: Color32 = Color32::from_rgb(107, 114, 128); // 次文本 / 坐标轴

/// 网格线颜色（浅灰）。
fn grid_color() -> Color32 {
    Color32::from_rgba_unmultiplied(237, 240, 244, 255)
}
/// 渐变填充顶部颜色（半透明橙）。
fn fill_top() -> Color32 {
    Color32::from_rgba_unmultiplied(236, 140, 64, 31)
}
/// 渐变填充底部颜色（全透明）。
fn fill_bottom() -> Color32 {
    Color32::from_rgba_unmultiplied(236, 140, 64, 0)
}

/// 价格可视化 Tab 的全部界面状态（由主窗口持有，面板只读写该结构）。
#[derive(Default)]
pub struct VizState {
    /// 选中的口径（与 `config` 的 key 对应）。
    pub caliber: String,
    /// 选中的子弹（数据项）。
    pub bullet: String,
    /// 子弹行复选框状态（默认全选，预留多系列对比展示）。
    pub checks: HashMap<String, bool>,
    /// records.jsonl 缓存（按文件 mtime 失效重读，避免每帧 IO）。
    pub records: Vec<PriceRecord>,
    /// 上次加载时的记录文件 mtime（缓存失效判断）。
    pub mtime: Option<SystemTime>,
    /// 时间范围模式：0=一天，1=一周，2=一个月。窗口起点由数据动态确定（见 `show`）。
    pub range: u8,
    /// 是否弹出「清除数据」确认对话框（点击清除按钮后置位，确认/取消后清零）。
    pub clear_pending: bool,
}

// ---- 记录解析 / 加载见 `crate::domain::records`（解析与加载逻辑已抽到领域层）----
// 本面板通过 `records::load_records` / `records::records_mtime` 读取，不直接解析 JSONL。

/// 价格可视化面板（Idle 态 Tab）—— 独立组件。
///
/// 职责边界：本面板只负责筛选交互与图表绘制；数据来源与落盘完全由
/// [`crate::observe`] 负责，口径 / 子弹列表由调用方（main）从标定配置传入。
pub struct PriceVizPanel;

impl PriceVizPanel {
    pub fn show(
        &self,
        ui: &mut Ui,
        st: &mut VizState,
        calibers: &[String],
        bullets: &[String],
        save_path: &str,
    ) {
        // 缓存失效重读：文件 mtime 变化（新一轮观察落盘）时才重新解析。
        let mtime = records::records_mtime(save_path);
        if mtime != st.mtime {
            st.records = records::load_records(save_path);
            st.mtime = mtime;
        }
        // 首次打开（口径为空）自动选中第一个口径，避免面板一片空白。
        if st.caliber.is_empty() {
            if let Some(first) = calibers.first() {
                st.caliber = first.clone();
            }
        }
        // 子弹选中项兜底：切换口径 / 配置变化后保证选中项始终在列表内。
        if !bullets.is_empty() && !bullets.iter().any(|b| *b == st.bullet) {
            st.bullet = bullets[0].clone();
        }

        // 价格点（时间升序）：先按口径/子弹/有效价筛选候选（不限时间），
        // 取候选中最大时间戳（最新一条记录）作为窗口右端点，向左回退对应时长得到起点。
        // 起点完全由数据时间动态推导，不写死固定值；切换 range 时窗口随之联动。
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut pts: Vec<(u64, u32)> = st
            .records
            .iter()
            .filter(|r| r.caliber == st.caliber && r.bullet == st.bullet && r.price.is_some())
            .map(|r| (r.ts_ms, r.price.unwrap()))
            .collect();
        let data_max = pts.iter().map(|p| p.0).max().unwrap_or(now_ms);
        let window_start = data_max.saturating_sub(range_duration_ms(st.range));
        let window_end = data_max;
        pts.retain(|p| p.0 >= window_start && p.0 <= window_end);
        pts.sort_by_key(|p| p.0);

        // 左右分栏：用 egui 原生 SidePanel + CentralPanel 实现各栏独立纵向布局，
        // 不再用 allocate_ui 固定尺寸 hack（那会使子 Ui 继承水平布局，导致
        // 口径/子弹被横向排布换行堆叠）。侧栏默认宽随窗口比例自适应，且可拖拽调宽。
        egui::SidePanel::left("viz_sidebar")
            .resizable(true)
            .default_width((ui.available_width() * 0.28).clamp(200.0, 300.0))
            .width_range(180.0..=360.0)
            .frame(
                egui::Frame::default()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::same(12.0)),
            )
            .show_inside(ui, |ui| {
                Self::left_column(ui, st, calibers, bullets);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::same(14.0)),
            )
            .show_inside(ui, |ui| {
                Self::right_column(ui, st, &pts, window_start, window_end);
            });

        // 清除数据确认对话框：仅在 clear_pending 置位时弹出，确认后才真正删除文件。
        // egui 0.27 无 Modal，这里用半透明遮罩 + 居中卡片手动实现模态效果。
        if st.clear_pending {
            let sp = save_path.to_string();
            let screen = ui.ctx().screen_rect();
            egui::Area::new("viz_clear_backdrop".into())
                .fixed_pos(screen.min)
                .show(ui.ctx(), |ui| {
                    ui.painter()
                        .rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
                });
            egui::Area::new("viz_clear_confirm".into())
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    egui::Frame::default()
                        .fill(PANEL)
                        .rounding(12.0)
                        .stroke(Stroke::new(1.0, ACCENT))
                        .inner_margin(Margin::same(16.0))
                        .shadow(egui::epaint::Shadow {
                            offset: egui::vec2(0.0, 6.0),
                            blur: 20.0,
                            spread: 0.0,
                            color: Color32::from_rgba_unmultiplied(0, 0, 0, 60),
                        })
                        .show(ui, |ui| {
                            ui.set_min_width(320.0);
                            ui.label(
                                RichText::new("清除数据").size(15.0).color(TEXT).strong(),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("确认清除所有价格数据？")
                                    .size(13.0)
                                    .color(TEXT),
                            );
                            ui.label(
                                RichText::new("将删除 records.jsonl 并清空当前缓存，此操作不可撤销。")
                                    .size(12.0)
                                    .color(MUTED),
                            );
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add(
                                        egui::Button::new("取消")
                                            .min_size(egui::vec2(90.0, 32.0)),
                                    )
                                    .clicked()
                                {
                                    st.clear_pending = false;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("确认清除")
                                                        .color(Color32::WHITE),
                                                )
                                                .fill(ACCENT)
                                                .min_size(egui::vec2(100.0, 32.0)),
                                            )
                                            .clicked()
                                        {
                                            // 删除新路径与旧版 prices/ 兼容路径：仅删新文件时，
                                            // 遗留的 prices/records.jsonl 会在下一帧被 records_mtime
                                            // 命中并重新加载，表现为「清除后旧数据仍在」。
                                            let _ = std::fs::remove_file(records_file(&sp));
                                            let _ = std::fs::remove_file(format!(
                                                "{sp}/prices/records.jsonl"
                                            ));
                                            st.records.clear();
                                            st.checks.clear();
                                            st.mtime = None;
                                            st.clear_pending = false;
                                        }
                                    },
                                );
                            });
                        });
                });
        }
    }

    // ---- 左栏（侧栏）：口径（筛选维度）与子弹（数据项）自上而下纵向排列 ----

    fn left_column(ui: &mut Ui, st: &mut VizState, calibers: &[String], bullets: &[String]) {
        ui.label(RichText::new("筛选").size(14.0).color(TEXT).strong());
        ui.add_space(10.0);

        // 口径：下拉选择，占满侧栏宽度。
        ui.label(RichText::new("口径").size(12.0).color(MUTED).strong());
        ui.add_space(4.0);
        if calibers.is_empty() {
            ui.label(
                RichText::new("未加载标定配置：config/info.json 为空或解析失败")
                    .size(12.0)
                    .color(MUTED),
            );
        } else {
            egui::ComboBox::from_id_source("viz_caliber")
                .width(ui.available_width())
                .selected_text(RichText::new(&st.caliber).color(TEXT))
                .show_ui(ui, |ui| {
                    for c in calibers {
                        ui.selectable_value(&mut st.caliber, c.clone(), c);
                    }
                });
        }

        ui.add_space(12.0);
        ui.label(RichText::new("子弹").size(12.0).color(MUTED).strong());
        ui.add_space(4.0);
        if bullets.is_empty() {
            ui.label(
                RichText::new("该口径下暂无子弹数据（请先在「开始观察」采集）")
                    .size(12.0)
                    .color(MUTED),
            );
        } else {
            // 子弹：单选列表，占满侧栏剩余高度，可滚动。
            egui::ScrollArea::vertical()
                .id_source("viz_bullet_list")
                .show(ui, |ui| {
                    for b in bullets {
                        let sel = *b == st.bullet;
                        let (r, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 30.0),
                            Sense::click(),
                        );
                        let p = ui.painter();
                        if sel {
                            p.rect_filled(r, 6.0, ACCENT_SOFT);
                            p.rect_filled(
                                egui::Rect::from_min_max(
                                    r.left_top(),
                                    egui::pos2(r.left() + 3.0, r.bottom()),
                                ),
                                0.0,
                                ACCENT,
                            );
                        }
                        p.text(
                            egui::pos2(r.left() + 12.0, r.center().y),
                            Align2::LEFT_CENTER,
                            b,
                            FontId::proportional(13.0),
                            if sel { TEXT } else { MUTED },
                        );
                        if resp.clicked() {
                            st.bullet = b.clone();
                        }
                    }
                });
        }

        // 清除数据：点击后弹出确认对话框，确认后才真正删除 records.jsonl 并清空缓存。
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("清除数据").color(ACCENT))
                        .stroke(Stroke::new(1.0, ACCENT))
                        .rounding(6.0)
                        .min_size(egui::vec2(ui.available_width(), 30.0)),
                )
                .clicked()
            {
                st.clear_pending = true;
            }
        });
    }

    // ---- 右栏：标题 + 信息带 + 折线图 + 底部统计条 ----

    fn right_column(
        ui: &mut Ui,
        st: &mut VizState,
        pts: &[(u64, u32)],
        window_start: u64,
        window_end: u64,
    ) {
        // CentralPanel 已提供白色底与内边距，这里直接纵向排布内容。
        ui.set_min_width(ui.available_width());

        // 标题行：左侧标题 + 动态窗口副标题；右侧时间范围分段控件。
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!("{} · 价格波动", st.bullet))
                        .size(17.0)
                        .color(TEXT)
                        .strong(),
                );
                let sub = format!(
                    "价格（哈夫币） · {} 至 {} · 视图：{}",
                    fmt_local(window_start, true),
                    fmt_local(window_end, true),
                    range_label(st.range),
                );
                ui.label(RichText::new(sub).size(12.0).color(MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                Self::range_selector(ui, st);
            });
        });

        ui.add_space(8.0);
        ui.add_space(8.0);

        let avail_w = ui.available_width();
        let chart_h = (ui.available_height() - 64.0).max(180.0);
        let (crect, _) = ui.allocate_exact_size(egui::vec2(avail_w, chart_h), Sense::hover());
        draw_chart(ui, crect, pts, window_start, window_end);

        ui.add_space(8.0);
        summary_bar(ui, pts);
    }

    /// 时间范围分段控件：一天 / 一周 / 一个月，互斥高亮当前选项。
    /// 点击即切换 `st.range`，下一帧 `show` 以新窗口重算并渲染，数据随之联动。
    fn range_selector(ui: &mut Ui, st: &mut VizState) {
        let opts: [(u8, &str); 3] = [(0, "一天"), (1, "一周"), (2, "一个月")];
        for (val, label) in opts {
            let active = st.range == val;
            let resp = ui.add(
                egui::Button::new(
                    RichText::new(label).color(if active { Color32::WHITE } else { TEXT }),
                )
                .fill(if active { ACCENT } else { PANEL2 })
                .stroke(Stroke::new(1.0, ACCENT))
                .rounding(6.0)
                .min_size(egui::vec2(60.0, 28.0)),
            );
            if resp.clicked() {
                st.range = val;
            }
        }
    }
}

// ---- 自绘图表 ----

/// 折线图：白底 + 橙色主线 + 渐变填充 + 峰值标注。
/// x 轴为「数据最新时刻 − 所选窗口时长 → 数据最新时刻」的动态窗口；
/// 刻度步长随窗口跨度自适应（一天→3 小时，一周→1 天，一个月→1 周），并按本地时区对齐。
fn draw_chart(ui: &mut Ui, rect: egui::Rect, pts: &[(u64, u32)], window_start: u64, window_end: u64) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 8.0, CHART_BG);
    if pts.is_empty() {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "暂无该时间段内的价格数据",
            FontId::proportional(13.0),
            MUTED,
        );
        return;
    }

    let (ml, mr, mt, mb) = (44.0, 16.0, 34.0, 34.0);
    let plot = egui::Rect::from_min_max(
        egui::pos2(rect.left() + ml, rect.top() + mt),
        egui::pos2(rect.right() - mr, rect.bottom() - mb),
    );
    // 退化保护：窗口过窄导致绘图区非正尺寸时，给出提示并退出，避免后续几何计算崩溃。
    if plot.width() < 1.0 || plot.height() < 1.0 {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "绘图区域过小，请放大窗口",
            FontId::proportional(13.0),
            MUTED,
        );
        return;
    }
    // x 轴域：窗口起点（window_start）→ 窗口终点（window_end，数据最新时刻）。
    let x0 = window_start as f64;
    let x1 = (window_end.max(window_start + 1)) as f64;
    let xs = |ts: u64| -> f32 {
        let t = ((ts as f64 - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0);
        plot.left() + t as f32 * plot.width()
    };

    // —— 价格轴：聚焦「变化最大」的区间，非等差刻度 ——
    // 找到相邻点中价格变化（绝对值）最大的一段，以其价格中点为中心展开，
    // 焦点区间占绘图区中部主体，区间外的极值被压缩到上下细边带，把细节留给波动最大的部分。
    let vals: Vec<f32> = pts.iter().map(|p| p.1 as f32).collect();
    let gmin = vals.iter().cloned().fold(f32::INFINITY, f32::min);
    let gmax = vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut focus_lo = gmin;
    let mut focus_hi = gmax;
    let mut focus_seg: Option<(u64, u64)> = None;
    if pts.len() >= 2 {
        let mut best = 0usize;
        let mut best_delta = 0.0f32;
        for i in 0..pts.len() - 1 {
            let d = (pts[i + 1].1 as f32 - pts[i].1 as f32).abs();
            if d > best_delta {
                best_delta = d;
                best = i;
            }
        }
        let mid = (pts[best].1 as f32 + pts[best + 1].1 as f32) / 2.0;
        let half = (best_delta * 1.5).max((gmax - gmin) * 0.15).max(1.0);
        focus_lo = mid - half;
        focus_hi = mid + half;
        // 限制焦点区间不过度退化成一条线。
        if focus_hi - focus_lo < 1.0 {
            focus_hi = focus_lo + 1.0;
        }
        focus_seg = Some((pts[best].0, pts[best + 1].0));
    }

    // 上下各留细边带（占绘图区高度比例）展示区间外的极值；焦点区间占中部主体。
    let margin = 0.12_f32;
    let band_top = plot.top() + plot.height() * margin;
    let band_bottom = plot.bottom() - plot.height() * margin;
    let band_h = (band_bottom - band_top).max(1.0);
    let frange = (focus_hi - focus_lo).max(1.0);

    // 价格 -> y 的非线性映射：焦点区间内线性细致展开，区间外压缩到上下边带。
    let ymap = |v: f32| -> f32 {
        if v >= focus_lo && v <= focus_hi {
            band_bottom - (v - focus_lo) / frange * band_h
        } else if v > focus_hi {
            let span = (gmax - focus_hi).max(1.0);
            let frac = ((v - focus_hi) / span).clamp(0.0, 1.0);
            band_top - frac * (band_top - plot.top()).max(0.0)
        } else {
            let span = (focus_lo - gmin).max(1.0);
            let frac = ((focus_lo - v) / span).clamp(0.0, 1.0);
            band_bottom + frac * (plot.bottom() - band_bottom).max(0.0)
        }
    };

    // 焦点区间内的精细刻度（在焦点区间均匀细分，而非全图等差）。
    let step = nice_ceil(frange / 6.0);
    let mut tick_vals: Vec<f32> = Vec::new();
    let mut tv = (focus_lo / step).floor() * step;
    while tv <= focus_hi + step * 0.5 {
        tick_vals.push(tv);
        tv += step;
    }
    // 落在焦点区间外的全局极值，单独以强调色标注。
    let mut extrema: Vec<f32> = Vec::new();
    if gmax > focus_hi + 1e-3 {
        extrema.push(gmax);
    }
    if gmin < focus_lo - 1e-3 {
        extrema.push(gmin);
    }

    // 先画竖直网格（时间）与水平网格（价格）+ 刻度标签。
    // 时间刻度：步长随窗口跨度自适应，并对齐到本地时区边界（整天→本地 0 点，
    // 小时级→本地 0 点后的整倍数），标签含日期当且仅当窗口跨越多天。
    let span_ms = window_end.saturating_sub(window_start);
    let step_ms = choose_tick_step(span_ms);
    let first_tick = first_tick_ms(window_start, step_ms);
    let with_date = span_ms > 2 * MS_PER_DAY;
    let mut k = first_tick;
    let mut safety: u32 = 0;
    while k <= window_end && safety < 256 {
        let x = xs(k);
        p.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1.0, grid_color()),
        );
        p.text(
            egui::pos2(x, plot.bottom() + 12.0),
            Align2::CENTER_CENTER,
            fmt_local(k, with_date),
            FontId::proportional(11.0),
            MUTED,
        );
        k += step_ms;
        safety += 1;
    }
    for &v in &tick_vals {
        let y = ymap(v);
        p.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1.0, grid_color()),
        );
        p.text(
            egui::pos2(plot.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            tick_label(v),
            FontId::proportional(11.0),
            MUTED,
        );
    }
    for &v in &extrema {
        let y = ymap(v);
        p.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(236, 140, 64, 110)),
        );
        p.text(
            egui::pos2(plot.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            tick_label(v),
            FontId::proportional(11.0),
            ACCENT,
        );
    }

    // 变化最大区间高亮（浅橙竖直带），直观呼应「聚焦波动最大处」。
    if let Some((tsa, tsb)) = focus_seg {
        let hx0 = xs(tsa).min(xs(tsb));
        let hx1 = xs(tsa).max(xs(tsb));
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(hx0, plot.top()),
                egui::pos2(hx1, plot.bottom()),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(236, 140, 64, 16),
        );
    }

    // 折线下方渐变填充（顶亮底透明，纵向条带拼接）。
    let mut mesh = egui::Mesh::default();
    for (i, (ts, v)) in pts.iter().enumerate() {
        mesh.colored_vertex(egui::pos2(xs(*ts), ymap(*v as f32)), fill_top());
        mesh.colored_vertex(egui::pos2(xs(*ts), plot.bottom()), fill_bottom());
        if i > 0 {
            let a = ((i - 1) * 2) as u32;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a + 1, a + 3, a + 2);
        }
    }
    p.add(egui::Shape::mesh(mesh));

    // 主折线 + 数据点
    let line_pts: Vec<egui::Pos2> = pts
        .iter()
        .map(|(ts, v)| egui::pos2(xs(*ts), ymap(*v as f32)))
        .collect();
    p.add(egui::Shape::line(
        line_pts.clone(),
        Stroke::new(2.0, ACCENT),
    ));

    let n = pts.len();
    let peak = max_index(pts);
    let label_every = ((n as f32) / 12.0).ceil().max(1.0) as usize;
    for (i, pos) in line_pts.iter().enumerate() {
        p.circle_filled(*pos, 3.0, ACCENT);
        if i != peak && (i % label_every == 0 || i == n - 1) {
            p.text(
                egui::pos2(pos.x, pos.y - 8.0),
                Align2::CENTER_BOTTOM,
                format!("{}", pts[i].1),
                FontId::proportional(11.0),
                TEXT,
            );
        }
    }

    // 峰值标注框（橙色圆角框 + 「峰值 / 数值」两行）+ 竖直参考线。
    let (pts_ts, pv) = pts[peak];
    let px = xs(pts_ts);
    let py = ymap(pv as f32);
    p.line_segment(
        [egui::pos2(px, py), egui::pos2(px, plot.bottom())],
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(236, 140, 64, 60)),
    );
    let (bw, bh) = (54.0, 36.0);
    // 排序边界，确保 min <= max（窄窗下 chart rect 可能退化，避免 clamp 断言崩溃）。
    let bx_lo = plot.left().min(plot.right() - bw);
    let bx_hi = plot.left().max(plot.right() - bw);
    let bx = (px - bw / 2.0).clamp(bx_lo, bx_hi);
    let by = (py - bh - 10.0).max(plot.top() + 2.0);
    let box_r = egui::Rect::from_min_size(egui::pos2(bx, by), egui::vec2(bw, bh));
    p.rect_filled(box_r, 6.0, ACCENT);
    p.text(
        egui::pos2(box_r.center().x, box_r.top() + 9.0),
        Align2::CENTER_CENTER,
        "峰值",
        FontId::proportional(10.0),
        Color32::from_rgba_unmultiplied(255, 255, 255, 230),
    );
    p.text(
        egui::pos2(box_r.center().x, box_r.bottom() - 11.0),
        Align2::CENTER_CENTER,
        format!("{pv}"),
        FontId::proportional(13.0),
        Color32::WHITE,
    );

    // x 轴标题
    p.text(
        egui::pos2(plot.center().x, rect.bottom() - 8.0),
        Align2::CENTER_CENTER,
        "时间（数据最新时刻回退所选窗口）",
        FontId::proportional(11.0),
        MUTED,
    );
}

/// 底部统计条：当前价格 / 最高 / 最低 / 波动幅度。
fn summary_bar(ui: &mut Ui, pts: &[(u64, u32)]) {
    let (last, mx, mn) = if pts.is_empty() {
        (None, None, None)
    } else {
        let vals: Vec<u32> = pts.iter().map(|p| p.1).collect();
        (
            Some(*vals.last().unwrap()),
            Some(*vals.iter().max().unwrap()),
            Some(*vals.iter().min().unwrap()),
        )
    };
    let amp = match (mx, mn) {
        (Some(a), Some(b)) => Some(a as i32 - b as i32),
        _ => None,
    };
    let (amp_txt, amp_col) = match amp {
        Some(v) if v > 0 => (format!("+{v}"), ACCENT),
        Some(v) if v < 0 => (format!("{v}"), Color32::from_rgb(110, 170, 255)),
        Some(_) => ("0".to_string(), MUTED),
        None => ("—".to_string(), MUTED),
    };
    egui::Frame::default()
        .fill(PANEL2)
        .rounding(8.0)
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("当前价格").size(12.0).color(MUTED));
                ui.label(RichText::new(fmt_opt(last)).size(15.0).color(TEXT).strong());
                ui.add_space(14.0);
                ui.label(RichText::new("●").size(9.0).color(MUTED));
                ui.label(RichText::new("最高").size(12.0).color(MUTED));
                ui.label(RichText::new(fmt_opt(mx)).size(13.0).color(TEXT));
                ui.add_space(10.0);
                ui.label(RichText::new("最低").size(12.0).color(MUTED));
                ui.label(RichText::new(fmt_opt(mn)).size(13.0).color(TEXT));
                ui.add_space(10.0);
                ui.label(RichText::new("●").size(9.0).color(MUTED));
                ui.label(RichText::new("波动幅度").size(12.0).color(MUTED));
                ui.label(RichText::new(amp_txt).size(13.0).color(amp_col));
            });
        });
}

// ---- 小工具 ----

/// 取「不小于 v 的整洁数值」（1 / 2 / 2.5 / 5 / 10 × 10^k），用于坐标轴刻度。
fn nice_ceil(v: f32) -> f32 {
    if v <= 0.0 {
        return 1.0;
    }
    let exp = 10f32.powf(v.log10().floor());
    let m = v / exp;
    let m = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 2.5 {
        2.5
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    m * exp
}

/// 最大值对应的下标（并列取最后一个，与「最近一次达到峰值」语义一致）。
fn max_index(pts: &[(u64, u32)]) -> usize {
    let mut best = 0usize;
    for (i, p) in pts.iter().enumerate() {
        if p.1 > pts[best].1 {
            best = i;
        }
    }
    best
}

/// y 轴刻度文本：整数直出，带小数保留一位，大数以「万」为单位压缩。
fn tick_label(v: f32) -> String {
    if v >= 10_000.0 {
        format!("{:.1}万", v / 10_000.0)
    } else if (v - v.round()).abs() < 0.01 {
        format!("{}", v.round() as i32)
    } else {
        format!("{v:.1}")
    }
}

/// 可选数值展示：无数据时显示占位符「—」。
fn fmt_opt(v: Option<u32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "—".into())
}
