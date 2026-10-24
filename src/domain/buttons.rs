//! 购买界面布局判定（ONNX Runtime / `ort`）—— 方案 A（纯查表）。
//!
//! 神经网络只判定当前购买界面属于哪一种固定布局：
//! - **场景1 · 可兑换子弹**（购买按钮位于上方一排，`BUY_LAYOUT_1`）；
//! - **场景2 · 不可兑换子弹**（购买按钮位于下方一排，`BUY_LAYOUT_2`）。
//!
//! 按钮坐标**直接写死查表**（见 `crate::config::BuyScene::layout`），不再由模型回归定位，
//! 彻底规避回归抖动，购买界面稳定。下游点击坐标全部取自 `scene.layout()` + `Scale::rect`
//! （见 `buy.rs`），本模块给出的 `buy/minus/plus` 仅用于日志与神经网络可视化画框。
//!
//! ## 前处理（须与 `train.py` 的 `layout_preprocess` 完全一致）
//! 全幅客户区 RGBA → 按实际客户区尺寸自适应缩放裁剪框 → 裁右下角子图
//! → 等比留边(letterbox) 256×256 → 丢 Alpha 取 RGB → `/255` → NCHW。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use image::imageops::FilterType;
use ort::session::Session;
use ort::value::Tensor;
use windows::Win32::Foundation::HWND;

use crate::config::{BuyScene, Rect, Scale};
use crate::screenshot::{CapturedImage, ClientCapturer};

/// ONNX 模型文件名（与 `train.py layout` 产物同名）。
const MODEL_NAME: &str = "layout.onnx";
/// 模型文件完整路径环境变量（优先级最高）。
const MODEL_ENV: &str = "DELTA_OBSERVER_BUTTONS_MODEL";
/// 模型所在目录环境变量（其次），在该目录下寻找 `layout.onnx`。
const MODEL_DIR_ENV: &str = "DELTA_OBSERVER_BUTTONS_MODEL_DIR";
/// 输入边长（正方形，像素），须与 `train.py` 的 `LAYOUT_INPUT_SIZE` 一致。
const INPUT_SIZE: usize = 256;
/// 标定视口（1920×1080）下的右下角裁剪框，须与 `train.py` 的 `LAYOUT_CROP` 一致。
/// 推理端会按实际客户区相对标定视口的缩放自适应（见 [`preprocess`]）。
const CROP_X0: f32 = 1433.0;
const CROP_Y0: f32 = 770.0;
const CROP_X1: f32 = 1829.0;
const CROP_Y1: f32 = 970.0;
const CALIB_W: f32 = 1920.0;
const CALIB_H: f32 = 1080.0;
/// 布局类别数（0=可兑换 / 1=不可兑换），须与模型输出通道数一致。
const NUM_LAYOUT_CLASSES: usize = 2;

/// 已加载的 ONNX 会话（进程内仅加载一次；模型缺失则为 `None`）。
/// `Mutex` 包裹以满足 `Session::run` 对 `&mut self` 的要求（观察在后台线程并发调用）。
static SESSION: OnceLock<Option<Mutex<Session>>> = OnceLock::new();

