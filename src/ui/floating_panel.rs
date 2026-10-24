//! 毛玻璃悬浮面板（激活态）—— 独立组件。

use eframe::egui::{Color32, Frame, Margin, RichText, Stroke, Ui};

use crate::ObserverApp;

/// 毛玻璃悬浮面板（激活态）—— 独立组件，不与其他面板共用。
pub struct FloatingPanel;

impl FloatingPanel {
    pub(crate) fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        // 固定面板宽度：在 Frame 绘制前约束所在 ui 的最大/最小宽度为 `FLOAT_W`。
        // 若只约束 Frame 内部子 ui，只能限制文字换行、无法约束 Frame 背景框宽度——Frame 仍会
        // 撑满上层 Area 的可用宽，进而引发「窗口随内容撑开 → 内容随窗口变宽」的正反馈自增（闪烁 + 宽度自增）。
        // 此处约束外层 ui 后，Frame 背景框宽度被锁定为 FLOAT_W，从根本上消除宽度自增与闪烁。
        ui.set_min_width(crate::FLOAT_W);
        ui.set_max_width(crate::FLOAT_W);
        // 毛玻璃容器：Frame 仅负责圆角 + 白色描边；磨砂底图由下方把全屏模糊桌面纹理
        // （后台 DXGI Desktop Duplication 采集）按本面板屏幕坐标裁切绘制，并压一层极淡白调得到通透质感。
        // 不再依赖会黑屏的 `DwmEnableBlurBehindWindow`。
        let glass = Frame::default()
            .fill(Color32::TRANSPARENT)
            .rounding(16.0)
            .stroke(Stroke::new(1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 210)))
            .inner_margin(Margin::same(16.0));
        glass.show(ui, |ui| {
            // 毛玻璃底图：把全屏模糊桌面纹理裁切到本面板区域（inner_margin == rounding，
            // 内容区角点恰好落在圆角弧心，方形底图不会探出圆角外）。
            let rect = ui.max_rect();
            if let Some(tex) = app.blur_tex() {
                let uv = app.blur_uv(rect);
                ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
                // 极淡白调磨砂叠加：在模糊底图上压一层半透明白，得到通透的磨砂质感（无黑色遮罩）。
                ui.painter()
                    .rect_filled(rect, 16.0, Color32::from_rgba_unmultiplied(245, 247, 250, 90));
            }
            Self::status_body(ui, app);
        });
    }

    /// 观察态：毛玻璃浮层内展示价格、状态、进度与结束提示。
    fn status_body(ui: &mut Ui, app: &mut ObserverApp) {
        // 数量采集工具运行中：优先展示采集进度（毛玻璃悬浮面板），F9 结束。
        if app.qty_tool_running {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("数量采集中")
                        .size(18.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "已截图 {}/{} 次",
                        app.qty_tool_taken,
                        crate::qty_tool::TOTAL_SHOTS
                    ))
                    .size(14.0)
                    .color(Color32::WHITE),
                );
            });
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("按 F9 结束采集")
                        .size(13.0)
                        .color(Color32::from_rgba_unmultiplied(255, 255, 255, 220)),
                );
                ui.label(
                    RichText::new("本面板已自动排除在截图之外")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(255, 255, 255, 220)),
                );
            });
            return;
        }
        // 购买阶段识别到的价格：大号数字直接渲染在毛玻璃上（仅显示数字，不弹提示框）。
        if let Some(price) = app.buy_price {
            ui.vertical_centered(|ui| {
                ui.add_space(2.0);
                ui.label(
                    RichText::new(format!("{}", price))
                        .size(42.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.label(
                    RichText::new("哈夫币")
                        .size(12.0)
                        .color(Color32::from_rgba_unmultiplied(255, 255, 255, 220)),
                );
            });
            ui.add_space(10.0);
        }
        // 购买结果 toast 归类（点击购买后监听游戏顶部弹窗）：仅展示，不弹提示框。
        if let Some(res) = &app.buy_result {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(res)
                        .size(13.0)
                        .strong()
                        .color(Color32::WHITE),
                );
            });
            ui.add_space(6.0);
        }
        ui.add_space(4.0);
        // 目标购买数量：仅当已判定购买界面场景（即处于购买阶段）时展示，便于实机核对减/加调整目标。
        if app.buy_scene.is_some() {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!("目标数量：{}", app.buy_qty))
                        .size(12.0)
                        .color(Color32::from_rgba_unmultiplied(255, 255, 255, 220)),
                );
            });
            ui.add_space(4.0);
            // 实时识别到的子弹数量：数量调整过程中持续刷新（减/加点击后逐步逼近目标值）。
            // 仅购买阶段展示；识别失败/尚未识别出显示占位「—」，避免面板空缺造成误解。
            ui.vertical_centered(|ui| {
                let txt = match app.detected_qty {
                    Some(q) => format!("当前数量：{q}"),
                    None => "当前数量：—".to_string(),
                };
                ui.label(
                    RichText::new(txt)
                        .size(15.0)
                        .strong()
                        .color(Color32::WHITE),
                );
            });
            ui.add_space(6.0);
        }
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(60, 200, 110), "●");
                ui.heading(RichText::new(&app.status).color(Color32::WHITE));
            });
            ui.add_space(8.0);
            if let Some((cycle, cal, idx, total, done)) = &app.observe_progress {
                ui.label(
                    RichText::new(format!(
                        "第 {} 轮 · 口径：{}（{}/{}）",
                        cycle, cal, idx + 1, total
                    ))
                    .color(Color32::WHITE),
                );
                ui.label(
                    RichText::new(format!("已识别：{done} 条价格"))
                        .color(Color32::WHITE),
                );
            }
            if let Some((cycle, remain)) = &app.observe_wait {
                ui.label(
                    RichText::new(format!(
                        "第 {cycle} 轮完成，{}:{:02} 后开始下一轮",
                        remain / 60,
                        remain % 60
                    ))
                    .color(Color32::WHITE),
                );
            }
            ui.label(RichText::new("按 F9 结束观察").color(Color32::WHITE));
            ui.label(
                RichText::new("本面板已自动排除在截图之外")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(255, 255, 255, 220)),
            );
        });
    }
}
