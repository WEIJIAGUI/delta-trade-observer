//! 「数量采集」工具 Tab 面板（Idle 态）—— 独立组件。
//!
//! 功能：点击「开始采集」后聚焦并前置游戏窗口，使用者手动将游戏切换到购买界面；
//! 随后程序自动循环采集「所选子弹数量」的**整块数字区域**截图（按真实数量值标注）并点击「加号」按钮，
//! 直至累计截图达 200 次自动停止。采集产物为带标注的训练集，用于「数量整体识别」模型专项训练。

use eframe::egui::{self, Color32, RichText, Ui};

use crate::qty_tool::TOTAL_SHOTS;
use crate::ObserverApp;

/// 数量采集工具面板。
pub struct QtyToolPanel;

impl QtyToolPanel {
    pub(crate) fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.heading("数量采集工具");
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "聚焦游戏窗口后，请手动将游戏切换到购买界面；程序随后自动采集整块数量区域截图（按真实数值标注）并点击加号，累计 200 次后停止。",
                    )
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 220)),
                );
                ui.add_space(18.0);
            });

            let accent = Color32::from_rgb(120, 95, 220);
            crate::ui::theme::card(ui, accent, |ui| {
                ui.heading(RichText::new("整块数量区域采集").color(accent));
                ui.add_space(4.0);
                ui.label("采集购买界面「所选子弹数量」的整块数字区域（一张 PNG），并点击加号递增，重复至 200 次截图。样本按真实数量值存入 qty_dataset/<label>/，可直接用于训练整体识别模型。");
                ui.add_space(10.0);

                // 起始数量：采集开始时游戏内「所选子弹数量」的当前值（须 ∈ 1..200，不含 0）。
                // 第 idx 轮截图标注的数量值 = 起始数量 + idx，作为该样本的真实标注（每个数量值只截一次）。
                ui.horizontal(|ui| {
                    ui.label("起始数量：");
                    ui.add(
                        egui::DragValue::new(&mut app.qty_tool_start)
                            .clamp_range(1..=200)
                            .speed(1),
                    );
                });
                ui.label(
                    RichText::new("请先在游戏内把数量调到该值（1~200），再开始采集；程序据此为每个样本标注真实整数值，且每个数量值只截一次。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
                ui.add_space(6.0);

                // 场景来源：自动检测 / 显式可兑换 / 显式不可兑换。
                egui::ComboBox::from_label("场景")
                    .selected_text(match app.qty_tool_scene {
                        1 => "可兑换子弹",
                        2 => "不可兑换子弹",
                        _ => "自动检测",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut app.qty_tool_scene, 0, "自动检测");
                        ui.selectable_value(&mut app.qty_tool_scene, 1, "可兑换子弹");
                        ui.selectable_value(&mut app.qty_tool_scene, 2, "不可兑换子弹");
                    });
                ui.label(
                    RichText::new("自动检测会据购买按钮位置判定场景；若检测不准，请手动选择对应场景。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
                ui.add_space(6.0);

                // 输出目录：采集到的整块区域裁剪直接写入 `qty_dataset/<label>/`。
                ui.horizontal(|ui| {
                    ui.label("输出目录：");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut app.qty_tool_dir)
                            .desired_width(220.0)
                            .hint_text("采集输出根目录（下建 qty_dataset/<label>/）"),
                    );
                    if ui.button("浏览...").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_title("选择数量采集输出目录")
                            .pick_folder()
                        {
                            app.qty_tool_dir = p.to_string_lossy().to_string();
                        }
                    }
                });
                ui.label(
                    RichText::new("整块数量区域图形如 qty_00000_shot.png，按数值（1~200）归入 qty_dataset/<label>/。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
                ui.add_space(10.0);

                // 进度 / 开始 / 停止
                if app.qty_tool_running {
                    ui.label(
                        RichText::new(format!("采集中：{}/{} 次截图", app.qty_tool_taken, TOTAL_SHOTS))
                            .strong()
                            .color(accent),
                    );
                    ui.add_space(6.0);
                    let stop = egui::Button::new(RichText::new("■  停止").color(Color32::WHITE))
                        .min_size(egui::vec2(248.0, 44.0))
                        .fill(Color32::from_rgb(200, 80, 80))
                        .rounding(8.0);
                    if ui.add(stop).clicked() {
                        app.stop_qty_tool();
                    }
                } else {
                    let start = egui::Button::new(RichText::new("▶  开始采集").color(Color32::WHITE))
                        .min_size(egui::vec2(248.0, 44.0))
                        .fill(accent)
                        .rounding(8.0);
                    if ui.add(start).clicked() {
                        app.start_qty_tool(ui.ctx());
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("点击后将聚焦游戏窗口；请在游戏内切到购买界面，剩余交由程序自动完成（F9 不中断本工具，用上方「停止」结束）。")
                            .small()
                            .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                    );
                }
            });
        });

        if let Some(err) = &app.error {
            ui.add_space(14.0);
            ui.centered_and_justified(|ui| {
                ui.colored_label(Color32::from_rgb(220, 80, 80), err);
            });
        }
    }
}
