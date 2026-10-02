//! 三角洲行动 · 交易行物价观察软件
//!
//! 模块分层（UI / 业务逻辑 / 平台底层分离到不同文件夹）：
//! - `domain/`：业务逻辑与领域层（标定配置、识别编排、OCR、数字识别神经网络、
//!   观察循环与落盘、记录解析）。
//! - `platform/`：平台 / 系统底层（Win32 窗口操作、截图裁剪、全局键盘钩子）。
//! - `ui/`：用户界面（三个 Tab 面板 + 自绘图表 + 公共视觉样式）。
//!
//! 主面板（Idle 态）含三个相互独立的 Tab，UI 与具体处理逻辑分离：
//! - **开始观察**：点击按钮后聚焦游戏窗口并收起为常驻置顶的毛玻璃悬浮面板
//!   （透明、鼠标穿透、自动排除在截图之外），后台线程循环检索各口径、逐单元格识别子弹
//!   名称（Windows OCR）与价格（千/百/十/个逐位数字神经网络识别），结构化结果直接写入
//!   `保存目录/records.jsonl`（异常另记 anomalies.log）。F9 随时结束观察、恢复主面板。
//! - **OCR 测试**：选择一张本地交易行截图并指定口径，复用与「开始观察」完全相同的
//!   识别逻辑（见 [`crate::domain::recognition`]）识别子弹名称与价格，汇总展示于预览区。
//! - **价格可视化**：按口径筛选 → 选择子弹，绘制最近 24 小时价格折线（数据源 records.jsonl）。
//!
//! 子弹识别（名称 + 价格）已提取为共享模块 [`crate::domain::recognition`]，各 Tab 共用，无重复实现。
//! 本文件只负责 GUI 编排、程序入口与 App 状态；识别与采样的全部细节见 `domain` / `platform` 模块。

mod domain;
pub use domain::{
    config, ocr, observe, digit, nn_classifier, recognition, records,
};
mod platform;
pub use platform::{win, screenshot, hotkey_hook};
mod ui;
pub use ui::{theme, control_panel, floating_panel, ocr_test_panel, price_viz_panel};
pub use ui::price_viz_panel as price_viz;

use eframe::egui::viewport::{ViewportCommand, WindowLevel};
use eframe::egui::{
    self, Align2, Color32, Context, Frame, Margin, RichText, Stroke, ViewportBuilder,
};
use eframe::egui::epaint::Shadow;
use eframe::App;

use global_hotkey::GlobalHotKeyEvent;
use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::HotKeyState;

use crate::config::{Config, GAME_KEYWORD};
use crate::observe::ObserveMsg;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// 本程序窗口标题（注意：不要包含「三角洲行动」，否则会误匹配到游戏窗口）。
const SELF_TITLE: &str = "交易行物价观察器";
/// 主控制面板（Idle 态）窗口尺寸（固定 1200×600，不可缩放）。
const IDLE_SIZE: (f32, f32) = (1200.0, 600.0);
/// 毛玻璃悬浮面板（激活态）窗口尺寸。
const FLOAT_SIZE: (f32, f32) = (320.0, 208.0);

/// 程序运行状态。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Idle,   // 主控制面板（完整界面）
    Active, // 毛玻璃悬浮面板（观察中）
}

/// 主面板（Idle 态）下的功能 Tab。各 Tab 的界面与具体处理逻辑相互独立：
/// UI 只负责采集输入与展示结果，识别等重逻辑统一走 [`crate::domain::recognition`] 模块。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainTab {
    Observe, // 开始观察（配置与启动）
    OcrTest, // OCR 测试（本地图片 + 口径 → 复用识别逻辑）
    PriceViz, // 价格可视化（口径 → 子弹 → 最近 24h 价格折线）
}

/// 存档配置文件路径（位于可执行文件同目录），用于持久化识别结果保存目录。
fn config_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("save_path.cfg"))
}

/// 读取持久化结果保存目录；若不存在则返回默认 D:\pic。
fn load_save_path() -> String {
    if let Some(p) = config_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "D:\\pic".to_string()
}

/// 将结果保存目录写入本地配置文件（持久化）。
///
/// **只在「开始观察」点击时调用一次**：输入框每敲一键都写盘会让半输入状态
/// （如 `D:\pi`）被持久化，下次启动表现为「保存目录自己变了」——同一功能
/// 不同次使用行为出现差异的根源，杜绝之。
pub(crate) fn persist_save_path(p: &str) {
    if let Some(cfg) = config_path() {
        let _ = std::fs::write(&cfg, p);
    }
}

