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
            // —— 顺序调整：面板最外层入口（切换到全屏顺序调整页：左口径 / 右子弹）——
            ui.horizontal(|ui| {
                let b = egui::Button::new(
                    RichText::new("⚙  顺序调整（口径 / 子弹）")
                        .strong()
                        .color(Color32::from_rgb(56, 135, 255)),
                )
                .min_size(egui::vec2(240.0, 34.0))
                .fill(Color32::from_rgba_unmultiplied(235, 242, 255, 255))
                .rounding(8.0)
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(56, 135, 255)));
                if ui.add(b).clicked() {
                    // 页面切换（取代模态弹窗）：重置拖拽态避免泄漏，默认选中第一个口径。
                    app.drag_src = None;
                    app.drop_dst = None;
                    app.drag_scope = None;
                    app.drag_pointer = None;
                    app.drag_snap_target = None;
                    app.drag_snap_t0 = None;
                    app.order_selected_caliber = app.buy_caliber_order.first().cloned();
                    app.page = crate::Page::OrderAdjust;
                }
                ui.label(
                    RichText::new("拖拽调整口径与子弹的处理先后（上/左=先处理）。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
            });
            ui.add_space(10.0);

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

            // 「保存扫描/购买阶段数字」两个独立开关：默认关闭；任一勾选后显示共享输出目录。
            // 扫描阶段数位裁剪写入 {目录}/scan/，购买阶段写入 {目录}/buy/，二者互不干扰。
            ui.checkbox(&mut app.save_scan_digits, "保存扫描阶段识别的数字");
            ui.checkbox(&mut app.save_buy_digits, "保存购买阶段识别的数字");
            // 任一开关勾选时显示共享的输出目录输入框（动态显示，避免未启用时干扰布局）。
            if app.save_scan_digits || app.save_buy_digits {
                ui.horizontal(|ui| {
                    ui.label("数字输出目录：");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut app.digits_save_path)
                            .desired_width(220.0)
                            .hint_text("数字裁剪输出根目录"),
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
                    RichText::new("扫描阶段数位裁剪写入 scan/ 子目录，购买阶段写入 buy/ 子目录（文件名含序号与识别标签），供神经网络训练人工校正。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
            }
            ui.add_space(6.0);

            // 「保存购买界面」独立开关 + 独立目录：勾选后，购买阶段界面稳定时
            // 把整幅购买界面截图（PNG）写入独立目录，与数字裁剪保存（scan/buy）完全互不干扰。
            ui.checkbox(&mut app.save_buy_ui, "保存购买界面（完整截图）");
            if app.save_buy_ui {
                ui.horizontal(|ui| {
                    ui.label("购买界面目录：");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut app.buy_ui_save_path)
                            .desired_width(220.0)
                            .hint_text("购买界面截图输出目录"),
                    );
                    if ui.button("浏览...").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_title("选择购买界面截图目录")
                            .pick_folder()
                        {
                            app.buy_ui_save_path = p.to_string_lossy().to_string();
                        }
                    }
                });
                ui.label(
                    RichText::new("购买阶段界面稳定后，将整幅客户区截图（PNG）写入该目录，便于复核购买界面布局与标定校准。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
            }

            // 「是否购买」复选框：默认关闭；勾选后展开各口径子弹的「最低接受价格」输入框。
            // 勾选/取消即持久化一次（见 persist_buy_cfg）。
            let buy_toggled = ui
                .checkbox(
                    &mut app.buy_enabled,
                    "是否购买（价格低于阈值时自动操作）",
                )
                .clicked();
            let mut buy_dirty = false;
            if app.buy_enabled {
                // 「购买数量」：购买阶段重验证价格仍低于阈值时，识别当前所选子弹数量并点击减/加按钮调到该值。
                // 默认 31，范围 1..=200（与数量整块分类器支持区间 1..200 一致）；每次启动回默认，避免误用上次残留值。
                ui.horizontal(|ui| {
                    ui.label("购买数量：");
                    ui.add(
                        egui::DragValue::new(&mut app.buy_qty)
                            .speed(1.0)
                            .clamp_range(1..=200)
                            .suffix(" 发"),
                    );
                });
                ui.label(
                    RichText::new("购买阶段将把所选子弹数量调整到该值，确认数量对齐后点击「购买」按钮、再 ESC 返回扫描（需「保存购买界面」复核数量框坐标与减/加点击落点）。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
                ui.add_space(4.0);
                // —— 各口径子弹「最低接受价格」输入框（每颗子弹各自输入，默认 10000；改动后持久化为新的默认）——
                ui.label(
                    RichText::new("各子弹「最低接受价格」（哈夫币）：留空或保持 10000 表示不设阈值；价格低于该值且开启购买时自动操作。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
                ui.add_space(4.0);
                // 与校准配置对账：剔除已移除口径、补入新增口径（按名排序），保证展示完整。
                {
                    let info_keys: Vec<String> = app.info.keys().cloned().collect();
                    app.buy_caliber_order.retain(|c| info_keys.contains(c));
                    let mut missing: Vec<String> = info_keys
                        .into_iter()
                        .filter(|c| !app.buy_caliber_order.contains(c))
                        .collect();
                    missing.sort();
                    app.buy_caliber_order.extend(missing);
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for cal in &app.buy_caliber_order {
                        ui.label(RichText::new(cal).strong());
                        if let Some(bullets) = app.info.get(cal) {
                            let mut bs: Vec<String> = bullets.keys().cloned().collect();
                            bs.sort();
                            ui.horizontal_wrapped(|ui| {
                                for b in &bs {
                                    ui.vertical(|ui| {
                                        ui.set_min_width(150.0);
                                        ui.label(
                                            RichText::new(b)
                                                .small()
                                                .strong()
                                                .color(Color32::from_rgba_unmultiplied(
                                                    60, 68, 84, 255,
                                                )),
                                        );
                                        let key = (cal.clone(), b.clone());
                                        // 默认显示 10000；若已持久化阈值则显示该阈值。
                                        let default = app
                                            .buy_thresholds
                                            .get(&key)
                                            .cloned()
                                            .filter(|s| !s.is_empty())
                                            .unwrap_or_else(|| "10000".to_string());
                                        let buf =
                                            app.buy_price_buffer.entry(key.clone()).or_insert(default);
                                        let resp = ui.add(
                                            egui::TextEdit::singleline(buf)
                                                .desired_width(118.0)
                                                .hint_text("最低接受价格"),
                                        );
                                        // 失焦才提交：改动后写入阈值并持久化（成为新的默认）；
                                        // 留空或维持 10000 视为不设阈值（不触发自动购买）。
                                        if resp.lost_focus() {
                                            let t = buf.trim().to_string();
                                            if t.is_empty() || t == "10000" {
                                                app.buy_thresholds.remove(&key);
                                            } else {
                                                app.buy_thresholds.insert(key.clone(), t);
                                            }
                                            buy_dirty = true;
                                        }
                                    });
                                    ui.add_space(8.0);
                                }
                            });
                        }
                        ui.add_space(6.0);
                    }
                });
            }
            if buy_toggled {
                crate::persist_buy_cfg(app.buy_enabled, &app.buy_thresholds);
            }
            if buy_dirty {
                crate::persist_buy_cfg(app.buy_enabled, &app.buy_thresholds);
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
            ui.add_space(8.0);

            // 导游模式开关：开启后，观察过程中每一步操作先以会议文字形式展示，回车确认后才执行。
            ui.checkbox(&mut app.tour_mode, "导游模式（每步展示计划，回车确认后执行）");
            ui.label(
                RichText::new("开启后，检索口径 / 进入购买 / 调整数量 / 确认购买等每一步会先以会议文字形式展示在毛玻璃导游层，按回车确认才执行；关闭则全自动运行（F9 随时中止）。")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
            ui.add_space(4.0);

            // 神经网络可视化触发方式改为「按住 F2」：松开即隐藏并暂停更新，不再用复选框常驻。
            ui.label(
                RichText::new("神经网络可视化：进入观察后「按住 F2」即可在游戏上方用矩形框 + 中文标签临时标注购买 / 加 / 减按钮；松开 F2 立即隐藏并停止检测更新。")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
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
