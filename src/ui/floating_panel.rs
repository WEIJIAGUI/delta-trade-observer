//! 毛玻璃悬浮面板（激活态）—— 独立组件。

use eframe::egui::epaint::Shadow;
use eframe::egui::{self, Color32, Frame, Margin, RichText, Stroke, Ui};

use crate::ObserverApp;

/// 毛玻璃悬浮面板（激活态）—— 独立组件，不与其他面板共用。
pub struct FloatingPanel;

impl FloatingPanel {
    pub(crate) fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        let glass = Frame::default()
            .fill(Color32::from_rgba_unmultiplied(228, 234, 246, 70))
            .rounding(16.0)
            .stroke(Stroke::new(1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 170)))
            .inner_margin(Margin::same(16.0))
            .shadow(Shadow {
                offset: egui::vec2(0.0, 6.0),
                blur: 18.0,
                spread: 0.0,
                color: Color32::from_rgba_unmultiplied(0, 0, 0, 70),
            });
        glass.show(ui, |ui| Self::status_body(ui, app));
    }

    /// 观察态：毛玻璃浮层内展示状态、进度与结束提示。
    fn status_body(ui: &mut Ui, app: &mut ObserverApp) {
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
    }
}
