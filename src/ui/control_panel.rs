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
                // —— 口径处理顺序：可拖拽卡片（拖拽手柄调整先后，控制观察流程中子弹处理顺序）——
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("处理顺序：拖拽口径卡片调整先后（上=先处理）；卡片内子弹已网格排列。")
                            .small()
                            .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                    );
                    if ui.button("重置顺序").clicked() {
                        // 清空后运行期回退到「口径名排序」默认顺序。
                        app.buy_caliber_order.clear();
                        crate::persist_buy_order(&[]);
                    }
                });
                ui.add_space(4.0);

                // 与校准配置对账：剔除已移除口径、补入新增口径（按名排序），保证「展示=处理顺序」且完整。
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

                // 拖拽落定（每帧统一在循环外判定一次）：释放主指针时移动并持久化；异常（指针离窗）复位。
                let released = ui
                    .ctx()
                    .input(|i| i.pointer.button_released(egui::PointerButton::Primary));
                let down = ui.ctx().input(|i| i.pointer.primary_down());
                if released {
                    if let (Some(src), Some(dst)) = (app.drag_src, app.drop_dst) {
                        if src != dst && dst < app.buy_caliber_order.len() {
                            let item = app.buy_caliber_order.remove(src);
                            app.buy_caliber_order.insert(dst, item);
                            crate::persist_buy_order(&app.buy_caliber_order);
                        }
                    }
                    app.drag_src = None;
                    app.drop_dst = None;
                } else if !down && app.drag_src.is_some() {
                    app.drag_src = None;
                    app.drop_dst = None;
                }

                let n_cal = app.buy_caliber_order.len();
                for i in 0..n_cal {
                    let cal = app.buy_caliber_order[i].clone();
                    // 卡片状态：源卡半透明、落点卡蓝色高亮描边。
                    let is_src = app.drag_src == Some(i);
                    let is_dst = app.drop_dst == Some(i) && app.drag_src.is_some() && !is_src;
                    let stroke = if is_dst {
                        egui::Stroke::new(2.0, Color32::from_rgb(56, 135, 255))
                    } else {
                        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 206, 214, 255))
                    };
                    let frame = egui::Frame::default()
                        .fill(Color32::from_rgba_unmultiplied(
                            255,
                            255,
                            255,
                            if is_src { 130 } else { 255 },
                        ))
                        .rounding(8.0)
                        .stroke(stroke)
                        .inner_margin(egui::Margin::same(10.0));
                    let card_resp = frame
                        .show(ui, |ui| {
                            // 拖拽手柄 + 口径标题：手柄承载 Sense::drag 以发起拖拽，不阻塞下方阈值输入。
                            ui.horizontal(|ui| {
                                let handle =
                                    ui.allocate_response(egui::vec2(18.0, 18.0), egui::Sense::drag());
                                if handle.drag_started() {
                                    app.drag_src = Some(i);
                                }
                                // 手柄视觉：三点竖排 grip。
                                let r = handle.rect;
                                let painter = ui.painter();
                                for k in 0..3 {
                                    let cy = r.min.y + 4.0 + k as f32 * 5.0;
                                    painter.circle_filled(
                                        egui::pos2(r.center().x, cy),
                                        1.6,
                                        Color32::from_rgb(150, 156, 166),
                                    );
                                }
                                ui.label(RichText::new(format!("{}. {}", i + 1, cal)).strong());
                            });
                            ui.add_space(2.0);
                            // 子弹网格：横向自动换行（替代原纯纵向长列），提升多子弹单屏可见性。
                            if let Some(bullets) = app.info.get(&cal) {
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
                                            let entry = app
                                                .buy_thresholds
                                                .entry((cal.clone(), b.clone()))
                                                .or_default();
                                            let resp = ui.add(
                                                egui::TextEdit::singleline(entry)
                                                    .desired_width(118.0)
                                                    .hint_text("最低接受价格"),
                                            );
                                            // 失焦即完成一次编辑：作为阈值持久化的干净提交点。
                                            if resp.lost_focus() {
                                                buy_dirty = true;
                                            }
                                        });
                                        ui.add_space(8.0);
                                    }
                                });
                            }
                        })
                        .response;

                    // 落点判定：拖拽进行中且指针悬停在本卡片矩形内（且非源卡）时记为落点。
                    if app.drag_src.is_some() && app.drag_src != Some(i) {
                        if let Some(p) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                            if card_resp.rect.contains(p) {
                                app.drop_dst = Some(i);
                            }
                        }
                    }
                    ui.add_space(8.0);
                }
                ui.label(
                    RichText::new("对每个子弹填写「最低接受价格」（哈夫币）：观察时若某子弹价格低于该值，将进入购买界面、自动把所选数量调整到目标值并点击「购买」，完成后 ESC 返回扫描；价格回升或超阈值则只 ESC 返回、不购买。留空表示不设阈值。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
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
