//! OCR 测试 Tab 面板（Idle 态）—— 独立组件。
//!
//! 职责边界（UI 与业务逻辑分离）：本面板只负责**采集输入**（图片路径、口径）与
//! **展示结果**（标注预览图 + 汇总表）；真正的识别逻辑全部委托给
//! [`crate::recognition::recognize_caliber`]，与「开始观察」复用同一套坐标与识别代码。

use eframe::egui::{self, Align2, Color32, Context, FontId, RichText, Ui};

use crate::digit::DigitDetail;
use crate::ObserverApp;

/// OCR 测试 Tab 的单子弹识别结果（用于预览区汇总展示）。
pub struct OcrSlot {
    /// 配置中该槽位对应的子弹名（来自 `config` 书写顺序）。
    pub bullet: String,
    /// 名称区 OCR 文本（可能含噪声，与 `bullet` 不完全一致）。
    pub ocr_name: String,
    /// 识别出的价格字符串（非法/空时为 `""`）。
    pub price: String,
    /// 价格状态：0=合法价格，1=全空，2=非法断层。
    pub price_state: u8,
    /// 非法时的原因说明（状态 2 时有效）。
    pub price_desc: String,
    /// 价格各数位的识别可视化详情（千/百/十/个，缺位为 None）。
    pub digits: Vec<Option<DigitDetail>>,
}

/// OCR 测试 Tab 的一次完整识别结果。
pub struct OcrTestResult {
    pub slots: Vec<OcrSlot>,
    pub width: i32,
    pub height: i32,
}

/// OCR 测试面板（Idle 态 Tab）—— 独立组件。
pub struct OcrTestPanel;

impl OcrTestPanel {
    pub(crate) fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        let accent = Color32::from_rgb(120, 90, 220);
        crate::ui::theme::card(ui, accent, |ui| {
            ui.heading(RichText::new("OCR 测试").color(accent));
            ui.add_space(4.0);
            ui.label("选择一张交易行截图与口径，点击「识别」即复用「开始观察」的同套坐标与识别逻辑，汇总子弹名称与价格。");
            ui.add_space(10.0);

            // 图片路径：通过系统文件选择对话框选取（仍支持拖拽 / 粘贴）。
            ui.horizontal(|ui| {
                ui.label("图片：");
                ui.add_enabled(
                    false,
                    egui::TextEdit::singleline(&mut app.ocr_test_image).desired_width(220.0),
                );
                if ui.button("选择图片...").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .set_title("选择交易行截图")
                        .add_filter("图片", &["png", "jpg", "jpeg", "bmp"])
                        .pick_file()
                    {
                        app.ocr_test_image = p.to_string_lossy().to_string();
                        app.ocr_test_texture = None;
                        app.ocr_test_result = None;
                        app.ocr_test_digit_tex.clear();
                        app.ocr_test_selected = 0;
                    }
                }
            });
            // 拖拽：把图片文件拖入窗口即填入路径（egui 内置，无需额外依赖）。
            if let Some(file) = ui.ctx().input(|i| i.raw.dropped_files.first().cloned()) {
                if let Some(p) = file.path {
                    let p = p.to_string_lossy().to_string();
                    if p != app.ocr_test_image {
                        app.ocr_test_image = p;
                        app.ocr_test_texture = None;
                        app.ocr_test_result = None;
                        app.ocr_test_digit_tex.clear();
                        app.ocr_test_selected = 0;
                    }
                }
            }
            ui.label(
                RichText::new("提示：可直接将图片拖入窗口；或粘贴 / 输入本地路径（支持 PNG / JPG）。")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
            ui.add_space(8.0);

            // 口径选择器
            ui.horizontal(|ui| {
                ui.label("口径：");
                egui::ComboBox::from_id_source("ocr_test_caliber")
                    .selected_text(&app.ocr_test_caliber)
                    .show_ui(ui, |ui| {
                        let calibers: Vec<String> = app.info.keys().cloned().collect();
                        for c in calibers {
                            ui.selectable_value(&mut app.ocr_test_caliber, c.clone(), &c);
                        }
                    });
            });
            ui.add_space(8.0);

            // 识别按钮（在 UI 线程同步执行单图识别，结果即时回显）
            if ui
                .add(
                    egui::Button::new(RichText::new("▶  识别").color(Color32::WHITE))
                        .min_size(egui::vec2(220.0, 36.0))
                        .fill(accent)
                        .rounding(8.0),
                )
                .clicked()
            {
                Self::run(app, ui.ctx());
            }
            if let Some(err) = &app.error {
                ui.add_space(6.0);
                ui.colored_label(Color32::from_rgb(220, 80, 80), err);
            }
        });