/// 程序核心状态（Idle 态主面板 + 激活态共享）。
pub(crate) struct ObserverApp {
    /// 当前运行状态。
    pub(crate) mode: Mode,
    /// 状态栏文本（激活态展示）。
    pub(crate) status: String,
    /// 错误提示（开始观察前校验失败）。
    pub(crate) error: Option<String>,
    /// 毛玻璃轻量通知：(文字, 是否成功, 过期时间戳)。
    pub(crate) toast: Option<(String, bool, f64)>,
    /// 后台观察任务的结果通道（OCR 抓取价格），完成后经此回传主线程（避免 UI 卡顿）。
    pub(crate) observe_rx: Option<mpsc::Receiver<ObserveMsg>>,
    /// 观察任务的中止信号（F9 置位），`AtomicBool` 跨线程共享。
    pub(crate) observe_stop: Option<Arc<AtomicBool>>,
    /// 观察进度：第几轮 / 当前口径名 / 口径序号 / 口径总数 / 已识别子弹数。
    pub(crate) observe_progress: Option<(usize, String, usize, usize, usize)>,
    /// 轮间等待：第几轮已完成、距下一轮剩余秒数。
    pub(crate) observe_wait: Option<(usize, u64)>,
    /// 观察循环间隔（分钟）：每轮遍历全部口径后等待该时长再开始下一轮（默认 30）。
    pub(crate) interval_min: f64,
    /// 识别结果保存目录（已持久化，records.jsonl 直接写入该目录）。
    pub(crate) save_path: String,
    /// 是否把每次识别出的数字裁剪写入用户指定目录用于神经网络训练。
    /// 由「开始观察」面板复选框控制，**默认关闭**，仅勾选时落盘。
    pub(crate) save_digits: bool,
    /// 数字裁剪输出目录：勾选「保存数字」后，所有数位裁剪统一写入此目录（不做分类）。
    /// 默认 = 结果保存目录下的 `digits/`，可经 UI 自定义；仅在勾选保留功能时显示与生效。
    pub(crate) digits_save_path: String,
    /// CJK 字体是否已安装（仅首次 update 安装一次）。
    font_ready: bool,
    /// 口径配置（代码化，见 `config` 模块）：口径 -> 子弹名 -> 区域坐标。
    pub(crate) info: Arc<Config>,
    /// 全局快捷键管理器（必须存活于整个程序生命周期）。
    _manager: GlobalHotKeyManager,
    /// F9（结束观察）快捷键 id。
    f9_id: u32,
    /// 当前主面板激活的 Tab（Idle 态下切换「开始观察」/「OCR 测试」/「价格可视化」）。
    pub(crate) active_tab: MainTab,
    /// OCR 测试 Tab：待识别的本地图片路径。
    pub(crate) ocr_test_image: String,
    /// OCR 测试 Tab：选定的口径（与 `info` 的 key 对应）。
    pub(crate) ocr_test_caliber: String,
    /// OCR 测试 Tab：最近一次识别结果（含逐子弹汇总，供预览区展示）。
    pub(crate) ocr_test_result: Option<crate::ui::ocr_test_panel::OcrTestResult>,
    /// OCR 测试 Tab：标注图的 egui 纹理句柄（在识别时创建，预览区直接引用）。
    pub(crate) ocr_test_texture: Option<egui::TextureHandle>,
    /// OCR 测试 Tab：「价格浏览」滑块当前选中的子弹序号（0 基）。
    pub(crate) ocr_test_selected: usize,
    /// OCR 测试 Tab：结果序列号（每次识别 +1），用于判断是否需要重建数字纹理。
    pub(crate) ocr_test_seq: u64,
    /// OCR 测试 Tab：已构建数字纹理的结果序列号（缓存）。
    pub(crate) ocr_test_digit_seq: u64,
    /// OCR 测试 Tab：已构建数字纹理的选中序号（缓存）。
    pub(crate) ocr_test_digit_sel: usize,
    /// OCR 测试 Tab：当前选中子弹各个数位的裁剪图纹理（千/百/十/个，缺位为 None）。
    pub(crate) ocr_test_digit_tex: Vec<Option<egui::TextureHandle>>,
    /// 价格可视化 Tab 的全部界面状态（含 records.jsonl 缓存，见 `price_viz` 模块）。
    pub(crate) viz: price_viz::VizState,
}

