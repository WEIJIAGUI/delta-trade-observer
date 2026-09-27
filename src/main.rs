//! 三角洲行动 · 交易行物价观察 / 截图辅助软件
//!
//! 第一阶段（GUI 基础框架，已落地）：
//! 1. 主控制面板拆分为「开始观察」「截图」两个标签页。
//!    - 开始观察：仅鼠标点击触发（无快捷键），激活游戏窗口并进入毛玻璃悬浮态。
//!    - 截图：默认截取当前窗口全屏图像；亦可选中口径（src/config.rs 中的口径 key），
//!            按各子弹的 name/price 区域坐标批量截取并按「口径/子弹/」层级归档。
//! 2. 截图标签页按 F10 截取：默认全屏；选中口径时按代码配置（src/config.rs）中该口径下每个子弹的
//!    `name`（名称区域）与 `price`（价格区域）坐标裁剪，存至 保存目录/<口径>/<子弹>/name.png、price.png。
//! 3. 保存目录可通过系统文件夹选择器设置，并用本地配置文件 `save_path.cfg` 持久化。
//! 4. 悬浮面板显示当前状态，并自动排除在截图之外（SetWindowDisplayAffinity + DWM 毛玻璃）。
//! 5. 按 F9 立即结束，恢复主控制面板。
//!
//! 本阶段不涉及价格抓取与图表功能。

mod config;
mod win;
mod ocr;
mod observe;
mod digit;
mod viz;
mod hotkey_hook;

use eframe::egui::viewport::{ViewportCommand, WindowLevel};
use eframe::egui::{
    self, Align2, Color32, Context, Frame, Image, Margin, RichText, Stroke, TextureHandle,
    Ui, ViewportBuilder,
};
use eframe::egui::epaint::Shadow;
use eframe::App;

use global_hotkey::GlobalHotKeyEvent;
use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::HotKeyState;

use crate::config::{Config, GAME_KEYWORD};
use crate::observe::ObserveMsg;
use crate::viz::{VizData, VizView};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// 本程序窗口标题（注意：不要包含「三角洲行动」，否则会误匹配到游戏窗口）。
const SELF_TITLE: &str = "交易行物价观察器";
/// 主控制面板（Idle 态）窗口尺寸。
const IDLE_SIZE: (f32, f32) = (600.0, 700.0);
/// 毛玻璃悬浮面板（激活态）窗口尺寸。
const FLOAT_SIZE: (f32, f32) = (320.0, 208.0);
/// 行情看板标签页所需更大窗口尺寸（图表 + 概览同屏）。
const VIZ_SIZE: (f32, f32) = (960.0, 680.0);

/// 根据当前标签页设置主面板窗口尺寸（行情看板需要更大画布）。
fn apply_idle_size(ctx: &Context, tab: usize) {
    let s = if tab == 2 { VIZ_SIZE } else { IDLE_SIZE };
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(s.0, s.1)));
}

/// 激活态的子类型：决定毛玻璃面板显示的文字与结束文案。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureKind {
    Observing,  // 观察态 → 显示「结束观察」
    Screenshot, // 截图态 → 显示「截图」
}

/// 程序运行状态。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,                // 主控制面板（完整界面，含标签页）
    Active(CaptureKind), // 毛玻璃悬浮面板（观察 / 截图中）
}

/// 存档配置文件路径（位于可执行文件同目录），用于持久化截图保存目录。
fn config_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("save_path.cfg"))
}

/// 读取持久化的截图保存目录；若不存在则返回默认 D:\pic。
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

/// 将截图保存目录写入本地配置文件（持久化）。
fn persist_save_path(p: &str) {
    if let Some(cfg) = config_path() {
        let _ = std::fs::write(&cfg, p);
    }
}

/// 口径配置已代码化，见 `src/config.rs`（`config::build_config()`）。
/// 不再从外部 JSON 文件读取：12 Gauge 的 9 种子弹坐标作为公共基准，
/// 其余口径按书写顺序复用对应序号的 name/price，不再各自重复定义。

/// 把字符串清洗为合法文件名片段：替换 Windows 非法字符为 `_`。
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// 后台截图任务回传给主线程的结果消息。
enum CaptureMsg {
    Done { flash: String, toast: String, ok: bool },
}

