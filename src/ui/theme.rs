//! 公共视觉样式与控件：卡片容器、Tab 按钮、CJK 字体安装。

use eframe::egui::epaint::Shadow;
use eframe::egui::{self, Color32, Context, FontData, FontDefinitions, FontFamily, Frame, Margin, RichText, Stroke, Ui};

use crate::MainTab;

/// 绘制一张带描边/圆角/阴影的卡片容器，用于清晰区分功能区块。
pub fn card(ui: &mut Ui, accent: Color32, content: impl FnOnce(&mut Ui)) {
    Frame::default()
        .fill(Color32::from_rgba_unmultiplied(247, 249, 252, 255))
        .rounding(12.0)
        .stroke(Stroke::new(1.5, accent))
        .inner_margin(Margin::same(16.0))
        .shadow(Shadow {
            offset: egui::vec2(0.0, 3.0),
            blur: 10.0,
            spread: 0.0,
            color: Color32::from_rgba_unmultiplied(0, 0, 0, 28),
        })
        .show(ui, content);
}

/// 顶部 Tab 切换按钮：选中态高亮，点击即切换 `active`。
pub(crate) fn tab_button(ui: &mut Ui, label: &str, tab: MainTab, active: &mut MainTab) {
    let selected = *active == tab;
    let accent = Color32::from_rgb(236, 140, 64);
    // 白色主题导航：透明底色按钮，选中=深字 + 橙下划线，未选中=灰字。
    let resp = ui.add(
        egui::Button::new(
            RichText::new(label).color(if selected {
                Color32::from_rgb(26, 29, 36)
            } else {
                Color32::from_rgb(107, 114, 128)
            }),
        )
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .rounding(0.0),
    );
    if selected {
        // 选中态底部强调条，视觉反馈当前 Tab。
        let rect = resp.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - 2.0),
                egui::pos2(rect.right(), rect.bottom()),
            ),
            0.0,
            accent,
        );
    }
    if resp.clicked() {
        *active = tab;
    }
}

/// 加载系统 CJK 字体，避免中文显示为方块（tofu）。
///
/// 依赖的只是 `ctx`，不访问 `ObserverApp` 实例状态，故实现为自由函数（原为 `&self` 方法）。
pub fn install_cjk_font(ctx: &Context) {
    let candidates = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/simsun.ttc",
        "C:/Windows/Fonts/simhei.ttf",
    ];
    let bytes = candidates.iter().find_map(|p| std::fs::read(p).ok());
    let Some(bytes) = bytes else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".to_string(), FontData::from_owned(bytes));
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("cjk".to_string());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("cjk".to_string());
    ctx.set_fonts(fonts);
}