impl ObserverApp {
    fn new() -> Self {
        let manager = GlobalHotKeyManager::new()
            .expect("无法初始化全局快捷键管理器（global-hotkey）");

        // 开始观察仅通过按钮点击触发，不绑定快捷键；仅 F9（结束观察）注册全局快捷键。
        let f9 = HotKey::new(None, Code::F9);
        let f9_id = f9.id;
        manager.register(f9).expect("注册 F9 快捷键失败");

        // 安装低级键盘钩子（WH_KEYBOARD_LL）：即便激活态把键盘焦点交给游戏，
        // 也能可靠捕获 F9（global-hotkey 在游戏独占焦点时收不到）。不影响游戏输入（透传）。
        hotkey_hook::install();

        let info = Arc::new(config::build_config());

        Self {
            mode: Mode::Idle,
            status: String::new(),
            error: None,
            toast: None,
            save_path: load_save_path(),
            save_digits: false,
            digits_save_path: format!("{}/digits", load_save_path()),
            font_ready: false,
            info,
            _manager: manager,
            f9_id,
            active_tab: MainTab::Observe,
            ocr_test_image: String::new(),
            ocr_test_caliber: "12 Gauge".to_string(),
            ocr_test_result: None,
            ocr_test_texture: None,
            ocr_test_selected: 0,
            ocr_test_seq: 0,
            ocr_test_digit_seq: 0,
            ocr_test_digit_sel: usize::MAX,
            ocr_test_digit_tex: Vec::new(),
            viz: {
                let mut v = price_viz::VizState::default();
                v.caliber = "12 Gauge".to_string();
                v
            },
            observe_rx: None,
            observe_stop: None,
            observe_progress: None,
            observe_wait: None,
            interval_min: 30.0,
        }
    }

