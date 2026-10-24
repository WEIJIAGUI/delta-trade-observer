//! 导游模式提示面板（激活态）—— 独立毛玻璃组件，与状态面板互不嵌套。

use eframe::egui::{Color32, Frame, Margin, RichText, Stroke, Ui};

use crate::ObserverApp;

/// 导游模式提示面板（激活态）—— 独立毛玻璃组件。
///
/// 与 [`crate::ui::floating_panel::FloatingPanel`]（状态面板）完全分离：
/// - 仅当导游模式有待确认计划（`Tour::pending_text` 为 `Some`）时才渲染，关闭态 / 无计划时为无操作、不占用任何空间；
/// - 自身按内容自适应撑开尺寸（容器不写死固定 px），长计划文案按可读性换行宽度折行，整体随内容动态伸缩。
pub struct TourGuidePanel;

impl TourGuidePanel {
    pub(crate) fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        // 仅待确认时展示；关闭态 / 无计划时直接返回（不绘制、不占位）。
        let Some(text) = app.tour.pending_text() else {
            return;
        };

        // 固定面板宽度（与状态面板共用 FLOAT_W）：必须在 Frame 绘制前约束外层 ui，
        // 否则 Frame 背景框会撑满上层 Area 可用宽，重新触发「窗口撑开 → 面板填充」的宽度自增与闪烁。
        ui.set_min_width(crate::FLOAT_W);
        ui.set_max_width(crate::FLOAT_W);
        // 独立毛玻璃容器：Frame 仅负责圆角 + 蓝调描边以区别于状态面板（状态面板为白描边）；
        // 磨砂底图由全屏模糊桌面纹理按本面板坐标裁切绘制（见 `floating_panel.rs` 同款逻辑），
        // 不再依赖会黑屏的 `DwmEnableBlurBehindWindow`。
        let glass = Frame::default()
            .fill(Color32::TRANSPARENT)
            .rounding(14.0)
            .stroke(Stroke::new(2.0, Color32::from_rgb(56, 135, 255)))
            .inner_margin(Margin::same(16.0));
        glass.show(ui, |ui| {
            // 毛玻璃底图：裁切全屏模糊桌面纹理到本面板区域，并压一层淡蓝调磨砂叠加。
            let rect = ui.max_rect();
            if let Some(tex) = app.blur_tex() {
                let uv = app.blur_uv(rect);
                ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
                ui.painter()
                    .rect_filled(rect, 14.0, Color32::from_rgba_unmultiplied(240, 248, 255, 70));
            }
            // 仅约束文字换行宽度（可读性），与固定面板宽度配合，长计划文案按 300 宽度折行。
            ui.set_max_width(crate::FLOAT_W - 32.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("导游模式 · 待确认")
                        .size(13.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(&text)
                        .size(16.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new("按 Enter 确认并执行 · F9 结束观察")
                        .size(12.0)
                        .color(Color32::from_rgba_unmultiplied(255, 255, 255, 220)),
                );
            });
        });
    }
}
