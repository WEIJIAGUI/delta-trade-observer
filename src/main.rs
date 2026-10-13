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

mod log;
mod domain;
pub use domain::{
    config, ocr, observe, buy, digit, nn_classifier, qty_classifier, recognition, records, buttons, qty_tool, toast,
};
mod platform;
pub use platform::{win, screenshot, hotkey_hook};
/// 导游模式协调器（跨模块共享：后台线程 `ask` 展示计划并等待，UI 主线程 `confirm` 放行）。
mod tour;
pub use tour::Tour;
mod ui;
pub use ui::{
    theme, control_panel, floating_panel, ocr_test_panel, price_viz_panel, qty_tool_panel,
};
pub use ui::price_viz_panel as price_viz;

use eframe::egui::viewport::{ViewportCommand, WindowLevel};
use eframe::egui::{self, Color32, Context, Frame, ViewportBuilder};
use eframe::App;

use global_hotkey::GlobalHotKeyEvent;
use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::HotKeyState;

use crate::config::{Config, GAME_KEYWORD};
use crate::observe::ObserveMsg;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// 本程序窗口标题（注意：不要包含「三角洲行动」，否则会误匹配到游戏窗口）。
const SELF_TITLE: &str = "交易行物价观察器";
/// 主控制面板（Idle 态）窗口尺寸（固定 1200×600，不可缩放）。
const IDLE_SIZE: (f32, f32) = (1200.0, 600.0);
/// 毛玻璃悬浮面板（激活态）窗口尺寸。
const FLOAT_SIZE: (f32, f32) = (320.0, 256.0);
/// 导游模式开启时的激活态窗口尺寸（更大，以容纳毛玻璃导游层与会议文字计划）。
const TOUR_FLOAT_SIZE: (f32, f32) = (500.0, 380.0);

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
    QtyTool, // 数量采集工具（采集购买界面数量数位截图 + 点击加号）
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

/// 「是否购买」配置文件路径（位于可执行文件同目录），用于持久化购买开关与阈值。
fn buy_cfg_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("buy.cfg"))
}

/// 购买口径处理顺序配置文件路径（与 `buy.cfg` 同目录）：每行一个口径名，记录 UI 拖拽调整后的顺序。
fn buy_order_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("buy_order.txt"))
}

/// 读取持久化的购买口径处理顺序（每行一个口径名）。文件不存在或为空时返回空列表，
/// 表示回退到「口径名排序」默认顺序；加载时不做与校准配置的校验（运行期再剔除/补尾）。
pub(crate) fn load_buy_order() -> Vec<String> {
    let mut order = Vec::new();
    if let Some(p) = buy_order_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            for line in s.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    order.push(line.to_string());
                }
            }
        }
    }
    order
}

/// 持久化购买口径处理顺序（每行一个口径名）。仅在拖拽落定后调用，不逐帧写盘。
pub(crate) fn persist_buy_order(order: &[String]) {
    if let Some(p) = buy_order_path() {
        let out = order
            .iter()
            .map(|c| format!("{}\n", c))
            .collect::<String>();
        let _ = std::fs::write(&p, out);
    }
}

/// 读取持久化的「是否购买」配置：返回 (开关, 阈值表)。
/// 阈值表键为 `(口径, 子弹)`，值为使用者填写的原始文本（空 = 不设阈值）。
pub(crate) fn load_buy_cfg() -> (bool, HashMap<(String, String), String>) {
    let mut enabled = false;
    let mut map: HashMap<(String, String), String> = HashMap::new();
    if let Some(p) = buy_cfg_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            for line in s.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(rest) = line.strip_prefix("enabled=") {
                    enabled = rest.trim() == "1";
                } else if let Some((key, val)) = line.split_once('=') {
                    if let Some((cal, bullet)) = key.split_once('|') {
                        map.insert((cal.to_string(), bullet.to_string()), val.trim().to_string());
                    }
                }
            }
        }
    }
    (enabled, map)
}