struct ObserverApp {
    mode: Mode,
    status: String,
    error: Option<String>,
    /// 截图相关的瞬时提示（成功/失败信息），回主面板时显示。
    flash: Option<String>,
    /// 截图成功/失败的毛玻璃轻量通知：(文字, 是否成功, 过期时间戳)。
    toast: Option<(String, bool, f64)>,
    /// 后台截图任务的结果通道：捕获与批量裁剪在独立线程并行执行，完成后经此回传主线程（避免 UI 卡顿）。
    capture_rx: Option<mpsc::Receiver<CaptureMsg>>,
    /// 后台观察任务的结果通道（OCR 抓取价格），完成后经此回传主线程（避免 UI 卡顿）。
    observe_rx: Option<mpsc::Receiver<ObserveMsg>>,
    /// 观察任务的中止信号（F9 置位），`AtomicBool` 跨线程共享。
    observe_stop: Option<Arc<AtomicBool>>,
    /// 观察进度：第几轮 / 当前口径名 / 口径序号 / 口径总数 / 已识别子弹数。
    observe_progress: Option<(usize, String, usize, usize, usize)>,
    /// 轮间等待：第几轮已完成、距下一轮剩余秒数。
    observe_wait: Option<(usize, u64)>,
    /// 观察循环间隔（分钟）：每轮遍历全部口径后等待该时长再开始下一轮（默认 30）。
    interval_min: f64,
    /// 截图保存目录（已持久化）。
    save_path: String,
    font_ready: bool,

    /// 当前标签页：0 = 开始观察，1 = 截图，2 = 行情看板。
    tab: usize,
    /// 行情看板 Tab 的本地交互状态（筛选 / 选中子弹 / 时间维度）。
    viz_view: VizView,
    /// 行情数据缓存：(文件 mtime, 解析结果)，按 mtime 懒加载避免每帧重解析。
    viz_cache: Option<(u64, VizData)>,
    /// 行情看板「刷新」按钮请求：下一帧强制重载缓存。
    viz_reload_requested: bool,

    /// 口径配置（代码化，见 `config` 模块）：口径 -> 子弹名 -> 区域坐标。
    info: Arc<Config>,
    /// 口径列表，用于下拉框。
    caliber_keys: Vec<String>,
    /// 下拉选中项：0 = 全屏截图（默认）；>=1 = calibers[idx-1]。
    selected: usize,
    /// 整窗标注图开关：口径截图时额外保存整窗截图并框出 name/price 区域。
    annotate_full: bool,

    /// 工具页：用户选取的参考图片路径（用于标定截图区域）。
    tool_image_path: Option<String>,
    /// 工具页：截图区域坐标 `[x, y, w, h]`（相对图片左上角），用于定位截图范围。
    tool_rect: [i32; 4],
    /// 工具页：已加载的图片纹理（选择图片后解码生成并缓存，避免每帧重解码）。
    tool_texture: Option<TextureHandle>,

    /// 全局快捷键管理器（必须存活于整个程序生命周期）。
    _manager: GlobalHotKeyManager,
    /// F9（结束）快捷键 id。
    f9_id: u32,
    /// F10（开始截图 / 截图触发）快捷键 id。
    f10_id: u32,
}

/// 单张待保存图像的规格（零拷贝：大块像素缓冲由共享 `Arc<[u8]>` 持有，此处仅存元数据与裁剪/标注参数）。
enum JobSpec {
    /// 从共享缓冲按 [x,y,w,h] 裁剪（name / price 区域）。
    Crop { rect: [i32; 4] },
    /// 在共享缓冲副本上框出 name（绿）/ price（红）区域（整窗标注图）。
    Annotate { name: Option<[i32; 4]>, price: Option<[i32; 4]> },
}

struct Job {
    dir: String,
    fname: String,
    spec: JobSpec,
}

/// 在独立线程中执行单张图像的裁剪（按需）+ 编码保存。
/// `buf` 为共享的客户区像素缓冲（Arc，引用计数复制零成本）；annotate 路径仅在编码该图时克隆一次，
/// 彻底消除原实现中「先为每个子弹整屏 `buf.clone()` N 次并持有全部副本」的串行拷贝与内存放大。
fn encode_job(buf: &Arc<[u8]>, bw: i32, bh: i32, job: Job) -> bool {
    match job.spec {
        JobSpec::Crop { rect } => match win::crop_region(buf, bw, bh, rect) {
            Some((img, iw, ih)) => {
                win::save_rgba_png(&job.dir, &job.fname, &img, iw as u32, ih as u32).is_ok()
            }
            None => false,
        },
        JobSpec::Annotate { name, price } => {
            let mut v = buf.to_vec();
            if let Some(r) = name {
                win::draw_rect_outline(&mut v, bw, bh, r, [80, 255, 120, 255], 3);
            }
            if let Some(r) = price {
                win::draw_rect_outline(&mut v, bw, bh, r, [255, 90, 90, 255], 3);
            }
            win::save_rgba_png(&job.dir, &job.fname, &v, bw as u32, bh as u32).is_ok()
        }
    }
}

