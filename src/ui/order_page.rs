//! 顺序调整页（全屏页面切换，取代原模态弹窗）。
//!
//! 布局：左栏 20% 宽展示口径列表，右栏 80% 展示所选口径的子弹网格（每行最多 3 张）。
//! 两侧均沿用「按下抬起 / 移动跟随 / 松开吸附」三段动画的拖拽排序逻辑，以 [`SCOPE_*`]
//! 区分拖拽作用域（共用 `ObserverApp` 的同一套拖拽状态），互不干扰；
//! 点击左栏口径项，右栏联动切换展示对应子弹。
//! 视觉风格参照游戏交易行截图（深藏青背景 + 暗色卡片 + 选中高亮）。

use eframe::egui::{
    self, Align, Color32, CursorIcon, Id, Key, Layout, Order, Rect, RichText, Sense, Stroke,
    TextureHandle, Ui,
};
use crate::ObserverApp;
use std::path::PathBuf;

/// 拖拽作用域：左栏口径列表（左右两栏共用拖拽状态，以此区分）。
const SCOPE_CALIBERS: u8 = 0;
/// 拖拽作用域：右栏子弹网格。
const SCOPE_BULLETS: u8 = 1;

// —— 暗色主题（参照游戏交易行截图配色）——
/// 页面背景（深藏青）。
const BG: Color32 = Color32::from_rgb(10, 18, 32);
/// 左栏背景。
const PANEL_BG: Color32 = Color32::from_rgb(13, 22, 38);
/// 卡片背景。
const CARD_BG: Color32 = Color32::from_rgb(19, 32, 50);
/// 左栏选中项高亮背景。
const SEL_BG: Color32 = Color32::from_rgb(32, 48, 73);
/// 卡片描边。
const CARD_STROKE: Color32 = Color32::from_rgb(37, 54, 77);
/// 主文字（浅蓝白）。
const TEXT: Color32 = Color32::from_rgb(212, 222, 238);
/// 次要文字。
const TEXT_DIM: Color32 = Color32::from_rgb(112, 128, 152);
/// 强调色（拖拽落点 / 高亮）。
const ACCENT: Color32 = Color32::from_rgb(56, 135, 255);

/// 子弹卡片尺寸：严格等于贴图原生像素尺寸（保证显示比例正确、无拉伸 / 变形）。
/// `assets/bullets` 下所有贴图均为 368×128，卡片据此逐张取各自贴图尺寸绘制。
/// 贴图缺失时回退到该默认尺寸（比例与正常贴图一致，避免布局塌陷）。
const BULLET_FALLBACK: egui::Vec2 = egui::vec2(368.0, 128.0);
/// 口径列表项高度（左栏垂直列表，紧凑）。
const CAL_ITEM_H: f32 = 34.0;
/// 卡片间距（紧凑）。
const CARD_GAP: f32 = 6.0;
/// 吸附高亮持续时长（秒）。
const SNAP_DUR: f32 = 0.3;

/// 顺序调整页主入口：暗色背景 + 顶部条 + 左右分栏（在 Idle 主 CentralPanel 内调用）。
pub(crate) fn show(ui: &mut Ui, app: &mut ObserverApp) {
    let page_rect = ui.max_rect();
    ui.painter().rect_filled(page_rect, 0.0, BG);

    // 对账与选中兜底（无拖拽进行时执行，避免打乱拖拽中的列表）。
    if app.drag_src.is_none() {
        reconcile_caliber(app);
        ensure_selection(app);
        if let Some(cal) = app.order_selected_caliber.clone() {
            reconcile_bullet(app, &cal);
        }
    }

    top_bar(ui, app);
    ui.add_space(2.0);

    // 左右分栏：左 20% 口径列表，右 80% 所选口径的子弹网格。
    let left_w = (page_rect.width() * 0.2).max(200.0);
    egui::SidePanel::left("order_caliber_panel")
        .resizable(false)
        .exact_width(left_w)
        .frame(egui::Frame::default().fill(PANEL_BG).inner_margin(egui::Margin::same(10.0)))
        .show_inside(ui, |ui| caliber_list(ui, app));
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(BG).inner_margin(egui::Margin::same(14.0)))
        .show_inside(ui, |ui| bullet_grid(ui, app));

    // Esc 返回主面板。
    if ui.ctx().input(|i| i.key_pressed(Key::Escape)) {
        back_to_main(app);
    }
}