        // 结果区整体可滚动：确保任意窗口高度下「价格浏览」按钮都可达（不被底部截断、无滚动条）。
        egui::ScrollArea::vertical().show(ui, |ui| {
            // 预览区：标注图 + 汇总表 + 价格浏览滑块（逐位数字识别）
            // 注意：`res` 是不可变借用 `app`，故任何对 `app` 的写入（提交选中 / 重建纹理）
            // 必须在其作用域之外完成；下面的渲染块只做只读访问。
            let n = app.ocr_test_result.as_ref().map_or(0, |r| r.slots.len());
            if n > 0 {
                let sel = app.ocr_test_selected.min(n - 1);
                app.ocr_test_selected = sel; // 提交选中序号（在 res 借用前完成）
            }
            // 仅在结果更新或选中变化时才重建数字纹理，避免每帧重复上传 GPU。
            if n > 0 {
                let sel = app.ocr_test_selected;
                if app.ocr_test_seq != app.ocr_test_digit_seq || app.ocr_test_digit_sel != sel {
                    if let Some(res) = app.ocr_test_result.as_ref() {
                        let slot = &res.slots[sel];
                        app.ocr_test_digit_tex.clear();
                        for d in &slot.digits {
                            let tex = match d {
                                Some(dd) if !dd.crop.is_empty() => {
                                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                                        [dd.crop_w as usize, dd.crop_h as usize],
                                        &dd.crop,
                                    );
                                    Some(ui.ctx().load_texture(
                                        format!("ocr_digit_{}_{}", sel, app.ocr_test_digit_tex.len()),
                                        ci,
                                        egui::TextureOptions::LINEAR,
                                    ))
                                }
                                _ => None,
                            };
                            app.ocr_test_digit_tex.push(tex);
                        }
                        app.ocr_test_digit_seq = app.ocr_test_seq;
                        app.ocr_test_digit_sel = sel;
                    }
                }
            }

            if let Some(res) = &app.ocr_test_result {
                ui.add_space(12.0);
                if let Some(tex) = &app.ocr_test_texture {
                    let w = res.width as f32;
                    let h = res.height as f32;
                    let max_w = ui.available_width().max(200.0);
                    ui.image((tex.id(), egui::vec2(max_w, max_w * h / w)));
                }
                ui.add_space(10.0);
                ui.label(RichText::new("识别汇总").strong());
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for slot in &res.slots {
                            ui.horizontal(|ui| {
                                ui.label(format!("• {}：", slot.bullet));
                                ui.colored_label(Color32::from_rgb(80, 140, 255), &slot.ocr_name);
                                let (ptext, pcolor) = match slot.price_state {
                                    0 => (slot.price.clone(), Color32::from_rgb(40, 160, 80)),
                                    1 => ("（空）".to_string(), Color32::GRAY),
                                    _ => (
                                        format!("⚠ {}", slot.price_desc),
                                        Color32::from_rgb(220, 120, 40),
                                    ),
                                };
                                ui.colored_label(pcolor, format!("价格 {}", ptext));
                            });
                        }
                    });