/// 后台线程执行：捕获窗口 → 收集所有待保存图像任务 → 分批并行编码保存。
/// 返回回传主线程的结果消息（不接触 egui，纯数据处理，可在任意线程调用）。
fn run_capture(
    save_path: String,
    selected: usize,
    annotate: bool,
    caliber: String,
    info: Arc<Config>,
) -> CaptureMsg {
    let hwnd = match win::find_window_by_keyword(GAME_KEYWORD) {
        Some(h) => h,
        None => {
            return CaptureMsg::Done {
                flash: "未检测到游戏窗口，无法截图".into(),
                toast: "✗ 未检测到游戏窗口".into(),
                ok: false,
            };
        }
    };

    // —— 默认：全屏截图 ——
    if selected == 0 {
        return match win::capture_window_to_png(hwnd, &save_path) {
            Ok(fname) => CaptureMsg::Done {
                flash: format!("已保存全屏截图：{fname}"),
                toast: format!("✓ 已保存全屏截图：{fname}"),
                ok: true,
            },
            Err(e) => CaptureMsg::Done {
                flash: format!("截图失败：{e}"),
                toast: format!("✗ 截图失败：{e}"),
                ok: false,
            },
        };
    }

    // —— 口径区域归档 ——
    let caliber_val = match info.get(&caliber) {
        Some(v) => v,
        None => {
            return CaptureMsg::Done {
                flash: format!("口径数据无效：{caliber}"),
                toast: format!("✗ 口径数据无效：{caliber}"),
                ok: false,
            };
        }
    };

    let (buf, bw, bh) = match win::capture_client_rgba(hwnd) {
        Ok(v) => v,
        Err(e) => {
            return CaptureMsg::Done {
                flash: format!("截图失败：{e}"),
                toast: format!("✗ 截图失败：{e}"),
                ok: false,
            };
        }
    };

    // 坐标契约（唯一真相）：配置中的 [x,y,w,h] 与默认截图均以
    // 「客户区 / 无标题栏截图」左上角为原点；capture_client_rgba 返回的缓冲即客户区缓冲，
    // 原点一致，直接使用坐标（历史版本曾多减一次偏移导致整体错位，已修正）。
    // 每个子弹的区域坐标直接取自代码配置（按书写顺序复用 12 Gauge 公共基准）。
    let base = sanitize(&caliber);
    // 收集任务元数据（仅元数据，不在此处拷贝像素）：大块缓冲交由共享 Arc 持有，
    // 标注路径在每个工作线程内「写时复制」仅克隆所需单张，彻底消除原实现 N 次整屏预拷贝与峰值内存放大。
    let buf: Arc<[u8]> = Arc::from(buf.into_boxed_slice());
    let mut jobs: Vec<Job> = Vec::new();

    for (bullet, rects) in caliber_val {
        let bdir = format!("{}/{}/{}", save_path, base, sanitize(bullet));
        let _ = std::fs::create_dir_all(&bdir);
        jobs.push(Job {
            dir: bdir.clone(),
            fname: "name.png".into(),
            spec: JobSpec::Crop { rect: rects.name },
        });
        jobs.push(Job {
            dir: bdir.clone(),
            fname: "price.png".into(),
            spec: JobSpec::Crop { rect: rects.price },
        });
        if annotate {
            jobs.push(Job {
                dir: bdir,
                fname: "annotated.png".into(),
                spec: JobSpec::Annotate {
                    name: Some(rects.name),
                    price: Some(rects.price),
                },
            });
        }
    }

    // 并行编码 + 保存：分批启动线程，并发度受 CPU 核心数限制（纯 std，无额外依赖）。
    // 单次 PrintWindow 捕获无法并行（同一时刻只能截一张），故仅在「编码落盘」这一步并行化。
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let mut iter = jobs.into_iter();
    let mut count = 0usize;
    while let Some(job) = iter.next() {
        let mut batch: Vec<Job> = Vec::with_capacity(cores);
        batch.push(job);
        for _ in 1..cores {
            match iter.next() {
                Some(j) => batch.push(j),
                None => break,
            }
        }
        let handles: Vec<_> = batch
            .into_iter()
            .map(|job| {
                let buf = buf.clone();
                std::thread::spawn(move || encode_job(&buf, bw, bh, job))
            })
            .collect();
        for h in handles {
            if h.join().unwrap_or(false) {
                count += 1;
            }
        }
    }

    CaptureMsg::Done {
        flash: format!("已归档 {count} 张区域截图（{caliber}）"),
        toast: format!("✓ 已归档 {count} 张：{caliber}"),
        ok: true,
    }
}