/// 顶部条：标题 + 返回按钮。
fn top_bar(ui: &mut Ui, app: &mut ObserverApp) {
    ui.horizontal(|ui| {
        ui.heading(RichText::new("处理顺序调整").strong().color(TEXT));
        ui.label(
            RichText::new("左侧拖拽调整口径顺序、点击查看子弹；右侧拖拽调整该口径内子弹顺序（左/上 = 先处理）。")
                .small()
                .color(TEXT_DIM),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let b = egui::Button::new(RichText::new("✕ 返回主面板").color(TEXT))
                .fill(CARD_BG)
                .rounding(8.0)
                .stroke(Stroke::new(1.0, CARD_STROKE));
            if ui.add(b).clicked() {
                back_to_main(app);
            }
        });
    });
}

/// 左栏：口径垂直列表（整列可拖拽排序，点击选中 → 右栏联动）。
fn caliber_list(ui: &mut Ui, app: &mut ObserverApp) {
    ui.label(RichText::new("口径").strong().color(TEXT));
    ui.add_space(2.0);

    if app.buy_caliber_order.is_empty() {
        ui.label(RichText::new("暂无口径").small().color(TEXT_DIM));
        return;
    }

    let mut order = app.buy_caliber_order.clone();
    let selected = app.order_selected_caliber.clone();
    drag_reorder(
        ui,
        app,
        "order_page_calibers",
        SCOPE_CALIBERS,
        &mut order,
        |app, items| {
            app.buy_caliber_order = items.to_vec();
            crate::persist_buy_order(items);
        },
        1, // 单列 = 垂直列表（宽度占满左栏）
        selected.as_deref(),
        |app, label| {
            // 点击口径项：右栏联动切换。
            app.order_selected_caliber = Some(label.to_string());
        },
    );
}

/// 右栏：所选口径的子弹网格（每行最多 3 张、网格布局，可拖拽排序）。
fn bullet_grid(ui: &mut Ui, app: &mut ObserverApp) {
    let cal = match &app.order_selected_caliber {
        Some(c) => c.clone(),
        None => {
            ui.label(RichText::new("请在左侧选择一个口径").color(TEXT_DIM));
            return;
        }
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("「{}」子弹顺序", cal)).strong().color(TEXT));
        ui.label(
            RichText::new("拖拽子弹卡片调整先后（左 = 先处理）。")
                .small()
                .color(TEXT_DIM),
        );
    });
    ui.add_space(4.0);

    let bullets: Vec<String> = app
        .buy_bullet_order
        .get(&cal)
        .cloned()
        .unwrap_or_default();
    if bullets.is_empty() {
        ui.label(RichText::new("该口径暂无子弹").color(TEXT_DIM));
        return;
    }

    let mut order = bullets;
    let cal2 = cal.clone();
    drag_reorder(
        ui,
        app,
        format!("order_page_bullets_{}", cal).as_str(),
        SCOPE_BULLETS,
        &mut order,
        move |app, items| {
            app.buy_bullet_order.insert(cal2.clone(), items.to_vec());
            crate::persist_bullet_order(&app.buy_bullet_order);
        },
        3, // 每行最多 3 张（网格布局，卡片尺寸 = 贴图原生尺寸）
        None,
        |_, _| {},
    );
}

