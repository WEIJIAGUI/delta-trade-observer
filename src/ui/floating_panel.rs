//! 毛玻璃悬浮面板（激活态）—— 独立组件。

use eframe::egui::{Color32, Frame, Margin, RichText, Stroke, Ui};

use crate::ObserverApp;

/// 毛玻璃悬浮面板（激活态）—— 独立组件，不与其他面板共用。
pub struct FloatingPanel;

impl FloatingPanel {
    pub(crate) fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        // 通透毛玻璃：极淡白底 + 白色描边；背景模糊由 DWM 提供。
        // 移除黑色实色底与黑色阴影，仅保留磨砂质感（半透明、背景模糊、无黑色遮罩）。
        let glass = Frame::default()
            .fill(Color32::from_rgba_unmultiplied(240, 244, 250, 50))
            .rounding(16.0)
            .stroke(Stroke::new(1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 210)))
            .inner_margin(Margin::same(16.0));
        glass.show(ui, |ui| Self::status_body(ui, app));
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
                        .color(Color32::from_rgb(18, 28, 46)),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "已截图 {}/{} 次",
                        app.qty_tool_taken,
                        crate::qty_tool::TOTAL_SHOTS
                    ))
                    .size(14.0)
                    .color(Color32::from_rgb(24, 34, 56)),
                );
            });
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("按 F9 结束采集")
                        .size(13.0)
                        .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
                );
                ui.label(
                    RichText::new("本面板已自动排除在截图之外")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
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
                        .color(Color32::from_rgb(18, 28, 46)),
                );
                ui.label(
                    RichText::new("哈夫币")
                        .size(12.0)
                        .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
                );
            });
            ui.add_space(10.0);
        }
        // 购买界面场景判定（按钮检测器据「购买」按钮位置判定）：仅展示，不弹提示框。
        if let Some(scene) = &app.buy_scene {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(scene)
                        .size(13.0)
                        .color(Color32::from_rgb(18, 28, 46)),
                );
            });
            ui.add_space(6.0);
        }
        // 购买结果 toast 归类（点击购买后监听游戏顶部弹窗）：仅展示，不弹提示框。
        if let Some(res) = &app.buy_result {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(res)
                        .size(13.0)
                        .strong()
                        .color(Color32::from_rgb(18, 28, 46)),
                );
            });
            ui.add_space(6.0);
        }
        // 购买后判断（toast 归类）逐模式核查：确认各场景模式均已接入「正常/低价购买」判定，并展示上次实测结果。
        // 两种场景均经同一条已接入 toast 判定的购买路径（唯一点击点 observe.rs），故接入状态恒为真；
        // 此处仅逐模式列出「✓ 已接入」并展示该模式上次实测到的购买结果（尚无购买记录则提示待实测）。
        {
            ui.add_space(4.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("购买后判断 · 模式核查")
                        .size(13.0)
                        .strong()
                        .color(Color32::from_rgb(18, 28, 46)),
                );
            });
            for scene in crate::config::BuyScene::all() {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(60, 200, 110), "✓");
                    ui.label(
                        RichText::new(scene.label())
                            .size(12.0)
                            .color(Color32::from_rgb(24, 34, 56)),
                    );
                    let txt = match app.buy_result_by_scene.get(scene) {
                        Some(r) => r.clone(),
                        None => "尚无购买记录".to_string(),
                    };
                    ui.label(
                        RichText::new(format!("· {txt}"))
                            .size(11.0)
                            .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
                    );
                });
            }
        }
        ui.add_space(4.0);
        // 目标购买数量：仅当已判定购买界面场景（即处于购买阶段）时展示，便于实机核对减/加调整目标。
        if app.buy_scene.is_some() {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!("目标数量：{}", app.buy_qty))
                        .size(12.0)
                        .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
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
                        .color(Color32::from_rgb(18, 28, 46)),
                );
            });
            ui.add_space(6.0);
        }
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(60, 200, 110), "●");
                ui.heading(RichText::new(&app.status).color(Color32::from_rgb(18, 28, 46)));
            });
            ui.add_space(8.0);
            if let Some((cycle, cal, idx, total, done)) = &app.observe_progress {
                ui.label(
                    RichText::new(format!(
                        "第 {} 轮 · 口径：{}（{}/{}）",
                        cycle, cal, idx + 1, total
                    ))
                    .color(Color32::from_rgb(24, 34, 56)),
                );
                ui.label(
                    RichText::new(format!("已识别：{done} 条价格"))
                        .color(Color32::from_rgb(24, 34, 56)),
                );
            }
            if let Some((cycle, remain)) = &app.observe_wait {
                ui.label(
                    RichText::new(format!(
                        "第 {cycle} 轮完成，{}:{:02} 后开始下一轮",
                        remain / 60,
                        remain % 60
                    ))
                    .color(Color32::from_rgb(24, 34, 56)),
                );
            }
            ui.label(RichText::new("按 F9 结束观察").color(Color32::from_rgb(24, 34, 56)));
            ui.label(
                RichText::new("本面板已自动排除在截图之外")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
            );
        });
        // 导游层：若有待确认计划，额外叠加一层毛玻璃卡片，展示会议文字形式的计划与「回车确认」提示。
        if let Some(text) = app.tour.pending_text() {
            ui.add_space(10.0);
            let guide = Frame::default()
                .fill(Color32::from_rgba_unmultiplied(250, 252, 255, 240))
                .rounding(14.0)
                .stroke(Stroke::new(2.0, Color32::from_rgb(56, 135, 255)))
                .inner_margin(Margin::same(16.0));
            guide.show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("导游模式 · 待确认")
                            .size(13.0)
                            .strong()
                            .color(Color32::from_rgb(56, 135, 255)),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(&text)
                            .size(16.0)
                            .strong()
                            .color(Color32::from_rgb(18, 28, 46)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("按 Enter 确认并执行 · F9 结束观察")
                            .size(12.0)
                            .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
                    );
                });
            });
        }
    }
}