impl ObserverApp {
    fn new() -> Self {
        let manager = GlobalHotKeyManager::new()
            .expect("无法初始化全局快捷键管理器（global-hotkey）");

        // F8 已不再绑定：开始观察仅通过按钮点击触发。
        let f9 = HotKey::new(None, Code::F9);
        let f10 = HotKey::new(None, Code::F10);
        let f9_id = f9.id;
        let f10_id = f10.id;
        manager.register(f9).expect("注册 F9 快捷键失败");
        manager.register(f10).expect("注册 F10 快捷键失败");

        // 安装低级键盘钩子（WH_KEYBOARD_LL）：即便激活态把键盘焦点交给游戏，
        // 也能可靠捕获 F9/F10（global-hotkey 在游戏独占焦点时收不到）。不影响游戏输入（透传）。
        hotkey_hook::install();

        let info = Arc::new(config::build_config());
        // 口径列表直接从代码配置顶层 key 取得（12 Gauge 公共基准亦作为口径之一参与截图）。
        let caliber_keys: Vec<String> = info.keys().cloned().collect();

        Self {
            mode: Mode::Idle,
            status: String::new(),
            error: None,
            flash: None,
            toast: None,
            save_path: load_save_path(),
            font_ready: false,
            tab: 0,
            info,
            caliber_keys,
            selected: 0,
            annotate_full: false,
            tool_image_path: None,
            tool_rect: [0, 0, 200, 60],
            tool_texture: None,
            _manager: manager,
            f9_id,
            f10_id,
            capture_rx: None,
            observe_rx: None,
            observe_stop: None,
            observe_progress: None,
            observe_wait: None,
            interval_min: 30.0,
            viz_view: VizView::default(),
            viz_cache: None,
            viz_reload_requested: false,
        }
    }

    /// 加载系统 CJK 字体，避免中文显示为方块（tofu）。
    fn install_cjk_font(&self, ctx: &Context) {
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

        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".to_string(), egui::FontData::from_owned(bytes));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("cjk".to_string());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_string());
        ctx.set_fonts(fonts);
    }