                // —— 价格浏览：用「上一个 / 下一个」按钮切换不同子弹价格的逐位数字识别效果 ——
                // 直接写回 app.ocr_test_selected，修正此前滑块绑定局部变量、未回写导致每次回弹 0 的 bug。
                let n = res.slots.len();
                if n > 0 {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(RichText::new("价格浏览（逐位数字识别）").strong());
                    let mut cur = app.ocr_test_selected.min(n - 1);
                    ui.horizontal(|ui| {
                        ui.label("价格序号：");
                        if ui
                            .add_enabled(
                                cur > 0,
                                egui::Button::new("◀ 上一个").min_size(egui::vec2(110.0, 30.0)),
                            )
                            .clicked()
                            && cur > 0
                        {
                            cur -= 1;
                        }
                        if ui
                            .add_enabled(
                                cur < n - 1,
                                egui::Button::new("下一个 ▶").min_size(egui::vec2(110.0, 30.0)),
                            )
                            .clicked()
                            && cur < n - 1
                        {
                            cur += 1;
                        }
                        ui.label(format!("（第 {} / {} 个）", cur + 1, n));
                    });
                    app.ocr_test_selected = cur; // 写回，确保数字纹理重建与下次渲染一致
                    let sel = cur;
                    let slot = &res.slots[sel];
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("子弹：{}", slot.bullet)).strong());
                        ui.label(format!("名称：{}", slot.ocr_name));
                        let (ptext, pcolor) = match slot.price_state {
                            0 => (slot.price.clone(), Color32::from_rgb(40, 160, 80)),
                            1 => ("（空）".to_string(), Color32::GRAY),
                            _ => (
                                format!("⚠ {}", slot.price_desc),
                                Color32::from_rgb(220, 120, 40),
                            ),
                        };
                        ui.colored_label(pcolor, format!("价格 {}", ptext));
                    });

                    ui.add_space(6.0);
                    ui.label("逐位数字识别（裁剪图 + 识别结果 + 各类概率分数条）：");
                    for (di, d) in slot.digits.iter().enumerate() {
                        let pos = ["千", "百", "十", "个"][di];
                        ui.horizontal(|ui| {
                            ui.label(format!("{pos}位："));
                            match d {
                                Some(dd) => {
                                    if let Some(tex) =
                                        app.ocr_test_digit_tex.get(di).and_then(|t| t.as_ref())
                                    {
                                        let cw = dd.crop_w as f32;
                                        let ch = dd.crop_h as f32;
                                        let disp_h = (ch * 3.0).clamp(28.0, 60.0);
                                        ui.image((tex.id(), egui::vec2(disp_h * cw / ch, disp_h)));
                                    }
                                    let rec = dd
                                        .recognized
                                        .map(|c| c.to_string())
                                        .unwrap_or_else(|| "—".into());
                                    ui.colored_label(
                                        Color32::from_rgb(40, 160, 80),
                                        format!("识别 {rec}"),
                                    );
                                    ui.label(format!("最高分 {:.3}", dd.best_score));
                                    draw_score_bars(ui, &dd.scores, dd.best_digit);
                                }
                                None => {
                                    ui.label("（无裁剪）");
                                }
                            }
                        });
                    }
                }
            }
        }); // ScrollArea 包裹结果区
    }

    /// 业务逻辑入口：加载图片 → 调共享识别器 → 生成标注图与汇总，结果写回 `app`。
    /// 与「开始观察」共用 [`crate::recognition::recognize_caliber`]，不重复实现识别。
    fn run(app: &mut ObserverApp, ctx: &Context) {
        app.error = None;
        if app.ocr_test_image.trim().is_empty() {
            app.error = Some("请先选择一张图片".into());
            return;
        }
        if app.ocr_test_caliber.trim().is_empty() {
            app.error = Some("请选择口径".into());
            return;
        }

        let frame = match crate::screenshot::CapturedImage::from_file(&app.ocr_test_image) {
            Ok(f) => f,
            Err(e) => {
                app.error = Some(e);
                return;
            }
        };

        // 复用「开始观察」的识别逻辑（同一模块、同一坐标），并附带每个数位的识别详情。
        let mut results = crate::recognition::recognize_caliber_detail(
            &frame,
            &app.ocr_test_caliber,
            &app.info,
        );
        // 配置内层 HashMap 遍历顺序不定，按子弹名排序使展示稳定。
        results.sort_by(|a, b| a.0.cmp(&b.0));

        let (w, h) = frame.dimensions();
        let scale = crate::config::Scale::for_client_width(w);
        let mut anno = frame.rgba.clone();
        let mut slots = Vec::with_capacity(results.len());
        for (name, rects, rec, digits) in &results {
            // 标注名称框 + 价格四个数位框（与「开始观察」调试帧风格一致）。
            crate::screenshot::draw_rect_outline(
                &mut anno,
                w,
                h,
                scale.rect(rects.name),
                crate::screenshot::NAME_BOX_COLOR,
                1,
            );
            for (i, d) in rects.digit_rects().iter().enumerate() {
                let d = scale.rect(*d);
                crate::screenshot::draw_rect_outline(
                    &mut anno,
                    w,
                    h,
                    d,
                    crate::screenshot::DIGIT_BOX_COLORS[i],
                    crate::screenshot::DIGIT_BOX_THICKNESS,
                );
                crate::screenshot::draw_digit(
                    &mut anno,
                    w,
                    h,
                    d[0],
                    (d[1] - crate::screenshot::DIGIT_LABEL_LIFT_PX).max(0),
                    i as u8,
                    crate::screenshot::DIGIT_BOX_COLORS[i],
                );
            }
            let (price, state, desc) = match &rec.price {
                crate::digit::PriceRec::Price(s) => (s.clone(), 0u8, String::new()),
                crate::digit::PriceRec::Empty => (String::new(), 1u8, String::new()),
                crate::digit::PriceRec::Invalid { reason, partial } => {
                    (String::new(), 2u8, format!("{reason} [{partial}]"))
                }
            };
            slots.push(OcrSlot {
                bullet: name.clone(),
                ocr_name: rec.name.clone(),
                price,
                price_state: state,
                price_desc: desc,
                digits: digits.clone(),
            });
        }

        // 把标注图载入为 egui 纹理，供预览区直接引用。
        let color_image =
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &anno);
        let tex = ctx.load_texture(
            "ocr_test_annotated",
            color_image,
            egui::TextureOptions::LINEAR,
        );
        app.ocr_test_texture = Some(tex);
        app.ocr_test_result = Some(OcrTestResult {
            slots,
            width: w,
            height: h,
        });
        // 触发「价格浏览」按钮重置到首个子弹，并标记数字纹理需重建。
        app.ocr_test_seq += 1;
        app.ocr_test_selected = 0;
    }
}