    /// 进入激活态：聚焦游戏窗口 + 本面板收起为置顶毛玻璃悬浮面板（透明、穿透、排除截图），
    /// 并启动后台 OCR 抓取线程。
    pub(crate) fn start(&mut self, ctx: &Context) {
        if let Mode::Active = self.mode {
            return;
        }
        // 查找并聚焦游戏窗口（标题包含关键字匹配）；未找到则给出提示，不进入激活态。
        let Some(hwnd) = win::find_window_by_keyword(GAME_KEYWORD) else {
            self.error = Some(format!(
                "未检测到「{}」窗口，请先启动游戏后再试。",
                GAME_KEYWORD
            ));
            return;
        };
        win::focus_window(hwnd);

        // 保存目录在此刻落盘一次（而非输入过程中每键一次）。
        persist_save_path(&self.save_path);

        self.mode = Mode::Active;
        self.error = None;
        self.status = "观察中".to_string();

        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
        // 激活态保持鼠标穿透，不遮挡游戏操作（面板仅作状态展示）。
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));

        // 激活态沿用小尺寸悬浮面板。
        let float_size = FLOAT_SIZE;
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(float_size.0, float_size.1)));
        let screen = ctx.input(|i| i.screen_rect);
        let x = screen.max.x - float_size.0 - 24.0;
        let y = 24.0;
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(x, y)));

        if let Some(my_hwnd) = win::find_window_by_keyword(SELF_TITLE) {
            win::set_exclude_from_capture(my_hwnd, true);
            win::set_window_blur(my_hwnd, true);
        }

        // 启动后台观察线程。
        self.start_observation();
    }

    /// 结束激活态：立即恢复完整界面（不透明、有边框、可交互），焦点归还本面板。
    fn stop(&mut self, ctx: &Context) {
        if self.mode == Mode::Idle {
            return;
        }
        // 中止仍在运行的后台观察线程，避免悬挂任务继续占用游戏窗口。
        if let Some(stop) = &self.observe_stop {
            stop.store(true, Ordering::Relaxed);
        }
        self.mode = Mode::Idle;
        self.status.clear();
        self.error = None;

        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(false));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(IDLE_SIZE.0, IDLE_SIZE.1)));
        ctx.send_viewport_cmd(ViewportCommand::Focus);

        if let Some(my_hwnd) = win::find_window_by_keyword(SELF_TITLE) {
            win::set_exclude_from_capture(my_hwnd, false);
            win::set_window_blur(my_hwnd, false);
        }
    }

    /// 启动后台观察线程：遍历 config.rs 所有口径，逐一检索 + OCR 抓取价格并持久化。
    /// 结果通过 `observe_rx` 通道回传；`observe_stop` 供 F9 优雅中止。
    fn start_observation(&mut self) {
        if self.observe_rx.is_some() {
            return; // 已有观察任务在后台运行，避免重复/并发写同一目录。
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let (tx, rx) = mpsc::channel::<ObserveMsg>();
        let save_path = self.save_path.clone();
        let info = self.info.clone();
        let interval_min = self.interval_min;
        let save_digits = self.save_digits;
        let digits_dir = self.digits_save_path.clone();
        self.observe_rx = Some(rx);
        self.observe_stop = Some(stop);
        self.observe_progress = None;
        self.observe_wait = None;
        std::thread::spawn(move || {
            observe::run_observation(
                save_path,
                info,
                stop_for_thread,
                tx,
                interval_min,
                save_digits,
                digits_dir,
            );
        });
    }

    /// 每帧检查后台观察任务消息：进度更新悬浮面板，完成时写提示并复位激活态（主线程，非阻塞）。
    fn poll_observe(&mut self, ctx: &Context) {
        let msg = match self.observe_rx.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(m) => Some(m),
                Err(_) => None,
            },
            None => None,
        };
        if let Some(m) = msg {
            match m {
                ObserveMsg::Progress {
                    cycle,
                    caliber,
                    idx,
                    total,
                    done_bullets,
                } => {
                    self.observe_wait = None;
                    self.observe_progress = Some((cycle, caliber, idx, total, done_bullets));
                    ctx.request_repaint();
                }
                ObserveMsg::Waiting { cycle, remain_sec } => {
                    self.observe_wait = Some((cycle, remain_sec));
                    ctx.request_repaint();
                }
                ObserveMsg::Done { ok, toast } => {
                    let now = ctx.input(|i| i.time);
                    self.toast = Some((toast, ok, now + if ok { 1.8 } else { 2.2 }));
                    win::play_capture_chime();
                    self.observe_rx = None;
                    self.observe_stop = None;
                    self.observe_progress = None;
                    self.observe_wait = None;
                    // 若仍在观察激活态（非 F9 提前结束导致已复位），恢复主面板。
                    if let Mode::Active = self.mode {
                        self.stop(ctx);
                    }
                    ctx.request_repaint();
                }
            }
        }
    }

    /// 每帧轮询快捷键事件。命中后强制重绘，保证 F9 即时生效。
    ///
    /// 双通道：
    /// 1) `global-hotkey`（RegisterHotKey）—— 本窗口有焦点时可靠；
    /// 2) 低级键盘钩子 `hotkey_hook`（WH_KEYBOARD_LL）—— 即便激活态把键盘焦点交给游戏、
    ///    游戏以原始输入/全屏独占焦点，也能在按键分发前于本线程捕获 F9，作为可靠兜底。
    fn poll_hotkeys(&mut self, ctx: &Context) {
        let mut handled = false;
        // 1) global-hotkey：只响应按下，规避 0.5.5 一次按键双事件（Pressed+Released）。
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if event.id == self.f9_id {
                self.handle_f9(ctx);
                handled = true;
            }
        }
        // 2) 低级键盘钩子兜底：消费式取走标志（即便游戏持焦点也能到达）。
        if hotkey_hook::take_f9() {
            self.handle_f9(ctx);
            handled = true;
        }
        if handled {
            ctx.request_repaint();
        }
    }

    /// F9：结束观察，复位 UI 回主控制面板（观察线程经 observe_stop 信号优雅退出）。
    fn handle_f9(&mut self, ctx: &Context) {
        self.stop(ctx);
    }

    /// 渲染「观察完成 / 失败」的毛玻璃轻量通知；已过期则清除。
    ///
    /// 通知绘制在本程序窗口内，自动被截图排除，不会污染游戏截图。
    /// 淡入淡出：`elapsed` 前 0.3s 线性淡入，剩余 0.4s 内线性淡出。
    fn render_toast(&mut self, ctx: &Context, now: f64) {
        // `take()` 取出而非借用：过期时不放回即完成清除，未过期则放回继续显示。
        let Some((msg, ok, expiry)) = self.toast.take() else {
            return;
        };
        if expiry <= now {
            return;
        }
        let total = if ok { 1.8 } else { 2.2 };
        let remain = expiry - now;
        let elapsed = total - remain;
        let alpha = if elapsed < 0.3 {
            (elapsed / 0.3).clamp(0.0, 1.0)
        } else if remain < 0.4 {
            (remain / 0.4).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let a = (210.0 * alpha) as u8;
        let (icon, fill_rgb, icon_rgb, text_rgb) = if ok {
            ('✓', (224, 246, 230), (46, 204, 113), (20, 46, 30))
        } else {
            ('✗', (250, 235, 232), (231, 76, 60), (46, 20, 18))
        };
        egui::Area::new("capture_toast".into())
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 22.0))
            .show(ctx, |ui| {
                Frame::default()
                    .fill(Color32::from_rgba_unmultiplied(
                        fill_rgb.0, fill_rgb.1, fill_rgb.2, a,
                    ))
                    .rounding(14.0)
                    .stroke(Stroke::new(
                        1.2,
                        Color32::from_rgba_unmultiplied(255, 255, 255, a),
                    ))
                    .inner_margin(Margin::same(12.0))
                    .shadow(Shadow {
                        offset: egui::vec2(0.0, 6.0),
                        blur: 16.0,
                        spread: 0.0,
                        color: Color32::from_rgba_unmultiplied(0, 0, 0, a / 3),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                Color32::from_rgba_unmultiplied(
                                    icon_rgb.0, icon_rgb.1, icon_rgb.2, a,
                                ),
                                RichText::new(icon.to_string()).size(18.0),
                            );
                            ui.colored_label(
                                Color32::from_rgba_unmultiplied(
                                    text_rgb.0, text_rgb.1, text_rgb.2, a,
                                ),
                                RichText::new(&msg).size(14.0),
                            );
                        });
                    });
            });
        self.toast = Some((msg, ok, expiry));
    }
}