/// 定位 `layout.onnx`：环境变量 → exe 向上追溯 `nn-classifier/`、`assets/` → exe 同目录。
fn locate_model() -> Option<PathBuf> {
    if let Ok(p) = std::env::var(MODEL_ENV) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(d) = std::env::var(MODEL_DIR_ENV) {
        let p = PathBuf::from(d).join(MODEL_NAME);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..8 {
            let Some(d) = dir else { break };
            if let Some(found) = [
                d.join("nn-classifier").join(MODEL_NAME),
                d.join("assets").join(MODEL_NAME),
                d.join(MODEL_NAME),
            ]
            .into_iter()
            .find(|p| p.is_file())
            {
                return Some(found);
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    None
}

/// 惰性加载模型（仅首次调用触发；模型缺失则永久记 `None`）。
fn load_once() {
    SESSION.get_or_init(|| {
        let model_path = locate_model()?;
        let session = Session::builder().ok()?.commit_from_file(&model_path).ok()?;
        Some(Mutex::new(session))
    });
}

/// 模型是否可用（已成功加载 `layout.onnx`）。首次调用触发加载。
pub fn is_available() -> bool {
    load_once();
    SESSION.get().map(|o| o.is_some()).unwrap_or(false)
}

/// 单帧检测输出：三类按钮的 bbox（客户区坐标，来自查表）与判定的场景。
pub struct ButtonDet {
    /// 「购买」按钮 bbox（客户区坐标，`None` = 未检出）。
    pub buy: Option<Rect>,
    /// 「减号」按钮 bbox（`None` = 未检出）。
    pub minus: Option<Rect>,
    /// 「加号」按钮 bbox（`None` = 未检出）。
    pub plus: Option<Rect>,
    /// 判定的场景（`None` = 无法判定 / 模型未加载）。
    pub scene: Option<BuyScene>,
}

/// 前处理：自适应裁剪右下角 + 等比留边(letterbox) + RGB/255 + NCHW。
///
/// 与 `train.py` 的 `layout_preprocess` 逐像素对应：
/// 1. 裁剪框按实际客户区相对标定视口(1920×1080)的缩放自适应（保证裁到对应区域）；
/// 2. 裁剪子图等比缩放到 256×256 以内（短边铺满），居中贴到黑底（letterbox）；
/// 3. 丢弃 Alpha，取 RGB 通道，归一化到 [0,1]，按 CHW 排布。
fn preprocess(rgba: &[u8], w: i32, h: i32) -> Option<Vec<f32>> {
    if w <= 0 || h <= 0 || rgba.len() < (w as usize * h as usize * 4) {
        return None;
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec())?;

    // 裁剪框按实际客户区尺寸自适应缩放。
    let sw = w as f32 / CALIB_W;
    let sh = h as f32 / CALIB_H;
    let x0 = (CROP_X0 * sw).round().max(0.0) as u32;
    let y0 = (CROP_Y0 * sh).round().max(0.0) as u32;
    let x1 = ((CROP_X1 * sw).round() as u32).max(x0 + 1);
    let y1 = ((CROP_Y1 * sh).round() as u32).max(y0 + 1);
    let cw = (x1 - x0) as u32;
    let ch = (y1 - y0) as u32;

    // 手动裁剪到独立的 RgbaImage（避免 SubImage 跨 image 版本的类型歧义）。
    let mut crop_img = image::RgbaImage::new(cw, ch);
    for j in 0..ch {
        for i in 0..cw {
            let px = *img.get_pixel(x0 + i, y0 + j);
            crop_img.put_pixel(i, j, px);
        }
    }
    // 等比留边：短边缩放铺满 INPUT_SIZE，居中贴到黑底 256×256。
    let scale = (INPUT_SIZE as f32 / cw as f32).min(INPUT_SIZE as f32 / ch as f32);
    let sw2 = ((cw as f32 * scale).round() as u32).max(1);
    let sh2 = ((ch as f32 * scale).round() as u32).max(1);
    // resize 使用项目直接依赖的 image（与旧 preprocess 一致，已验证可编译）。
    let resized = image::imageops::resize(&crop_img, sw2, sh2, FilterType::Triangle);
    let mut canvas = image::RgbaImage::new(INPUT_SIZE as u32, INPUT_SIZE as u32);
    let ox = ((INPUT_SIZE as u32 - sw2) / 2) as u32;
    let oy = ((INPUT_SIZE as u32 - sh2) / 2) as u32;
    for j in 0..sh2 {
        for i in 0..sw2 {
            let px = *resized.get_pixel(i, j);
            canvas.put_pixel(ox + i, oy + j, px);
        }
    }

    let n = INPUT_SIZE * INPUT_SIZE;
    let mut data = vec![0f32; 3 * n];
    for y in 0..INPUT_SIZE {
        for x in 0..INPUT_SIZE {
            let px = canvas.get_pixel(x as u32, y as u32);
            let i = (y * INPUT_SIZE + x) as usize;
            // CHW：先排满 R 平面，再 G，再 B。
            data[i] = px[0] as f32 / 255.0;
            data[n + i] = px[1] as f32 / 255.0;
            data[2 * n + i] = px[2] as f32 / 255.0;
        }
    }
    Some(data)
}

/// 对单帧客户区截图跑布局分类，返回判定场景与查表坐标。
///
/// 与 [`detect_buttons`] 的区别：本函数不负责截图，调用方自行提供已截好的帧
/// （便于测试与复用既有 `ClientCapturer`）。返回 `None` 表示模型未加载 / 推理失败；
/// 否则给出场景与三类 bbox（坐标来自 `BuyScene::layout` 查表，经 `Scale` 映射到客户区）。
pub fn detect_buttons_from_frame(frame: &CapturedImage, scale: &Scale) -> Option<ButtonDet> {
    load_once();
    let mut guard = SESSION.get().and_then(|o| o.as_ref())?.lock().ok()?;

    let data = preprocess(&frame.rgba, frame.width, frame.height)?;
    let tensor =
        Tensor::<f32>::from_array(([1usize, 3, INPUT_SIZE, INPUT_SIZE], data)).ok()?;
    let outputs = guard.run(ort::inputs!["input" => &tensor]).ok()?;
    // 输出名与 `train.py` 的 export_layout_onnx 一致：`logits` [1, NUM_LAYOUT_CLASSES]。
    let logits = outputs["logits"]
        .try_extract_tensor::<f32>()
        .ok()?
        .1
        .iter()
        .copied()
        .collect::<Vec<f32>>();
    if logits.len() < NUM_LAYOUT_CLASSES {
        return None;
    }

    // argmax：0 = 可兑换子弹，1 = 不可兑换子弹。
    let mut best = 0usize;
    let mut best_v = logits[0];
    for i in 1..NUM_LAYOUT_CLASSES {
        if logits[i] > best_v {
            best_v = logits[i];
            best = i;
        }
    }
    let scene = match best {
        0 => BuyScene::Exchangeable,
        _ => BuyScene::NonExchangeable,
    };
    let layout = scene.layout();
    Some(ButtonDet {
        buy: Some(scale.rect(layout.buy)),
        minus: Some(scale.rect(layout.minus)),
        plus: Some(scale.rect(layout.plus)),
        scene: Some(scene),
    })
}

/// 对当前客户区截图跑布局分类，返回判定场景与查表坐标。
///
/// 便捷封装：内部 `ClientCapturer` 截图后转交 [`detect_buttons_from_frame`]。
pub fn detect_buttons(hwnd: HWND, scale: &Scale) -> Option<ButtonDet> {
    let cap = ClientCapturer::new(hwnd).capture().ok()?;
    detect_buttons_from_frame(&cap, scale)
}

/// 便捷封装：只取场景判定（购买界面打开期间调用）。
pub fn detect_scene(hwnd: HWND, scale: &Scale) -> Option<BuyScene> {
    detect_buttons(hwnd, scale).and_then(|d| d.scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{determine_scene, BuyScene, BUY_LAYOUT_1, BUY_LAYOUT_2};

    #[test]
    fn determine_scene_matches_layouts() {
        assert_eq!(
            determine_scene(BUY_LAYOUT_1.buy),
            Some(BuyScene::Exchangeable)
        );
        assert_eq!(
            determine_scene(BUY_LAYOUT_2.buy),
            Some(BuyScene::NonExchangeable)
        );
        // 远离两套模板的 bbox → 无法判定
        assert_eq!(determine_scene([10, 10, 20, 20]), None);
    }

    /// 端到端：加载真实训练出的 `layout.onnx`，对真实标注截图跑完整推理链路
    /// （前处理 → ORT 推理 → 场景分类），验证集成可正常工作且分类与标注布局一致。
    ///
    /// 该测试只验证「Rust 端 ONNX 推理 + 场景判定」全链路跑通并产出正确场景，
    /// **不**对模型逐帧识别精度做硬性断言（识别精度取决于训练，属模型层面问题；
    /// 场景映射正确性已由 `determine_scene_matches_layouts` 单测覆盖）。
    /// 依赖 `nn-classifier/layout.onnx` 与 `nn-classifier/button_dataset/` 标注（随仓库存在）。
    #[test]
    fn e2e_real_model_classifies_layout() {
        use crate::screenshot::CapturedImage;
        assert!(
            is_available(),
            "layout.onnx 未找到，无法跑端到端测试（先 `python train.py layout`）"
        );

        let anno_path = "nn-classifier/button_dataset/annotations.jsonl";
        let content = std::fs::read_to_string(anno_path).expect("读取标注文件失败");

        let parse_buy = |line: &str| -> Option<[i32; 4]> {
            let start = line.find("\"buy\":[")? + "\"buy\":[".len();
            let end = start + line[start..].find(']')?;
            let nums: Vec<i32> = line[start..end]
                .split(',')
                .filter_map(|x| x.trim().parse::<i32>().ok())
                .collect();
            (nums.len() == 4).then_some([nums[0], nums[1], nums[2], nums[3]])
        };
        let want = |b: [i32; 4]| -> BuyScene {
            if b == [1482, 829, 297, 61] {
                BuyScene::Exchangeable
            } else if b == [1484, 887, 293, 59] {
                BuyScene::NonExchangeable
            } else {
                panic!("未知布局 buy={b:?}")
            }
        };
        let parse_img = |line: &str| -> Option<String> {
            let s = line.find("\"image\":\"")? + "\"image\":\"".len();
            let e = s + line[s..].find('"')?;
            Some(line[s..e].to_string())
        };

        let mut classified = 0usize;
        let mut matched = 0usize;
        for line in content.lines().take(120) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let compact = line.replace(' ', "");
            let Some(buy) = parse_buy(&compact) else { continue };
            let Some(img_rel) = parse_img(&compact) else { continue };
            let img_path = format!("nn-classifier/button_dataset/{img_rel}");
            let Ok(frame) = CapturedImage::from_file(&img_path) else {
                continue;
            };
            let scale = Scale::for_client_width(frame.width);
            let Some(det) = detect_buttons_from_frame(&frame, &scale) else {
                continue;
            };
            // 链路跑通：分类得到场景即证明 ONNX 加载 + 前处理 + 推理全通。
            if let Some(scene) = det.scene {
                classified += 1;
                if scene == want(buy) {
                    matched += 1;
                }
            }
        }
        let rate = if classified > 0 {
            matched as f32 / classified as f32
        } else {
            0.0
        };
        eprintln!(
            "[e2e layout] 判定场景 {classified} 张、与标注一致 {matched} 张（一致率 {:.1}%）",
            rate * 100.0
        );
        // 集成正确性门槛：对真实模型稳定判定场景（≥5 张即证明 ONNX 加载/前处理/推理全通）。
        assert!(
            classified >= 5,
            "端到端未能在真实标注图上判定场景（ONNX 推理链路可能异常）"
        );
        // 方案 A 为查表式分类，标注集仅含两种固定布局，分类应近乎 100% 一致。
        assert!(
            rate >= 0.99,
            "布局分类一致率过低：{:.1}%（模型或前处理可能有问题）",
            rate * 100.0
        );
    }
}