/// 通用可拖拽重排列表：单列（垂直列表，宽度占满）或网格（固定列数）。
///
/// - `scope`：拖拽作用域（`SCOPE_CALIBERS` / `SCOPE_BULLETS`），左右两栏共用拖拽状态时以此互斥。
/// - `cols`：1 = 垂直列表（宽度占满列宽）；>1 = 网格（固定列数上限，窄屏自动降列，最多 3 行）。
/// - 子弹卡片尺寸严格等于各贴图原生像素（由 `load_bullet_texture` 取得），无缩放 / 拉伸 / 变形。
/// - `on_click`：点击条目回调（左栏口径选中用；网格传空闭包）。
/// - 三段动画：按下抬起（lift）、移动跟随（浮动预览每帧跟随指针）、松开吸附（落点高亮淡出）。
fn drag_reorder(
    ui: &mut Ui,
    app: &mut ObserverApp,
    id_prefix: &str,
    scope: u8,
    items: &mut Vec<String>,
    on_commit: impl FnOnce(&mut ObserverApp, &[String]),
    cols: usize,
    selected: Option<&str>,
    on_click: impl Fn(&mut ObserverApp, &str),
) {
    let ctx = ui.ctx().clone();
    let released = ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary));
    let down = ctx.input(|i| i.pointer.primary_down());
    let mine = app.drag_scope == Some(scope); // 本列表是否拥有当前拖拽

    // 实时跟随：拖拽进行中每帧更新指针位置（浮动预览平滑跟随鼠标）。
    if mine && app.drag_src.is_some() {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            app.drag_pointer = Some(p);
        }
    }

    // 松开：落定重排并持久化（松开动画 = 落点高亮淡出）。仅处理属于本列表的拖拽。
    if released && mine {
        if let (Some(src), Some(dst)) = (app.drag_src, app.drop_dst) {
            if src != dst && dst < items.len() {
                let moved = items.remove(src);
                items.insert(dst, moved);
                app.drag_snap_target = Some(dst);
                app.drag_snap_t0 = Some(ctx.input(|i| i.time));
                on_commit(app, items);
            }
        }
        reset_drag(app);
    } else if !down && app.drag_src.is_some() && mine {
        // 指针离窗 / 取消：复位拖拽态，不重排。
        reset_drag(app);
    }

    // 清除过期的吸附高亮。
    if let Some(t0) = app.drag_snap_t0 {
        if ctx.input(|i| i.time) - t0 > SNAP_DUR as f64 {
            app.drag_snap_target = None;
            app.drag_snap_t0 = None;
        }
    }

    // —— 布局：单列（垂直）或网格（固定列数），逐项显式定位（顶部对齐、无基线偏移）——
    // 子弹卡片尺寸严格等于各贴图原生像素；口径项占满列宽、高度固定。
    // 网格列数 = 需求列数与「可容纳列数」的较小值（窄屏自动降列，避免横向溢出 / 滚动条）。
    let n = items.len();
    let avail_w = ui.available_width();

    // 1) 逐张计算卡片尺寸：子弹取贴图原生尺寸，口径占满列宽。
    let mut sizes: Vec<egui::Vec2> = Vec::with_capacity(n);
    for label in items.iter() {
        let s = if scope == SCOPE_BULLETS {
            if let Some(tex) = app.bullet_textures.get(label) {
                tex.size_vec2()
            } else if let Some(tex) = load_bullet_texture(ui, app, label) {
                tex.size_vec2()
            } else {
                BULLET_FALLBACK
            }
        } else {
            egui::vec2(avail_w, CAL_ITEM_H)
        };
        sizes.push(s);
    }

    let max_w = sizes.iter().map(|s| s.x).fold(1.0f32, f32::max);
    let cols_req = cols.max(1);
    let fit = (((avail_w + CARD_GAP) / (max_w + CARD_GAP)).floor()).max(1.0) as usize;
    let cols = fit.min(cols_req); // 窄屏时以可容纳列数为准
    let rows = ((n + cols - 1) / cols).max(1);

    // 2) 逐列最大宽 / 逐行最大高（行优先分配），保证各卡片以其原生尺寸顶部/左对齐。
    let mut col_w = vec![0.0f32; cols];
    let mut row_h = vec![0.0f32; rows];
    for i in 0..n {
        let c = i % cols;
        let r = i / cols;
        col_w[c] = col_w[c].max(sizes[i].x);
        row_h[r] = row_h[r].max(sizes[i].y);
    }
    let mut x_off = vec![0.0f32; cols];
    for c in 1..cols {
        x_off[c] = x_off[c - 1] + col_w[c - 1] + CARD_GAP;
    }
    let mut y_off = vec![0.0f32; rows];
    for r in 1..rows {
        y_off[r] = y_off[r - 1] + row_h[r - 1] + CARD_GAP;
    }
    let content_w = if cols > 0 { x_off[cols - 1] + col_w[cols - 1] } else { 0.0 };
    let content_h = if rows > 0 { y_off[rows - 1] + row_h[rows - 1] } else { 0.0 };

    if n > 0 {
        // 网格整体居中（剩余空间均分左右，避免宽面板下右侧大片留白）。
        let start_x = ui.cursor().min.x + ((avail_w - content_w) / 2.0).max(0.0);
        let start_y = ui.cursor().min.y;
        for i in 0..n {
            let c = i % cols;
            let r = i / cols;
            let x = start_x + x_off[c];
            let y = start_y + y_off[r];
            let cell = Rect::from_min_size(egui::pos2(x, y), sizes[i]);
            let label = items[i].clone();
            ui.allocate_ui_at_rect(cell, |ui| {
                let resp = card_frame(
                    ui,
                    app,
                    scope,
                    i,
                    sizes[i],
                    mine,
                    selected == Some(label.as_str()),
                    |ui, app| card_inner(ui, app, scope, i, &label),
                );
                // 点击选中（无拖拽发生时才生效）。
                if resp.clicked() && app.drag_scope.is_none() {
                    on_click(app, &label);
                }
            });
        }
        let total = Rect::from_min_size(egui::pos2(start_x, start_y), egui::vec2(content_w, content_h));
        ui.advance_cursor_after_rect(total);
    }

    // 浮动预览：跟随指针（移动动画），含抬起位移；实时跟随鼠标。
    // 预览卡尺寸 = 被拖拽卡片的贴图原生尺寸（与网格卡一致，无缩放）。
    if mine {
        if let (Some(src), Some(pos)) = (app.drag_src, app.drag_pointer) {
            if src < sizes.len() {
                let label = items[src].clone();
                let card_w = sizes[src].x;
                let card_h = sizes[src].y;
                let lift = ctx
                    .animate_value_with_time(
                        Id::new(format!("{}_lift_{}", id_prefix, src)),
                        1.0,
                        0.12,
                    )
                    as f32;
                // 卡片中心锚定光标，并随抬起动画略微上移。
                let off = egui::vec2(-card_w / 2.0, -card_h / 2.0 - 6.0 - 6.0 * lift);
                egui::Area::new(Id::new(format!("{}_float_{}", id_prefix, src)))
                    .fixed_pos(pos + off)
                    .order(Order::Foreground)
                    .show(&ctx, |ui| {
                        ui.set_min_width(card_w);
                        card_frame(
                            ui,
                            app,
                            scope,
                            src,
                            sizes[src],
                            false,
                            false,
                            |ui, app| card_inner(ui, app, scope, src, &label),
                        );
                    });
            }
        }
    }
}

