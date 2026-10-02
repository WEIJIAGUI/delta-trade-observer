//! 「开始观察」Tab 面板（Idle 态）—— 独立组件。

use eframe::egui::{self, Color32, RichText, Ui};

use crate::nn_classifier;
use crate::ui::theme::card;
use crate::ObserverApp;

/// 主控制面板（Idle 态）下的观察配置与启动面板。
pub struct ControlPanel;

impl ControlPanel {
    pub(crate) fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.heading("三角洲行动 · 交易行物价观察");
                ui.add_space(4.0);
                ui.label(
                    RichText::new("聚焦游戏窗口并进入常驻毛玻璃悬浮状态，循环检索各口径并识别价格（F9 随时中止）。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 220)),
                );
                ui.add_space(18.0);
            });

            Self::observe_panel(ui, app);
        });
    }

    /// 开始观察面板：循环间隔、保存目录、启动按钮。
    fn observe_panel(ui: &mut Ui, app: &mut ObserverApp) {
        let obs_accent = Color32::from_rgb(56, 135, 255);
        card(ui, obs_accent, |ui| {
            ui.heading(RichText::new("开始观察").color(obs_accent));
            ui.add_space(4.0);
            ui.label("聚焦游戏窗口并进入常驻毛玻璃悬浮状态，循环检索各口径并识别价格。");
            ui.add_space(10.0);

            // 循环间隔输入框（默认 30 分钟）：每轮遍历全部口径后等待该时长再开始下一轮。
            ui.horizontal(|ui| {
                ui.label("循环间隔（分钟）：");
                ui.add(
                    egui::DragValue::new(&mut app.interval_min)
                        .speed(0.5)
                        .clamp_range(0.0..=1440.0)
                        .suffix(" 分钟"),
                );
            });
            ui.label(
                RichText::new("每轮遍历全部口径并识别完成后，等待该时长自动开始下一轮；F9 随时中止。")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
            ui.add_space(6.0);

            // 保存目录：识别记录直接写入该目录（不再使用 prices/ 子目录）。
            // 通过系统文件夹选择对话框选取，避免手输路径；选取后立即持久化。
            ui.horizontal(|ui| {
                ui.label("保存目录：");
                ui.add_enabled(
                    false,
                    egui::TextEdit::singleline(&mut app.save_path)
                        .desired_width(220.0)
                        .hint_text("识别结果保存目录"),
                );
                if ui.button("浏览...").clicked() {
                    if let Some(p) =
                        rfd::FileDialog::new().set_title("选择识别结果保存目录").pick_folder()
                    {
                        app.save_path = p.to_string_lossy().to_string();
                        crate::persist_save_path(&app.save_path);
                    }
                }
            });
            ui.label(
                RichText::new("识别结果（records.jsonl）与异常日志（anomalies.log）将直接写入此目录。")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
            ui.add_space(8.0);

            // 「保存数字」复选框：默认关闭；勾选后动态显示输出目录输入框。
            ui.checkbox(
                &mut app.save_digits,
                "保存识别到的数字（勾选后写入下方自定义目录）",
            );
            // 仅在勾选时显示输出目录输入框：动态显示，避免未启用时干扰布局。
            if app.save_digits {
                ui.horizontal(|ui| {
                    ui.label("数字输出目录：");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut app.digits_save_path)
                            .desired_width(220.0)
                            .hint_text("数字裁剪输出目录"),
                    );
                    if ui.button("浏览...").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_title("选择数字裁剪输出目录")
                            .pick_folder()
                        {
                            app.digits_save_path = p.to_string_lossy().to_string();
                        }
                    }
                });
                ui.label(
                    RichText::new("所有数位裁剪统一写入该目录（不做分类），文件名含序号与识别标签，供神经网络训练人工校正。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
            }

            // 识别引擎状态：神经网络依赖 model.onnx。
            ui.label(
                RichText::new(format!(
                    "识别引擎：{}",
                    if nn_classifier::is_available() {
                        "神经网络（model.onnx 已加载）"
                    } else {
                        "未加载 model.onnx（数字识别不可用）"
                    }
                ))
                .small()
                .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                let btn = egui::Button::new(RichText::new("▶  开始观察").color(Color32::WHITE))
                    .min_size(egui::vec2(248.0, 44.0))
                    .fill(obs_accent)
                    .rounding(8.0);
                if ui.add(btn).clicked() {
                    app.start(ui.ctx());
                }
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new("「开始观察」仅鼠标点击触发（无快捷键）；聚焦游戏窗口并进入常驻毛玻璃悬浮状态，循环检索各口径并识别价格（F9 随时中止）。")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
            ui.add_space(10.0);
        });

        if let Some(err) = &app.error {
            ui.add_space(14.0);
            ui.centered_and_justified(|ui| {
                ui.colored_label(Color32::from_rgb(220, 80, 80), err);
            });
        }
    }
}