    /// 进入激活态：聚焦游戏窗口 + 本面板收起为置顶毛玻璃悬浮面板（透明、穿透、排除截图）。
    /// 观察态还会在后台启动 OCR 抓取线程；截图态沿用原有截图逻辑。
    fn start(&mut self, kind: CaptureKind, ctx: &Context) {
        if let Mode::Active(_) = self.mode {
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

        self.mode = Mode::Active(kind);
        self.flash = None;
        self.error = None;
        self.status = match kind {
            CaptureKind::Observing => "观察中".to_string(),
            CaptureKind::Screenshot => "截图".to_string(),
        };

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

        // 按激活态启动对应后台任务。
        if let CaptureKind::Observing = kind {
            self.start_observation(ctx);
        }
    }

    /// 结束激活态：立即恢复完整界面（不透明、有边框、可交互、正常进截图），焦点归还本面板。
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
        self.flash = None;

        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(false));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(IDLE_SIZE.0, IDLE_SIZE.1)));
        apply_idle_size(ctx, self.tab);
        ctx.send_viewport_cmd(ViewportCommand::Focus);

        if let Some(my_hwnd) = win::find_window_by_keyword(SELF_TITLE) {
            win::set_exclude_from_capture(my_hwnd, false);
            win::set_window_blur(my_hwnd, false);
        }
    }

    /// 截图触发（F10 / 已处于截图激活态时）：在后台线程执行捕获与批量裁剪，
    /// 主线程立即返回，结果通过 `capture_rx` 通道异步回传（避免 UI 卡顿）。
    fn capture_screenshot(&mut self, _ctx: &Context) {
        // 已有截图任务在后台执行时忽略新的触发，避免重复/并发写同一目录。
        if self.capture_rx.is_some() {
            return;
        }
        let save_path = self.save_path.clone();
        let selected = self.selected;
        let annotate = self.annotate_full;
        let caliber = if selected == 0 {
            String::new()
        } else {
            self.caliber_keys[selected - 1].clone()
        };
        let info = self.info.clone();
        let (tx, rx) = mpsc::channel::<CaptureMsg>();
        self.capture_rx = Some(rx);
        // 后台线程：单次捕获 + 并行编码保存，完成后回传结果。
        std::thread::spawn(move || {
            let msg = run_capture(save_path, selected, annotate, caliber, info);
            let _ = tx.send(msg);
        });
    }

    /// 启动后台观察线程：遍历 config.rs 所有口径，逐一检索 + OCR 抓取价格并持久化。
    /// 结果通过 `observe_rx` 通道回传；`observe_stop` 供 F9 优雅中止。
    fn start_observation(&mut self, _ctx: &Context) {
        if self.observe_rx.is_some() {
            return; // 已有观察任务在后台运行，避免重复/并发写同一目录。
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let (tx, rx) = mpsc::channel::<ObserveMsg>();
        let save_path = self.save_path.clone();
        let info = self.info.clone();
        let interval_min = self.interval_min;
        self.observe_rx = Some(rx);
        self.observe_stop = Some(stop);
        self.observe_progress = None;
        self.observe_wait = None;
        std::thread::spawn(move || {
            observe::run_observation(save_path, info, stop_for_thread, tx, interval_min);
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
                ObserveMsg::Done { ok, flash, toast } => {
                    let now = ctx.input(|i| i.time);
                    self.flash = Some(flash);
                    self.toast = Some((toast, ok, now + if ok { 1.8 } else { 2.2 }));
                    win::play_capture_chime();
                    self.observe_rx = None;
                    self.observe_stop = None;
                    self.observe_progress = None;
                    self.observe_wait = None;
                    // 若仍在观察激活态（非 F9 提前结束导致已复位），恢复主面板。
                    if let Mode::Active(CaptureKind::Observing) = self.mode {
                        self.stop(ctx);
                    }
                    ctx.request_repaint();
                }
            }
        }
    }

    /// 每帧轮询快捷键事件。命中后强制重绘，保证 F9/F10 即时生效。
    ///
    /// 双通道：
    /// 1) `global-hotkey`（RegisterHotKey）—— 本窗口有焦点时可靠；
    /// 2) 低级键盘钩子 `hotkey_hook`（WH_KEYBOARD_LL）—— 即便激活态把键盘焦点交给游戏、
    ///    游戏以原始输入/全屏独占焦点，也能在按键分发前于本线程捕获 F9/F10，作为可靠兜底。
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
            } else if event.id == self.f10_id {
                self.handle_f10(ctx);
                handled = true;
            }
        }
        // 2) 低级键盘钩子兜底：消费式取走标志（即便游戏持焦点也能到达）。
        if hotkey_hook::take_f9() {
            self.handle_f9(ctx);
            handled = true;
        }
        if hotkey_hook::take_f10() {
            self.handle_f10(ctx);
            handled = true;
        }
        if handled {
            ctx.request_repaint();
        }
    }

    /// F9：观察态先置位中止信号让后台线程尽快退出，再复位 UI 回主控制面板。
    fn handle_f9(&mut self, ctx: &Context) {
        if matches!(self.mode, Mode::Active(CaptureKind::Observing)) {
            if let Some(stop) = &self.observe_stop {
                stop.store(true, Ordering::Relaxed);
            }
        }
        self.stop(ctx);
    }

    /// F10：Idle → 进入截图态；截图态 → 触发截图。
    fn handle_f10(&mut self, ctx: &Context) {
        match self.mode {
            Mode::Idle => self.start(CaptureKind::Screenshot, ctx),
            Mode::Active(CaptureKind::Screenshot) => self.capture_screenshot(ctx),
            _ => {}
        }
    }

    /// 每帧检查后台截图任务是否完成；完成后写入 flash/toast 并播放提示音（主线程，非阻塞）。
    fn poll_capture_result(&mut self, ctx: &Context) {
        let msg = match self.capture_rx.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(m) => Some(m),
                Err(_) => None,
            },
            None => None,
        };
        if let Some(CaptureMsg::Done { flash, toast, ok }) = msg {
            let now = ctx.input(|i| i.time);
            self.flash = Some(flash);
            self.toast = Some((toast, ok, now + if ok { 1.8 } else { 2.2 }));
            win::play_capture_chime();
            self.capture_rx = None;
            ctx.request_repaint();
        }
    }

    /// 行情看板数据懒加载：按 `records.jsonl` 的 mtime 仅在变更时重解析，
    /// 「刷新」按钮置位 `viz_reload_requested` 时强制重载。避免每帧重复解析大文件。
    fn viz_ensure(&mut self) {
        if self.viz_reload_requested {
            self.viz_reload_requested = false;
            self.viz_cache = None;
        }
        let mtime = self.records_mtime();
        let need = match (&self.viz_cache, mtime) {
            (None, _) => true,
            (Some((mt, _)), Some(m)) if *mt != m => true,
            _ => false,
        };
        if need {
            let data = viz::load(&self.save_path);
            self.viz_cache = Some((mtime.unwrap_or(0), data));
        }
    }

    /// 取 `records.jsonl` 的最后修改时间（毫秒），文件缺失返回 None。
    fn records_mtime(&self) -> Option<u64> {
        let p = format!("{}/prices/records.jsonl", self.save_path);
        let meta = std::fs::metadata(&p).ok()?;
        let modified = meta.modified().ok()?;
        let dur = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(dur.as_millis() as u64)
    }
}