/// 卡片框组件：固定尺寸外框，承载拖拽/落点/吸附/选中状态与指针光标。
///
/// - 整张卡片框 `Sense::drag()`：按住任意处发起拖拽；悬停时光标变小手。
/// - 返回 `Response` 供调用方处理点击（左栏口径选中）。
fn card_frame(
    ui: &mut Ui,
    app: &mut ObserverApp,
    scope: u8,
    index: usize,
    size: egui::Vec2,
    active: bool,
    selected: bool,
    content: impl FnOnce(&mut Ui, &mut ObserverApp),
) -> egui::Response {
    let is_source = active && app.drag_src == Some(index);
    let is_drop = active && app.drop_dst == Some(index) && !is_source;
    let snap = snap_intensity(ui, app, index);

    let resp = ui.allocate_response(size, Sense::click_and_drag());
    let rect = resp.rect;

    // 光标：进入卡片区域变为小手。
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    // 拖拽起始：按住任意处发起拖拽（移动超过阈值才真正开始），并记录作用域与指针位置。
    // 注意：此处**不能**用 `active`（`= mine = drag_scope == Some(scope)`）拦截——
    // 起始帧 `drag_scope` 尚为 `None`，若加 `active &&` 则拖拽永远无法发起。
    // 仅在当前没有进行中的拖拽时允许发起新的拖拽（单指针下旧拖拽未结束不会新发起）。
    if resp.drag_started() && app.drag_src.is_none() {
        app.drag_src = Some(index);
        app.drag_scope = Some(scope);
        if let Some(p) = ui.ctx().input(|i| i.pointer.interact_pos()) {
            app.drag_pointer = Some(p);
        }
    }
    // 落点判定：本列表拖拽进行中且指针悬停于非源卡片框内。
    if active && mine_drag(app, scope) && app.drag_src != Some(index) {
        if let Some(p) = ui.ctx().input(|i| i.pointer.interact_pos()) {
            if rect.contains(p) {
                app.drop_dst = Some(index);
            }
        }
    }

    // 子弹卡片为纯贴图（无卡片底色 / 边框 / 装饰），仅口径项保留暗色卡片框与选中高亮。
    let painter = ui.painter();
    if scope == SCOPE_CALIBERS {
        let radius = 8.0;
        let fill = if is_source {
            Color32::from_rgba_unmultiplied(CARD_BG.r(), CARD_BG.g(), CARD_BG.b(), 80)
        } else if selected {
            SEL_BG
        } else {
            CARD_BG
        };
        let stroke = if is_drop {
            Stroke::new(2.5, ACCENT)
        } else if snap > 0.0 {
            Stroke::new(
                2.0,
                Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), (90.0 + 130.0 * snap) as u8),
            )
        } else if selected {
            Stroke::new(1.5, ACCENT.gamma_multiply(0.55))
        } else {
            Stroke::new(1.0, CARD_STROKE)
        };
        painter.rect_filled(rect, radius, fill);
        painter.rect_stroke(rect, radius, stroke);
    }

    // 内部内容：口径项留内边距 10；子弹项使用整格（无边框，贴图直接充满单元格）。
    let inner = if scope == SCOPE_CALIBERS {
        rect.shrink(10.0)
    } else {
        rect
    };
    ui.allocate_ui_at_rect(inner, |ui| {
        content(ui, app);
    });
    resp
}