/// 绘制 0~9 各类的 softmax 概率分数条，便于直观对比每个数字位的识别效果。
///
/// `scores` 为与 0..9 一一对应的概率（区间 `[0.0, 1.0]`）；`best` 标记最高概率对应的数字，
/// 用绿色高亮。分数映射到 `[0.0, 1.0]`（负分贴底、正分向上）后按比例绘制竖条。
fn draw_score_bars(ui: &mut Ui, scores: &[f32; 10], best: Option<u8>) {
    let bar_w = 14.0_f32;
    let gap = 4.0_f32;
    let h = 40.0_f32;
    let total_w = 10.0 * bar_w + 9.0 * gap;
    ui.allocate_ui(egui::vec2(total_w, h + 14.0), |ui| {
        let painter = ui.painter();
        let rect = ui.max_rect();
        let x0 = rect.min.x;
        let base_y = rect.min.y + h;
        for d in 0..10usize {
            let s = scores[d];
            let norm = ((s + 1.0) / 2.0).clamp(0.0, 1.0); // [-1,1] -> [0,1]
            let bh = (norm * h).max(1.0);
            let x = x0 + d as f32 * (bar_w + gap);
            let col = if Some(d as u8) == best {
                Color32::from_rgb(60, 200, 90)
            } else {
                Color32::from_gray(150)
            };
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, base_y - bh), egui::vec2(bar_w, bh)),
                2.0,
                col,
            );
            painter.text(
                egui::pos2(x + bar_w / 2.0, base_y + 2.0),
                Align2::CENTER_TOP,
                format!("{d}"),
                FontId::monospace(11.0),
                Color32::GRAY,
            );
        }
    });
}