/// 绘制一张带描边/圆角/阴影的卡片容器，用于清晰区分功能区块。
fn card(ui: &mut Ui, accent: Color32, content: impl FnOnce(&mut Ui)) {
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

/// 主控制面板（Idle 态）—— 独立组件，内含「开始观察 / 截图」两个标签页，不与其他面板共用。
struct ControlPanel;
impl ControlPanel {
    fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.heading("三角洲行动 · 交易行物价观察");
                ui.add_space(4.0);
                ui.label(
                    RichText::new("标签页：开始观察 / 截图（区域归档） / 行情看板 / 工具")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 220)),
                );
                ui.add_space(18.0);
            });

            // —— 标签页切换 ——
            ui.horizontal_wrapped(|ui| {
                let tabs = ["开始观察", "截图", "行情看板", "工具"];
                for (i, name) in tabs.iter().enumerate() {
                    if ui.selectable_label(app.tab == i, *name).clicked() {
                        let changed = app.tab != i;
                        app.tab = i;
                        app.error = None;
                        if changed {
                            apply_idle_size(ui.ctx(), i);
                        }
                    }
                }
            });
            ui.separator();
            ui.add_space(6.0);

            match app.tab {
                0 => Self::observe_tab(ui, app),
                1 => Self::capture_tab(ui, app),
                2 => Self::viz_tab(ui, app),
                _ => Self::tool_tab(ui, app),
            }
        });
    }

    /// 开始观察标签页。
    fn observe_tab(ui: &mut Ui, app: &mut ObserverApp) {
        let obs_accent = Color32::from_rgb(56, 135, 255);
        card(ui, obs_accent, |ui| {
            ui.heading(RichText::new("开始观察").color(obs_accent));
            ui.add_space(4.0);
            ui.label("聚焦游戏窗口并进入常驻毛玻璃悬浮状态，仅用于观察（不截图）。");
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

            ui.horizontal(|ui| {
                let btn = egui::Button::new(RichText::new("▶  开始观察").color(Color32::WHITE))
                    .min_size(egui::vec2(248.0, 44.0))
                    .fill(obs_accent)
                    .rounding(8.0);
                if ui.add(btn).clicked() {
                    app.start(CaptureKind::Observing, ui.ctx());
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

    /// 工具标签页：选取一张参考图片，填写截图区域坐标 `[x, y, w, h]`，
    /// 预览图实时叠加半透明高亮矩形，直观定位截图范围（用于校准 config 坐标）。
    fn tool_tab(ui: &mut Ui, app: &mut ObserverApp) {
        let tool_accent = Color32::from_rgb(150, 110, 60);
        card(ui, tool_accent, |ui| {
            ui.heading(RichText::new("工具 · 截图范围标定").color(tool_accent));
            ui.add_space(4.0);
            ui.label(
                "选取一张参考截图，填写截图区域坐标 [x, y, w, h]（相对图片左上角）；预览区将实时叠加高亮框定位该范围。",
            );
            ui.add_space(10.0);

            // —— 图片选择 ——
            ui.horizontal(|ui| {
                if ui.button("选择图片…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("图片", &["png", "jpg", "jpeg", "bmp"])
                        .pick_file()
                    {
                        if let Some(s) = p.to_str() {
                            app.tool_image_path = Some(s.to_string());
                            match image::open(s) {
                                Ok(img) => {
                                    let rgba = img.to_rgba8();
                                    let (w, h) =
                                        (rgba.width() as usize, rgba.height() as usize);
                                    let color = egui::ColorImage::from_rgba_unmultiplied(
                                        [w, h],
                                        &rgba.into_raw(),
                                    );
                                    let tex = ui.ctx().load_texture(
                                        "tool_preview",
                                        color,
                                        egui::TextureOptions::default(),
                                    );
                                    app.tool_texture = Some(tex);
                                    // 默认矩形设为整图一半，便于上手调整。
                                    app.tool_rect = [0, 0, (w as i32) / 2, (h as i32) / 2];
                                    app.error = None;
                                }
                                Err(e) => {
                                    app.error = Some(format!("图片解码失败：{e}"));
                                }
                            }
                        }
                    }
                }
                if let Some(p) = &app.tool_image_path {
                    ui.label(
                        RichText::new(p)
                            .small()
                            .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                    );
                }
            });

            ui.add_space(8.0);

            // —— 坐标输入：x / y / w / h ——
            ui.label("截图区域 x, y, w, h：");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut app.tool_rect[0])
                        .prefix("x ")
                        .clamp_range(0..=4096),
                );
                ui.add(
                    egui::DragValue::new(&mut app.tool_rect[1])
                        .prefix("y ")
                        .clamp_range(0..=4096),
                );
                ui.add(
                    egui::DragValue::new(&mut app.tool_rect[2])
                        .prefix("w ")
                        .clamp_range(0..=4096),
                );
                ui.add(
                    egui::DragValue::new(&mut app.tool_rect[3])
                        .prefix("h ")
                        .clamp_range(0..=4096),
                );
                if ui.button("复制坐标").clicked() {
                    let r = app.tool_rect;
                    ui.ctx().output_mut(|o| {
                        o.copied_text = format!("[{}, {}, {}, {}]", r[0], r[1], r[2], r[3]);
                    });
                }
            });

            ui.add_space(8.0);

            // —— 预览 + 叠加矩形 ——
            if let Some(tex) = &app.tool_texture {
                let max_w = ui.available_width().min(560.0);
                let img_size = tex.size_vec2();
                let scale = max_w / img_size.x;
                let preview = egui::vec2(max_w, img_size.y * scale);
                let resp = ui.add(
                    Image::new(tex)
                        .fit_to_exact_size(preview)
                        .rounding(6.0)
                        .sense(egui::Sense::hover()),
                );
                let [x, y, w, h] = app.tool_rect;
                let rx = resp.rect.min.x + x as f32 * scale;
                let ry = resp.rect.min.y + y as f32 * scale;
                let rw = (w as f32) * scale;
                let rh = (h as f32) * scale;
                let overlay =
                    egui::Rect::from_min_size(egui::pos2(rx, ry), egui::vec2(rw, rh));
                ui.painter().rect_filled(
                    overlay,
                    2.0,
                    Color32::from_rgba_unmultiplied(255, 70, 70, 40),
                );
                ui.painter().rect_stroke(
                    overlay,
                    2.0,
                    Stroke::new(2.0, Color32::from_rgb(255, 70, 70)),
                );
            } else {
                ui.label(
                    RichText::new("尚未选择图片。")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(150, 150, 150, 200)),
                );
            }
        });

        if let Some(err) = &app.error {
            ui.add_space(14.0);
            ui.centered_and_justified(|ui| {
                ui.colored_label(Color32::from_rgb(220, 80, 80), err);
            });
        }
    }

    /// 截图标签页：默认全屏；可选口径批量区域归档。
    fn capture_tab(ui: &mut Ui, app: &mut ObserverApp) {
        let shot_accent = Color32::from_rgb(16, 160, 150);
        card(ui, shot_accent, |ui| {
            ui.heading(RichText::new("截图 / 区域归档").color(shot_accent));
            ui.add_space(4.0);
            ui.label("默认截取当前窗口全屏图像；亦可选择口径，按代码配置（src/config.rs）批量截取各子弹的「名称 / 价格」区域并按层级归档。");
            ui.add_space(10.0);

            // 保存目录
            ui.horizontal(|ui| {
                ui.label("保存目录：");
                ui.add(
                    egui::TextEdit::singleline(&mut app.save_path)
                        .desired_width(220.0)
                        .hint_text("选择截图保存目录"),
                );
                if ui.button("浏览…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .set_title("选择截图保存目录")
                        .pick_folder()
                    {
                        if let Some(s) = p.to_str() {
                            app.save_path = s.to_string();
                            persist_save_path(&app.save_path);
                        }
                    }
                }
            });

            ui.add_space(8.0);

            // 口径下拉
            if app.caliber_keys.is_empty() {
                ui.label(
                    RichText::new(
                        "未找到口径配置：请检查 src/config.rs 中的口径定义。",
                    )
                    .small()
                    .color(Color32::from_rgb(200, 70, 70)),
                );
            } else {
                egui::ComboBox::from_label("子弹口径")
                    .selected_text(if app.selected == 0 {
                        "全屏截图（默认）".to_string()
                    } else {
                        app.caliber_keys[app.selected - 1].clone()
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(app.selected == 0, "全屏截图（默认）")
                            .clicked()
                        {
                            app.selected = 0;
                        }
                        for (i, k) in app.caliber_keys.iter().enumerate() {
                            if ui
                                .selectable_label(app.selected == i + 1, k)
                                .clicked()
                            {
                                app.selected = i + 1;
                            }
                        }
                    });
                ui.add_space(4.0);
                if app.selected == 0 {
                    ui.label(
                        RichText::new("当前：全屏模式 —— 按 F10 截取整个游戏窗口。")
                            .small()
                            .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "当前：{} —— 按 F10 截取该口径下每个子弹的名称与价格区域，归档至「保存目录 / 口径 / 子弹」目录。",
                            app.caliber_keys[app.selected - 1]
                        ))
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                    );
                }
            }

            // 整窗标注图复选框（仅在选择了具体口径时生效）
            ui.add_space(6.0);
            ui.checkbox(
                &mut app.annotate_full,
                "整窗标注图：口径截图时额外保存整窗截图，并框出 name / price 区域",
            );
            if app.selected == 0 {
                ui.label(
                    RichText::new("（该选项仅在选择具体口径时生效；默认全屏截图不会生成标注图）")
                        .small()
                        .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
                );
            }

            if let Some(msg) = &app.flash {
                ui.add_space(6.0);
                let ok = msg.starts_with("已");
                ui.label(
                    RichText::new(msg).small().color(if ok {
                        Color32::from_rgb(30, 150, 90)
                    } else {
                        Color32::from_rgb(200, 70, 70)
                    }),
                );
            }

            ui.add_space(10.0);
            let btn = egui::Button::new(RichText::new("▶  开始截图").color(Color32::WHITE))
                .min_size(egui::vec2(320.0, 44.0))
                .fill(shot_accent)
                .rounding(8.0);
            if ui.add(btn).clicked() {
                app.start(CaptureKind::Screenshot, ui.ctx());
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("开始截图：点击按钮 或 按 F10（按钮本身不绑定快捷键）")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(90, 100, 120, 200)),
            );
        });

        if let Some(err) = &app.error {
            ui.add_space(14.0);
            ui.centered_and_justified(|ui| {
                ui.colored_label(Color32::from_rgb(220, 80, 80), err);
            });
        }
    }

    /// 行情看板标签页：加载并展示识别结果可视化（概览 + 趋势图）。
    fn viz_tab(ui: &mut Ui, app: &mut ObserverApp) {
        viz::show(ui, app);
    }
}

