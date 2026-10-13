//! 数量采集工具：后台线程循环采集「购买界面所选子弹数量」的**整块数字区域**截图，并点击
//! 「加号」按钮递增，直至累计截图达到上限（默认 200）后自动停止。
//!
//! 与早期「逐位裁剪百/十/个位」不同，本工具直接裁剪**整块数量显示区域**并保存——因为数量在
//! 画面中居中显示、不同位数对应不同坐标，逐位裁剪会错位；整块裁剪才是「数量整体识别」模型
//! （`nn-classifier/train_qty.py` → `qty.onnx`）所需的训练样本形态。
//!
//! 流程（每轮，截图与点击**严格串联**）：
//! 1. 对当前客户区截图，裁剪整块数量区域（按场景取 `BuyScene::qty_region`），按「当前应显示数量」
//!    标注为 `qty_dataset/<label>/`（`label` ∈ 1..200，**每个数量值只截一次**）；
//! 2. 点击「加号」按钮（数量 +1）；
//! 3. **等待画面刷新**（[`CLICK_SETTLE_MS`]），弥合「截图快于画面动作」的时序差，确保下一轮
//!    截到的是已 +1 的新数量，而非旧值被重复标注；
//! 4. 重复，直到累计截图次数达到 [`TOTAL_SHOTS`] 或数量值超出 1..200 区间。
//!
//! 采集产物 `qty_dataset/<label>/*.png` 即为「整块区域分类」训练集，可直接 `python train_qty.py train`。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::HWND;

use crate::config::{BuyScene, Rect, Scale};
use crate::screenshot::{CapturedImage, ClientCapturer, crop_region, save_rgba_png};
use crate::win::click_client_rect;
use crate::buttons;
use crate::log;

/// 数量采集工具：场景来源模式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SceneMode {
    /// 用 `buttons.onnx` 自动判定当前购买界面场景（检测失败时回退为可兑换子弹的场景）。
    Auto,
    /// 显式使用「可兑换子弹的场景」坐标。
    Exchangeable,
    /// 显式使用「不可兑换子弹的场景」坐标。
    NonExchangeable,
}

impl SceneMode {
    /// 由 UI 的 `u8` 选择值构造（0=自动 / 1=可兑换 / 2=不可兑换）。
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => SceneMode::Exchangeable,
            2 => SceneMode::NonExchangeable,
            _ => SceneMode::Auto,
        }
    }
}

/// 累计截图次数上限（达到后自动停止）。
pub const TOTAL_SHOTS: usize = 200;

/// 点击「加号」后等待购买界面数量刷新显示的时长（毫秒）。
///
/// 实测截图动作比画面动作快一倍以上：若不等画面更新就截下一帧，会截到旧值、造成同值重复/错标。
/// 故此等待用于弥合时序差，确保下一轮截到的是已 +1 的新数量（每个数量值只截一次）。
const CLICK_SETTLE_MS: u64 = 250;

/// 后台线程 → 主线程的消息。
pub enum QtyToolMsg {
    /// 进度：已截图次数 / 总次数。
    Progress { taken: usize, total: usize },
    /// 完成（含手动停止）：ok 表示是否正常结束，msg 为提示文案。
    Done { ok: bool, msg: String },
}

/// 将单帧的**整块数量区域**裁剪并保存到 `out_dir/qty_dataset/<label>/`。
///
/// 任一裁剪或落盘失败即返回 `false`（调用方可据此重试本帧，避免丢帧）。
/// 文件名基：`qty_{idx:05}_{phase}.png`，`phase` ∈ {shot}；`<label>` 即该帧整块区域表达的**真实
/// 数量值**（由调用方按「当前应显示数量」推算，每数量值只截一次，见 `run_qty_collection`）。
///
/// **保留已有图片、绝不覆盖**：若目标文件已存在（例如重跑采集、同一 label 多次采集），
/// 自动追加 `_1`/`_2`… 序号生成新文件名，原有 `*.png` 一律原样保留，不清空、不覆盖。
fn save_qty_region(
    frame: &CapturedImage,
    region: Rect,
    base_dir: &str,
    label: u32,
    idx: u64,
    phase: &str,
) -> bool {
    let Some((buf, w, h)) = crop_region(&frame.rgba, frame.width, frame.height, region) else {
        return false;
    };
    let dir = format!("{}/qty_dataset/{}", base_dir.trim_end_matches('/'), label);
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    // 同名检测：避免覆盖已有图片，保留文件夹内历史样本。
    let base = format!("qty_{:05}_{}", idx, phase);
    let mut fname = format!("{}.png", base);
    let mut suffix: u32 = 1;
    while Path::new(&dir).join(&fname).exists() {
        fname = format!("{}_{}.png", base, suffix);
        suffix += 1;
    }
    save_rgba_png(&dir, &fname, &buf, w as u32, h as u32).is_ok()
}