/// 卡片内容：
/// - 子弹卡片（`scope == SCOPE_BULLETS`）：贴图按原生像素 1:1 绘制（卡片尺寸已等于贴图尺寸，
///   因此无缩放 / 拉伸 / 变形），无序号 / 名称 / 边框 / 装饰；贴图缺失则留空（不绘制占位块）。
/// - 口径卡片（`SCOPE_CALIBERS`）：仅居中展示口径名（已移除顺序序号）。
fn card_inner(ui: &mut Ui, app: &mut ObserverApp, scope: u8, _index: usize, label: &str) {
    if scope == SCOPE_BULLETS {
        let area = ui.max_rect();
        if let Some(tex) = load_bullet_texture(ui, app, label) {
            let native = tex.size_vec2();
            // 卡片单元格尺寸 == 贴图原生尺寸 → 以原生尺寸绘制，严格 1:1、零缩放。
            ui.put(area, egui::Image::new((tex.id(), native)));
        }
    } else {
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(label).strong().color(TEXT));
        });
    }
}

/// 定位 `assets/bullets` 目录：从 `current_exe` 向上追溯，命中即返回（与 `buttons.rs` 资源定位一致）。
fn bullets_assets_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..8 {
        let Some(d) = dir else { break };
        let cand = d.join("assets").join("bullets");
        if cand.is_dir() {
            return Some(cand);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

/// 子弹贴图完整路径：`assets/bullets/<子弹key>.png`（key 对应 `config.rs::build_config` 的子弹名）。
fn bullets_texture_path(key: &str) -> Option<PathBuf> {
    bullets_assets_dir().map(|d| d.join(format!("{key}.png")))
}

/// 惰性加载子弹贴图：命中缓存直接返回；否则从磁盘解码 PNG → egui 纹理并写入缓存。
/// 返回 `None` 表示贴图不存在或解码失败（调用方降级为纯文本）。
fn load_bullet_texture(ui: &mut Ui, app: &mut ObserverApp, key: &str) -> Option<TextureHandle> {
    if let Some(tex) = app.bullet_textures.get(key) {
        return Some(tex.clone());
    }
    let path = bullets_texture_path(key)?;
    let img = image::open(&path).ok()?;
    let img = img.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let pixels = img.into_raw();
    let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels);
    let tex = ui
        .ctx()
        .load_texture(format!("bullet_{key}"), ci, egui::TextureOptions::LINEAR);
    app.bullet_textures.insert(key.to_string(), tex.clone());
    Some(tex)
}

/// 当前拖拽是否属于指定作用域。
fn mine_drag(app: &ObserverApp, scope: u8) -> bool {
    app.drag_scope == Some(scope) && app.drag_src.is_some()
}

/// 松开吸附高亮强度（0=无，1=最强），随时间线性淡出。
fn snap_intensity(ui: &Ui, app: &ObserverApp, index: usize) -> f32 {
    if app.drag_snap_target == Some(index) {
        let t0 = app.drag_snap_t0.unwrap_or(0.0);
        (1.0 - (ui.ctx().input(|i| i.time) - t0).max(0.0) as f32 / SNAP_DUR).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// 口径顺序与校准配置对账：剔除已移除口径、补入新增口径（按名排序）。
fn reconcile_caliber(app: &mut ObserverApp) {
    let keys: Vec<String> = app.info.keys().cloned().collect();
    app.buy_caliber_order.retain(|c| keys.contains(c));
    let mut missing: Vec<String> = keys
        .into_iter()
        .filter(|c| !app.buy_caliber_order.contains(c))
        .collect();
    missing.sort();
    app.buy_caliber_order.extend(missing);
}

/// 子弹顺序与校准配置对账：剔除已移除子弹、补入新增子弹（网格阅读顺序 y→x 行优先）。
fn reconcile_bullet(app: &mut ObserverApp, cal: &str) {
    let Some(bullets) = app.info.get(cal) else {
        return;
    };
    let keys: Vec<String> = bullets.keys().cloned().collect();
    let entry = app.buy_bullet_order.entry(cal.to_string()).or_default();
    entry.retain(|b| keys.contains(b));
    let mut missing: Vec<String> = keys
        .into_iter()
        .filter(|b| !entry.contains(b))
        .collect();
    missing.sort_by(|a, b| {
        let ra = &bullets[a].name;
        let rb = &bullets[b].name;
        ra[1].cmp(&rb[1]).then(ra[0].cmp(&rb[0]))
    });
    entry.extend(missing);
}

/// 选中口径兜底：未选中或已失效时取列表第一个。
fn ensure_selection(app: &mut ObserverApp) {
    let ok = app
        .order_selected_caliber
        .as_ref()
        .map(|c| app.buy_caliber_order.contains(c))
        .unwrap_or(false);
    if !ok {
        app.order_selected_caliber = app.buy_caliber_order.first().cloned();
    }
}

/// 复位全部拖拽临时状态。
fn reset_drag(app: &mut ObserverApp) {
    app.drag_src = None;
    app.drop_dst = None;
    app.drag_scope = None;
    app.drag_pointer = None;
    app.drag_snap_target = None;
    app.drag_snap_t0 = None;
}

/// 返回主面板（页面切换，复位拖拽态）。
fn back_to_main(app: &mut ObserverApp) {
    reset_drag(app);
    app.page = crate::Page::Main;
}