/// 毛玻璃悬浮面板（激活态）—— 独立组件，不与其他面板共用。
struct FloatingPanel;
impl FloatingPanel {
    fn show(&self, ui: &mut Ui, app: &mut ObserverApp) {
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

    /// 测试观察态：毛玻璃浮层内以 3×3 网格（对应游戏交易行布局）实时展示各格名称与价格。
    /// 未识别到的格显示「—」，与「识别不出就不显示」的语义保持一致（仅以占位符提示该格无数据）。
    /// 单枚子弹的精致价格卡片（毛玻璃浮层内）：序号徽标 + 名称 + 大字号金色价格，
    /// 半透明白底圆角 + 柔和外描边 + 轻阴影，融入整体浅色毛玻璃界面。
    /// 观察 / 截图态：毛玻璃浮层内展示状态、进度与结束提示。
    fn status_body(ui: &mut Ui, app: &mut ObserverApp) {
        let (end_text, cap_text) = match app.mode {
            Mode::Active(CaptureKind::Observing) => ("按 F9 结束观察", None),
            Mode::Active(CaptureKind::Screenshot) => {
                let t = if app.selected == 0 {
                    "按 F10 截取全屏"
                } else {
                    "按 F10 归档区域"
                };
                ("按 F9 结束截图", Some(t))
            }
            _ => ("按 F9 结束", None),
        };
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(60, 200, 110), "●");
                ui.heading(RichText::new(&app.status).color(Color32::from_rgb(18, 28, 46)));
            });
            ui.add_space(8.0);
            if let Some(t) = cap_text {
                ui.label(RichText::new(t).color(Color32::from_rgb(24, 34, 56)));
            }
            if let Some((cycle, cal, idx, total, done)) = &app.observe_progress {
                ui.label(
                    RichText::new(format!(
                        "第 {} 轮 · 口径：{}（{}/{}）",
                        cycle,
                        cal,
                        idx + 1,
                        total
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
            ui.label(RichText::new(end_text).color(Color32::from_rgb(24, 34, 56)));
            ui.label(
                RichText::new("本面板已自动排除在截图之外")
                    .small()
                    .color(Color32::from_rgba_unmultiplied(70, 80, 100, 220)),
            );
        });
    }
}