/// 运行数量采集循环（在后台线程中调用）。
///
/// - `hwnd`：已聚焦到前台的游戏窗口句柄（调用方负责 `focus_window`）；
/// - `scene_mode`：场景来源（自动检测 / 显式可兑换 / 显式不可兑换）；
/// - `out_dir`：采集输出根目录（绝对路径优先；函数会在其下建 `qty_dataset/<label>/`）；
/// - `start_qty`：采集开始时的「所选子弹数量」初始值（用户手动设定，默认 1，须 ∈ 1..200）。
///   第 `idx` 轮截图标注的数量值 = `start_qty + idx`（每点一次加号 +1），裁剪文件名中的 `<label>`
///   即真实整数值，直接构成带标注的训练集；每个数量值只截一次；
/// - `stop`：手动停止信号（主线程置位即优雅退出）；
/// - `tx`：进度/完成消息回传通道。
///
/// 循环内每轮依次：整块区域截图标注（当前数量值）→ 点击加号 → 等待画面刷新
/// （[`CLICK_SETTLE_MS`]，弥合时序差）→ 下一轮截下一数量值；**每个数量值只截一次**，避免同值重复/错标。
/// 累计截图达 [`TOTAL_SHOTS`] 或数量越界 1..200 自动停止。截图与落盘均为同步操作，
/// 单帧保存完成后才进入下一步，保证「每次截图完整保存」。
pub fn run_qty_collection(
    hwnd: HWND,
    scene_mode: SceneMode,
    out_dir: String,
    start_qty: u32,
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<QtyToolMsg>,
) {
    let _ = std::fs::create_dir_all(&out_dir);

    let capturer = ClientCapturer::new(hwnd);
    let client_w = capturer.client_width();
    let scale = Scale::for_client_width(client_w);

    // —— 场景判定（采集期间假定布局不变，仅判定一次）——
    let scene = match scene_mode {
        SceneMode::Exchangeable => Some(BuyScene::Exchangeable),
        SceneMode::NonExchangeable => Some(BuyScene::NonExchangeable),
        SceneMode::Auto => capturer
            .capture()
            .ok()
            .and_then(|f| buttons::detect_buttons_from_frame(&f, &scale))
            .and_then(|d| d.scene)
            .or_else(|| {
                log::write("[qty_tool] 自动检测场景失败，回退为「可兑换子弹的场景」");
                Some(BuyScene::Exchangeable)
            }),
    };
    let scene = match scene {
        Some(s) => s,
        None => {
            let _ = tx.send(QtyToolMsg::Done {
                ok: false,
                msg: "无法确定购买界面场景，已中止采集。".into(),
            });
            return;
        }
    };
    log::write(&format!(
        "[qty_tool] 场景={} 起始数量={start_qty} 开始采集（上限 {} 次）",
        scene.label(),
        TOTAL_SHOTS
    ));

    // —— 加号按钮位置：直接用已知场景的配置模板（标定视口坐标经缩放映射到实际客户区）。
    // 与整块区域裁剪同样走 `scale.rect`，坐标确定性正确，**不会**误点到「购买」按钮。
    // 注意：此处不使用检测器输出的 `d.plus` 作为点击目标——buy 与 plus 外形相近时，
    // 检测器可能把「购买」误判为「加号」，导致实际点到购买按钮（实机已观察到该现象）。
    let plus_rect = scale.rect(scene.layout().plus);

    // 整块数量区域（标定视口坐标经缩放映射到实际客户区）。
    let region_rect = scale.rect(scene.qty_region());

    let mut taken: usize = 0;
    let mut idx: u64 = 0;
    let mut cycles: u64 = 0;
    let max_cycles = TOTAL_SHOTS as u64 + 300; // 安全阀：避免截图持续失败时死循环

    // 当前应显示的整块数量值（即本帧截图要标注的真实整数值）。
    // 从 `start_qty` 起步、每点击一次加号 +1；用 clamp 兜底越界，保证落在分类区间 1..=200。
    let mut expected = start_qty.clamp(1, 200);

    while taken < TOTAL_SHOTS {
        if stop.load(Ordering::Relaxed) {
            let _ = tx.send(QtyToolMsg::Done {
                ok: true,
                msg: format!("已手动停止，共采集 {taken} 次截图。"),
            });
            return;
        }
        if cycles >= max_cycles {
            let _ = tx.send(QtyToolMsg::Done {
                ok: false,
                msg: format!("超出最大循环次数仍未能集满 {TOTAL_SHOTS} 次，已中止（已采集 {taken} 次）。"),
            });
            return;
        }
        cycles += 1;

        // 1) 截图当前整块数量区域：此时画面应正显示 `expected`，截一次即标注为该值。
        //    每个数量值只截一次，从根本上避免「同值重复 / 多次触发」导致的错标。
        let frame = match capturer.capture() {
            Ok(f) => f,
            Err(_) => {
                thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        if !save_qty_region(&frame, region_rect, &out_dir, expected, idx, "shot") {
            thread::sleep(Duration::from_millis(50));
            continue; // 保存失败重试本帧，不推进 idx / taken
        }
        taken += 1;
        idx += 1;
        let _ = tx.send(QtyToolMsg::Progress {
            taken,
            total: TOTAL_SHOTS,
        });
        if taken >= TOTAL_SHOTS {
            break; // 已达上限，不再点击加号
        }

        // 2) 点击「加号」按钮（数量 +1）。
        let _ = click_client_rect(hwnd, plus_rect);

        // 3) 等待画面真正刷新到 `expected + 1`，弥合「截图快于画面动作」的时序差：
        //    若不等画面更新就截下一帧，会截到旧值、造成同值重复/错标。等待后再进入下一轮。
        thread::sleep(Duration::from_millis(CLICK_SETTLE_MS));
        expected = expected.saturating_add(1);
        if expected > 200 {
            // 已超出分类区间 1..=200，无法再可靠标注，提前结束采集。
            break;
        }
    }

    log::write(&format!(
        "[qty_tool] 采集完成，共 {} 次截图，保存至 {}/qty_dataset/",
        taken, out_dir
    ));
    let _ = tx.send(QtyToolMsg::Done {
        ok: true,
        msg: format!("采集完成，共 {taken} 次截图已保存至 {out_dir}/qty_dataset/"),
    });
}