/// 将「是否购买」配置写入本地配置文件（持久化）。
///
/// 调用时机（与 `persist_save_path` 同思路，避免逐键写盘）：
/// - 勾选/取消「是否购买」复选框时；
/// - 任一阈值输入框失焦（完成一次编辑）时；
/// - 点击「开始观察」时（整组配置的最终落盘点）。
/// 仅持久化非空阈值，空值不写入（加载时自然表现为「未设置」）。
pub(crate) fn persist_buy_cfg(enabled: bool, thresholds: &HashMap<(String, String), String>) {
    if let Some(p) = buy_cfg_path() {
        let mut out = format!("enabled={}\n", if enabled { 1 } else { 0 });
        let mut keys: Vec<&(String, String)> = thresholds
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, _)| k)
            .collect();
        keys.sort();
        for k in keys {
            if let Some(v) = thresholds.get(k) {
                out.push_str(&format!("{}|{}={}\n", k.0, k.1, v.trim()));
            }
        }
        let _ = std::fs::write(&p, out);
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
    /// 后台观察任务的结果通道（OCR 抓取价格），完成后经此回传主线程（避免 UI 卡顿）。
    pub(crate) observe_rx: Option<mpsc::Receiver<ObserveMsg>>,
    /// 观察任务的中止信号（F9 置位），`AtomicBool` 跨线程共享。
    pub(crate) observe_stop: Option<Arc<AtomicBool>>,
    /// 观察进度：第几轮 / 当前口径名 / 口径序号 / 口径总数 / 已识别子弹数。
    pub(crate) observe_progress: Option<(usize, String, usize, usize, usize)>,
    /// 轮间等待：第几轮已完成、距下一轮剩余秒数。
    pub(crate) observe_wait: Option<(usize, u64)>,
    /// 购买阶段重验证识别到的价格：直接渲染到毛玻璃面板（仅显示数字），不弹提示框。
    pub(crate) buy_price: Option<u32>,
    /// 购买界面场景判定结果（按钮检测器据「购买」按钮位置判定）：展示于毛玻璃面板。
    pub(crate) buy_scene: Option<String>,
    /// 购买界面实时识别出的子弹数量（减/加调整过程中的当前值）：展示于毛玻璃面板。
    pub(crate) detected_qty: Option<u32>,
    /// 购买结果 toast 归类（正常购买成功 / 低价购买成功 / 未捕获）：展示于毛玻璃面板。
    pub(crate) buy_result: Option<String>,
    /// 购买后判断（toast 归类）按场景模式的核查记录：逐模式登记「是否已接入」与「上次实测结果」，
    /// 供毛玻璃面板「模式核查」区块展示。两种场景均经同一条已接入 toast 判定的购买路径（唯一点击点），
    /// 故接入状态恒为真；此处记录每次实测结果，便于实机逐模式验证。
    pub(crate) buy_result_by_scene: HashMap<crate::config::BuyScene, String>,
    /// 观察循环间隔（分钟）：每轮遍历全部口径后等待该时长再开始下一轮（默认 30）。
    pub(crate) interval_min: f64,
    /// 识别结果保存目录（已持久化，records.jsonl 直接写入该目录）。
    pub(crate) save_path: String,
    /// 是否把扫描阶段识别出的数字裁剪写入 `{digits_save_path}/scan/` 用于神经网络训练。
    /// 由「开始观察」面板「保存扫描阶段」复选框控制，**默认关闭**。
    pub(crate) save_scan_digits: bool,
    /// 是否把购买阶段识别出的数字裁剪写入 `{digits_save_path}/buy/` 用于神经网络训练。
    /// 由「开始观察」面板「保存购买阶段」复选框控制，**默认关闭**，与扫描阶段开关相互独立。
    pub(crate) save_buy_digits: bool,
    /// 数字裁剪输出根目录：扫描阶段写入 `scan/` 子目录、购买阶段写入 `buy/` 子目录。
    /// 默认 = 结果保存目录下的 `digits/`，可经 UI 自定义；任一保存开关勾选时显示与生效。
    pub(crate) digits_save_path: String,
    /// 是否保存购买界面的完整截图（PNG）：勾选后，购买阶段界面稳定时把整幅客户区截图写入独立目录，
    /// 便于人工复核购买界面布局 / 标定校准。默认关闭，与数字裁剪保存（`scan/`/`buy/`）完全独立。
    pub(crate) save_buy_ui: bool,
    /// 购买界面截图输出目录（独立设置，不共享 `digits_save_path`）。
    pub(crate) buy_ui_save_path: String,
    /// 是否购买：勾选后，观察时若某子弹价格低于其「最低接受价格」阈值，
    /// 自动点击价格左侧/上方区域并 ESC 恢复（轻量回退，直接继续下一颗子弹）。
    pub(crate) buy_enabled: bool,
    /// 最低接受价格：键为 `(口径, 子弹)`，值为使用者填写的文本（空 = 不设阈值）。
    /// 启动时解析为 `u32` 阈值传递给后台观察线程；配置持久化于 `buy.cfg`。
    pub(crate) buy_thresholds: HashMap<(String, String), String>,
    /// 目标购买数量：购买阶段重验证价格仍低于阈值时，识别当前所选子弹数量并点击减/加按钮调到该值
    /// （默认 31）。仅在「是否购买」开启时生效；面板可改，未持久化（每次启动回默认，避免误用）。
    pub(crate) buy_qty: u32,
    /// 购买处理顺序（口径级）：勾选「是否购买」后，口径卡片按此顺序展示，后台观察线程也按此顺序
    /// （前置口径全部子弹处理完后）依次处理。由 UI 拖拽调整并持久化；校准配置新增/移除的口径
    /// 会自动并入列表尾/剔除。空列表表示回退到「口径名排序」默认顺序。
    pub(crate) buy_caliber_order: Vec<String>,
    /// 拖拽排序临时状态：当前被拖拽的口径卡片索引（None = 未拖拽）。
    pub(crate) drag_src: Option<usize>,
    /// 拖拽排序临时状态：当前悬停（落点）的口径卡片索引（None = 无落点）。
    pub(crate) drop_dst: Option<usize>,
    /// 导游模式开关：开启后，观察过程中每一步操作（检索口径 / 进入购买 / 调整数量 / 确认购买等）
    /// 会先以会议文字形式展示在毛玻璃导游层，使用者回车确认后才执行；关闭则全自动运行。
    pub(crate) tour_mode: bool,
    /// 导游模式协调器（运行时实例）：后台观察线程经其 `ask` 展示计划并等待确认，UI 经其渲染导游层并
    /// `confirm` 放行。由「开始观察」按 `tour_mode` 构造（开启 `Tour::new` / 关闭 `Tour::disabled`），
    /// 整个观察周期内与后台线程共享同一份「待确认计划」与「确认信号」。
    pub(crate) tour: Tour,
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
    /// 数量采集工具：场景来源选择（0=自动检测 / 1=可兑换子弹 / 2=不可兑换子弹）。
    pub(crate) qty_tool_scene: u8,
    /// 数量采集工具：采集开始时的「所选子弹数量」初始值（用户手动设定，默认 0）。
    /// 第 `idx` 轮点击前数量 = `qty_tool_start + idx`，用于给采集样本标注真实整数值。
    pub(crate) qty_tool_start: u32,
    /// 数量采集工具：数位裁剪输出目录（默认 = 保存目录下的 `qty_tool/`）。
    pub(crate) qty_tool_dir: String,
    /// 数量采集工具：是否正在采集。
    pub(crate) qty_tool_running: bool,
    /// 数量采集工具：已采集截图次数（实时展示）。
    pub(crate) qty_tool_taken: usize,
    /// 数量采集工具：后台线程结果通道。
    pub(crate) qty_tool_rx: Option<mpsc::Receiver<crate::qty_tool::QtyToolMsg>>,
    /// 数量采集工具：手动停止信号（主线程置位，后台线程轮询）。
    pub(crate) qty_tool_stop: Option<Arc<AtomicBool>>,
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

        let (buy_enabled, buy_thresholds) = load_buy_cfg();

        Self {
            mode: Mode::Idle,
            status: String::new(),
            error: None,
            save_path: load_save_path(),
            save_scan_digits: false,
            save_buy_digits: false,
            digits_save_path: format!("{}/digits", load_save_path()),
            save_buy_ui: false,
            buy_ui_save_path: format!("{}/buy_ui", load_save_path()),
            buy_enabled,
            buy_thresholds,
            buy_qty: 31,
            buy_caliber_order: load_buy_order(),
            drag_src: None,
            drop_dst: None,
            tour_mode: false,
            tour: Tour::disabled(),
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
            qty_tool_scene: 0,
            qty_tool_start: 1,
            qty_tool_dir: format!("{}/qty_tool", load_save_path()),
            qty_tool_running: false,
            qty_tool_taken: 0,
            qty_tool_rx: None,
            qty_tool_stop: None,
            observe_rx: None,
            observe_stop: None,
            observe_progress: None,
            observe_wait: None,
            buy_price: None,
            buy_scene: None,
            detected_qty: None,
            buy_result: None,
            buy_result_by_scene: HashMap::new(),
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
        // 「是否购买」配置在此刻整组落盘一次（最终落盘点）。
        persist_buy_cfg(self.buy_enabled, &self.buy_thresholds);

        // 收起为置顶毛玻璃悬浮面板（透明、穿透、排除截图）。
        self.enter_active_mode(ctx, "观察中");

        // 启动后台观察线程。
        self.start_observation();
    }

    /// 进入激活态：聚焦游戏窗口 + 本面板收起为置顶毛玻璃悬浮面板（透明、穿透、排除截图）。
    /// 「开始观察」与「数量采集」工具共用此入口，避免重复的视口命令堆叠。
    fn enter_active_mode(&mut self, ctx: &Context, status: &str) {
        self.mode = Mode::Active;
        self.error = None;
        self.status = status.to_string();
        self.buy_price = None;
        self.buy_scene = None;
        self.detected_qty = None;
        self.buy_result = None;

        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
        // 激活态保持鼠标穿透，不遮挡游戏操作（面板仅作状态展示）。
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));

        // 激活态窗口尺寸：导游模式开启时用更大尺寸，以容纳毛玻璃导游层与会议文字计划。
        let float_size = if self.tour_mode { TOUR_FLOAT_SIZE } else { FLOAT_SIZE };
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(float_size.0, float_size.1)));
        let screen = ctx.input(|i| i.screen_rect);
        let x = screen.max.x - float_size.0 - 24.0;
        let y = 24.0;
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(x, y)));

        if let Some(my_hwnd) = win::find_window_by_keyword(SELF_TITLE) {
            win::set_exclude_from_capture(my_hwnd, true);
            win::set_window_blur(my_hwnd, true);
        }
    }

    /// 结束激活态：立即恢复完整界面（不透明、有边框、可交互），焦点归还本面板。
    fn stop(&mut self, ctx: &Context) {
        if self.mode == Mode::Idle {
            return;
        }
        // 若导游模式有未确认的计划卡住后台线程，先放行，使其随后据 stop 自行退出
        // （否则线程会永久阻塞在 `Tour::ask` 的条件变量上，无法随 stop 退出）。
        if self.tour.pending_text().is_some() {
            self.tour.confirm();
        }
        // 中止仍在运行的后台观察线程，避免悬挂任务继续占用游戏窗口。
        if let Some(stop) = &self.observe_stop {
            stop.store(true, Ordering::Relaxed);
        }
        // 中止数量采集工具（若正在运行）：F9 / 采集完成均经此统一退出。
        if let Some(stop) = &self.qty_tool_stop {
            stop.store(true, Ordering::Relaxed);
        }
        self.mode = Mode::Idle;
        self.status.clear();
        self.error = None;

        // 复位数量采集状态（后台线程随后仍会发 Done，此处先清避免面板残留）。
        self.qty_tool_running = false;
        self.qty_tool_rx = None;
        self.qty_tool_stop = None;
        self.qty_tool_taken = 0;

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
        let save_scan_digits = self.save_scan_digits;
        let save_buy_digits = self.save_buy_digits;
        let digits_dir = self.digits_save_path.clone();
        let save_buy_ui = self.save_buy_ui;
        let buy_ui_save_path = self.buy_ui_save_path.clone();
        // 面板设定的「目标购买数量」：透传给后台观察线程，购买阶段据当前识别数点击减/加按钮调到该值。
        let buy_qty = self.buy_qty;
        // 购买口径处理顺序：透传给后台观察线程，控制各口径的先后处理顺序（缺漏口径运行期补尾）。
        let buy_caliber_order = self.buy_caliber_order.clone();
        // 导游模式：按开关构造协调器（开启 `Tour::new` / 关闭 `Tour::disabled`），
        // 后台线程经其 `ask` 展示计划并等待确认；UI 经 App 持有的 `tour` 渲染导游层并 `confirm` 放行。
        let tour = if self.tour_mode {
            Tour::new()
        } else {
            Tour::disabled()
        };
        self.tour = tour.clone();
        // 解析「是否购买」阈值：仅保留非空且可解析为正整数者，供后台线程直接使用。
        let buy_enabled = self.buy_enabled;
        let mut buy_thresholds: HashMap<(String, String), u32> = HashMap::new();
        for ((c, b), v) in &self.buy_thresholds {
            let t = v.trim();
            if !t.is_empty() {
                if let Ok(n) = t.parse::<u32>() {
                    buy_thresholds.insert((c.clone(), b.clone()), n);
                }
            }
        }
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
                save_scan_digits,
                save_buy_digits,
                digits_dir,
                buy_enabled,
                buy_thresholds,
                save_buy_ui,
                buy_ui_save_path,
                buy_qty,
                buy_caliber_order,
                tour,
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
                ObserveMsg::BuyNotice { price } => {
                    // 购买阶段重验证识别到的价格：直接渲染到毛玻璃面板（仅显示数字），不弹提示框。
                    self.buy_price = Some(price);
                    ctx.request_repaint();
                }
                ObserveMsg::BuySceneNotice { label } => {
                    // 购买界面场景判定（可兑换子弹的场景 / 不可兑换子弹的场景）：毛玻璃面板展示。
                    self.buy_scene = Some(label);
                    ctx.request_repaint();
                }
                ObserveMsg::DetectedQty { qty } => {
                    // 购买阶段实时识别到的「所选子弹数量」：毛玻璃面板实时展示（数量调整过程中持续刷新）。
                    self.detected_qty = qty;
                    ctx.request_repaint();
                }
                ObserveMsg::BuyResultNotice { label, scene } => {
                    // 购买结果 toast 归类（点击购买后监听游戏顶部弹窗）：毛玻璃面板展示，并按场景记入核查表。
                    self.buy_result = Some(label.clone());
                    if let Some(s) = scene {
                        self.buy_result_by_scene.insert(s, label);
                    }
                    ctx.request_repaint();
                }
                ObserveMsg::BuyCleared => {
                    // 进入新一颗子弹的购买判定前，清空上一颗子弹残留的购买相关展示。
                    self.buy_price = None;
                    self.buy_scene = None;
                    self.detected_qty = None;
                    self.buy_result = None;
                    ctx.request_repaint();
                }
                ObserveMsg::Done { ok, toast } => {
                    // Notification 组件已移除：不再弹提示框。失败时保留错误信息到主面板 error 字段，避免静默丢失。
                    if !ok {
                        self.error = Some(toast);
                    }
                    win::play_capture_chime();
                    self.observe_rx = None;
                    self.observe_stop = None;
                    self.observe_progress = None;
                    self.observe_wait = None;
                    self.buy_price = None;
                    self.buy_scene = None;
                    self.detected_qty = None;
                    self.buy_result = None;
                    self.buy_result_by_scene.clear();
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

    /// 启动数量采集工具：聚焦并前置游戏窗口，收起为毛玻璃悬浮面板，
    /// 随后在后台线程循环采集数量数位截图并点击加号。
    ///
    /// 程序不负责将游戏切换到购买界面（使用者手动完成）；点击后仅聚焦窗口，
    /// 真正的采集逻辑跑在 [`crate::qty_tool::run_qty_collection`] 的后台线程中，避免 UI 卡顿。
    /// 采集期间本面板以毛玻璃悬浮层呈现（与观察态共用），F9 可随时结束采集并恢复主面板。
    fn start_qty_tool(&mut self, ctx: &Context) {
        if self.qty_tool_running {
            return;
        }
        // 若观察任务在跑，不冲突（二者独立），但为避免抢窗口焦点，这里同样要求先定位游戏窗口。
        let Some(hwnd) = win::find_window_by_keyword(crate::config::GAME_KEYWORD) else {
            self.error = Some("未检测到游戏窗口，请先启动游戏后再试。".into());
            return;
        };
        win::focus_window(hwnd);

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let (tx, rx) = mpsc::channel::<crate::qty_tool::QtyToolMsg>();
        let scene_mode = crate::qty_tool::SceneMode::from_u8(self.qty_tool_scene);
        let out_dir = self.qty_tool_dir.clone();
        let start_qty = self.qty_tool_start;

        self.qty_tool_rx = Some(rx);
        self.qty_tool_stop = Some(stop);
        self.qty_tool_running = true;
        self.qty_tool_taken = 0;
        self.error = None;

        // 收起为毛玻璃悬浮面板（与观察态共用入口），实时展示采集进度且不遮挡游戏操作。
        self.enter_active_mode(ctx, "数量采集中");

        std::thread::spawn(move || {
            crate::qty_tool::run_qty_collection(
                hwnd,
                scene_mode,
                out_dir,
                start_qty,
                stop_for_thread,
                tx,
            );
        });
    }

    /// 手动停止数量采集（仅置位停止信号，由后台线程在下一轮轮询时优雅退出）。
    fn stop_qty_tool(&mut self) {
        if let Some(stop) = &self.qty_tool_stop {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// 每帧检查数量采集工具的消息：更新已采集截图计数，完成时复位状态。
    fn poll_qty_tool(&mut self, ctx: &Context) {
        if !self.qty_tool_running {
            return;
        }
        let msg = match self.qty_tool_rx.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(m) => Some(m),
                Err(_) => None,
            },
            None => None,
        };
        if let Some(m) = msg {
            match m {
                crate::qty_tool::QtyToolMsg::Progress { taken, .. } => {
                    self.qty_tool_taken = taken;
                    ctx.request_repaint();
                }
                crate::qty_tool::QtyToolMsg::Done { ok, msg } => {
                    if !ok {
                        self.error = Some(msg);
                    }
                    win::play_capture_chime();
                    // 复位数量采集状态（线程已结束），并退出毛玻璃悬浮面板回到完整主面板。
                    self.qty_tool_running = false;
                    self.qty_tool_rx = None;
                    self.qty_tool_stop = None;
                    self.qty_tool_taken = 0;
                    if let Mode::Active = self.mode {
                        self.stop(ctx);
                    }
                    ctx.request_repaint();
                }
            }
        }
    }
}

impl App for ObserverApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // 全透明清屏：让 DWM 背景模糊（毛玻璃）透出桌面，而非 eframe 默认的深灰半透明底色（黑色背景根因）。
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.font_ready {
            crate::ui::theme::install_cjk_font(ctx);
            // 统一白色主题：覆盖价格可视化 Tab 原有的深色背景，使各 Tab 风格一致。
            ctx.set_visuals(egui::Visuals::light());
            self.font_ready = true;
        }

        self.poll_hotkeys(ctx);
        self.poll_observe(ctx);
        self.poll_qty_tool(ctx);

        // 导游模式：若有待确认计划，聚焦本窗口以接收回车（避免回车泄漏到游戏），
        // 并检测回车确认放行。仅当存在待确认计划时才抢焦点，确认后由后台线程在下一步动作前交还游戏焦点。
        if self.tour.pending_text().is_some() {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.tour.confirm();
            }
        }

        match self.mode {
            Mode::Idle => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical(|ui| {
                        // Tab 切换栏：各 Tab 界面与处理逻辑相互独立（UI 只采集/展示，识别走 `recognition` 模块）。
                        ui.horizontal(|ui| {
                            crate::ui::theme::tab_button(ui, "开始观察", MainTab::Observe, &mut self.active_tab);
                            crate::ui::theme::tab_button(ui, "OCR 测试", MainTab::OcrTest, &mut self.active_tab);
                            crate::ui::theme::tab_button(ui, "价格可视化", MainTab::PriceViz, &mut self.active_tab);
                            crate::ui::theme::tab_button(ui, "数量采集", MainTab::QtyTool, &mut self.active_tab);
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
                            MainTab::QtyTool => {
                                crate::ui::qty_tool_panel::QtyToolPanel.show(ui, self);
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

        // 保持事件循环持续重绘（约 30fps），确保 F9 即时响应。
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}

fn main() -> eframe::Result<()> {
    // 每次启动清空并初始化运行期排查日志（observe.log），避免旧日志跨班次累积消耗。
    crate::log::init();

    // 声明 Per-Monitor V2 DPI 感知：使 `GetWindowRect`/`GetClientRect`/`ClientToScreen`/
    // `PrintWindow` 全部以设备像素取舍，缩放 ≠ 100% 时不再产生尺寸错位与黑边。
    // 必须在创建任何窗口（eframe 的 run_native）之前调用，否则会被后续默认设置拒绝。
    // 返回值必须可见化：若被 manifest / 框架预先设置拒绝（E_ACCESSDENIED），进程会退回
    // DPI-unaware，导致 `ClientToScreen` 坐标虚拟化错位却无从察觉——这正是坐标错乱的隐患。
    unsafe {
        use windows::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        let r = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        match r {
            Ok(()) => crate::log::write("[DPI] 已声明 Per-Monitor V2 DPI 感知（设备像素基准）"),
            Err(e) => crate::log::write(&format!(
                "[DPI] Per-Monitor V2 声明失败（{:?}）：进程可能仍为 DPI-unaware，ClientToScreen 坐标或受虚拟化影响",
                e
            )),
        }
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