impl App for ObserverApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.font_ready {
            self.install_cjk_font(ctx);
            self.font_ready = true;
        }

        self.poll_hotkeys(ctx);
        self.poll_capture_result(ctx);
        self.poll_observe(ctx);

        match self.mode {
            Mode::Idle => {
                egui::CentralPanel::default().show(ctx, |ui| ControlPanel.show(ui, self));
            }
            Mode::Active(_) => {
                egui::CentralPanel::default()
                    .frame(Frame::default().fill(Color32::TRANSPARENT))
                    .show(ctx, |ui| FloatingPanel.show(ui, self));
            }
        }

        // —— 截图成功 / 失败的毛玻璃轻量通知 ——
        // 位于本程序窗口内，自动被截图排除，不会污染游戏截图。
        let now = ctx.input(|i| i.time);
        if let Some((msg, ok, expiry)) = self.toast.as_ref() {
            if *expiry <= now {
                self.toast = None;
            } else {
                let total = if *ok { 1.8 } else { 2.2 };
                let remain = *expiry - now;
                let elapsed = total - remain;
                let alpha = if elapsed < 0.3 {
                    (elapsed / 0.3).clamp(0.0, 1.0)
                } else if remain < 0.4 {
                    (remain / 0.4).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let a = (210.0 * alpha) as u8;
                let (icon, fill_rgb, icon_rgb, text_rgb) = if *ok {
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
                                        RichText::new(msg).size(14.0),
                                    );
                                });
                            });
                    });
            }
        }

        // 保持事件循环持续重绘（约 30fps），确保 F9/F10 即时响应。
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}

fn main() -> eframe::Result<()> {
    let app = ObserverApp::new();

    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title(SELF_TITLE)
            .with_inner_size([IDLE_SIZE.0, IDLE_SIZE.1])
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        SELF_TITLE,
        options,
        Box::new(|_cc| Box::new(app) as Box<dyn eframe::App>),
    )
}