impl App for ObserverApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.font_ready {
            crate::ui::theme::install_cjk_font(ctx);
            // 统一白色主题：覆盖价格可视化 Tab 原有的深色背景，使各 Tab 风格一致。
            ctx.set_visuals(egui::Visuals::light());
            self.font_ready = true;
        }

        self.poll_hotkeys(ctx);
        self.poll_observe(ctx);

        match self.mode {
            Mode::Idle => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical(|ui| {
                        // Tab 切换栏：各 Tab 界面与处理逻辑相互独立（UI 只采集/展示，识别走 `recognition` 模块）。
                        ui.horizontal(|ui| {
                            crate::ui::theme::tab_button(ui, "开始观察", MainTab::Observe, &mut self.active_tab);
                            crate::ui::theme::tab_button(ui, "OCR 测试", MainTab::OcrTest, &mut self.active_tab);
                            crate::ui::theme::tab_button(ui, "价格可视化", MainTab::PriceViz, &mut self.active_tab);
                        });
                        ui.separator();
                        match self.active_tab {
                            MainTab::Observe => crate::ui::control_panel::ControlPanel.show(ui, self),
                            MainTab::OcrTest => crate::ui::ocr_test_panel::OcrTestPanel.show(ui, self),
                            MainTab::PriceViz => {
                                // 口径 / 子弹列表来自标定配置；记录缓存由面板按文件 mtime 失效重读。
                                let calibers: Vec<String> = {
                                    let mut v: Vec<String> = self.info.keys().cloned().collect();
                                    v.sort();
                                    v
                                };
                                let bullets: Vec<String> = self
                                    .info
                                    .get(&self.viz.caliber)
                                    .map(|m| {
                                        let mut v: Vec<String> = m.keys().cloned().collect();
                                        v.sort();
                                        v
                                    })
                                    .unwrap_or_default();
                                let save_path = self.save_path.clone();
                                price_viz::PriceVizPanel.show(
                                    ui,
                                    &mut self.viz,
                                    &calibers,
                                    &bullets,
                                    &save_path,
                                );
                            }
                        }
                    });
                });
            }
            Mode::Active => {
                egui::CentralPanel::default()
                    .frame(Frame::default().fill(Color32::TRANSPARENT))
                    .show(ctx, |ui| crate::ui::floating_panel::FloatingPanel.show(ui, self));
            }
        }

        // —— 观察完成/失败的毛玻璃轻量通知（渲染细节见 `render_toast`）——
        let now = ctx.input(|i| i.time);
        self.render_toast(ctx, now);

        // 保持事件循环持续重绘（约 30fps），确保 F9 即时响应。
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}

fn main() -> eframe::Result<()> {
    // 声明 Per-Monitor V2 DPI 感知：使 `GetWindowRect`/`GetClientRect`/`ClientToScreen`/
    // `PrintWindow` 全部以设备像素取舍，缩放 ≠ 100% 时不再产生尺寸错位与黑边。
    // 必须在创建任何窗口（eframe 的 run_native）之前调用，否则会被后续默认设置拒绝。
    unsafe {
        use windows::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let app = ObserverApp::new();

    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title(SELF_TITLE)
            .with_inner_size([IDLE_SIZE.0, IDLE_SIZE.1])
            .with_resizable(false)
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        SELF_TITLE,
        options,
        Box::new(|_cc| Box::new(app) as Box<dyn eframe::App>),
    )
}
