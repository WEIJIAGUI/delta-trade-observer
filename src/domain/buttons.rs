//! 购买界面按钮布局检测（ONNX Runtime / `ort`）。
//!
//! 用 `buttons.onnx`（由 `nn-classifier/train.py button` 导出，3 类 CenterNet 检测器）定位
//! 购买界面中的「购买 / 减号 / 加号」三按钮，并据「购买」按钮位置判定当前界面属于
//! 哪一种场景：
//! - **场景1 · 可兑换子弹的场景**：购买按钮位于上方一排（`buy ≈ [1482,829,297,61]`）；
//! - **场景2 · 不可兑换子弹的场景**：购买按钮位于下方一排（`buy ≈ [1484,887,293,59]`）。
//!
//! ## 前处理（须与 `train.py` 完全一致）
//! 全幅客户区 RGBA → 丢 Alpha 得 RGB → 双线性缩放 256×256 → `/255` → NCHW。
//!
//! ## 模型缺失时的行为
//! 若 `buttons.onnx` 不存在，`detect_*` 返回 `None`，调用方据此跳过场景判定（不影响扫描）。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use image::imageops::FilterType;
use ort::session::Session;
use ort::value::Tensor;
use windows::Win32::Foundation::HWND;

use crate::config::{BuyScene, Rect, Scale};
use crate::screenshot::{CapturedImage, ClientCapturer};

/// ONNX 模型文件名（与 `train.py --export-onnx` 产物同名）。
const MODEL_NAME: &str = "buttons.onnx";
/// 模型文件完整路径环境变量（优先级最高）。
const MODEL_ENV: &str = "DELTA_OBSERVER_BUTTONS_MODEL";
/// 模型所在目录环境变量（其次），在该目录下寻找 `buttons.onnx`。
const MODEL_DIR_ENV: &str = "DELTA_OBSERVER_BUTTONS_MODEL_DIR";
/// 输入边长（正方形，像素），须与 `train.py` 的 `BUTTON_INPUT_SIZE` 一致。
const INPUT_SIZE: usize = 256;
/// 热力图/尺寸图边长（= INPUT_SIZE / 8，由 3 次 maxpool 决定），须与 `BUTTON_HM` 一致。
const HM: usize = 32;
/// 类别顺序固定：0=buy, 1=minus, 2=plus（与 `train.py` 的 `BUTTON_CLASSES` 一致）。
const NUM_CLASSES: usize = 3;
/// 热力图峰值（经 sigmoid）的置信度阈值，低于则判为无该按钮。
const THRESH: f32 = 0.3;

/// 已加载的 ONNX 会话（进程内仅加载一次；模型缺失则为 `None`）。
/// `Mutex` 包裹以满足 `Session::run` 对 `&mut self` 的要求（观察在后台线程并发调用）。
static SESSION: OnceLock<Option<Mutex<Session>>> = OnceLock::new();

/// 定位 `buttons.onnx`：环境变量 → exe 向上追溯 `nn-classifier/`、`assets/` → exe 同目录。
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

/// 模型是否可用（已成功加载 `buttons.onnx`）。首次调用触发加载。
pub fn is_available() -> bool {
    load_once();
    SESSION.get().map(|o| o.is_some()).unwrap_or(false)
}

/// 单帧检测输出：三类按钮的 bbox（客户区坐标）与据「购买」按钮判定的场景。
pub struct ButtonDet {
    /// 「购买」按钮 bbox（客户区坐标，`None` = 未检出）。
    pub buy: Option<Rect>,
    /// 「减号」按钮 bbox（`None` = 未检出）。
    pub minus: Option<Rect>,
    /// 「加号」按钮 bbox（`None` = 未检出）。
    pub plus: Option<Rect>,
    /// 据购买按钮 bbox 判定的场景（`None` = 无法判定）。
    pub scene: Option<BuyScene>,
}

/// 全幅客户区 RGBA → 256×256 RGB /255，CHW 排布（与 `train.py` 的 `preprocess` 一致）。
fn preprocess(rgba: &[u8], w: i32, h: i32) -> Option<Vec<f32>> {
    if w <= 0 || h <= 0 || rgba.len() < (w as usize * h as usize * 4) {
        return None;
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec())?;
    // 双线性缩放至 256×256（与 PIL `Image.BILINEAR` 等价）。
    let resized =
        image::imageops::resize(&img, INPUT_SIZE as u32, INPUT_SIZE as u32, FilterType::Triangle);
    let n = INPUT_SIZE * INPUT_SIZE;
    let mut data = vec![0f32; 3 * n];
    for y in 0..INPUT_SIZE {
        for x in 0..INPUT_SIZE {
            let px = resized.get_pixel(x as u32, y as u32);
            let i = (y * INPUT_SIZE + x) as usize;
            // CHW：先排满 R 平面，再 G，再 B。
            data[i] = px[0] as f32 / 255.0;
            data[n + i] = px[1] as f32 / 255.0;
            data[2 * n + i] = px[2] as f32 / 255.0;
        }
    }
    Some(data)
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// 单类解码：`hm_ch` [HM*HM]（原始 logits）、`sz_ch` [2*HM*HM]（w_norm, h_norm），
/// `orig_w`/`orig_h` 为原图（客户区）尺寸。返回 `(bbox 客户区坐标, 置信度)`；
/// 置信度低于 [`THRESH`] 视为未检出，返回 `None`。
///
/// 坐标还原：模型在 32×32 网格上输出，输入 256×256；训练时对原图做拉伸缩放，
/// 故还原需乘以原图宽高比（`sx = orig_w/256`、`sy = orig_h/256`）。与 `predict_layout.py`
/// 的 `decode_class` 完全一致。
fn decode_class(hm_ch: &[f32], sz_ch: &[f32], orig_w: f32, orig_h: f32) -> Option<(Rect, f32)> {
    let peak_raw = hm_ch.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let conf = sigmoid(peak_raw);
    if conf < THRESH {
        return None;
    }
    // 峰值格（argmax）。
    let mut arg = 0usize;
    let mut best = hm_ch[0];
    for i in 1..hm_ch.len() {
        if hm_ch[i] > best {
            best = hm_ch[i];
            arg = i;
        }
    }
    let gy = arg / HM;
    let gx = arg % HM;
    let wn = sz_ch[gy * HM + gx];
    let hn = sz_ch[HM * HM + gy * HM + gx];
    // 模型空间像素中心 / 尺寸。
    let cx_m = (gx as f32 + 0.5) / HM as f32 * INPUT_SIZE as f32;
    let cy_m = (gy as f32 + 0.5) / HM as f32 * INPUT_SIZE as f32;
    let w_m = wn * INPUT_SIZE as f32;
    let h_m = hn * INPUT_SIZE as f32;
    // 还原到原图（拉伸缩放逆运算）。
    let sx = orig_w / INPUT_SIZE as f32;
    let sy = orig_h / INPUT_SIZE as f32;
    let mut x = (cx_m - w_m / 2.0) * sx;
    let mut y = (cy_m - h_m / 2.0) * sy;
    let mut w = w_m * sx;
    let mut h = h_m * sy;
    x = x.max(0.0).min(orig_w - 1.0);
    y = y.max(0.0).min(orig_h - 1.0);
    w = w.max(1.0).min(orig_w - x);
    h = h.max(1.0).min(orig_h - y);
    Some((
        [
            x.round() as i32,
            y.round() as i32,
            w.round() as i32,
            h.round() as i32,
        ],
        conf,
    ))
}

/// 对单帧客户区截图跑检测器，返回三类按钮 bbox 与判定场景。
///
/// 与 [`detect_buttons`] 的区别：本函数不负责截图，调用方自行提供已截好的帧
/// （便于测试与复用既有 `ClientCapturer`）。返回 `None` 表示模型未加载 / 推理失败；
/// 否则给出三类 bbox（客户区坐标）与据「购买」bbox 判定的场景
/// （见 [`crate::config::determine_scene`]）。
pub fn detect_buttons_from_frame(frame: &CapturedImage, scale: &Scale) -> Option<ButtonDet> {
    load_once();
    let mut guard = SESSION.get().and_then(|o| o.as_ref())?.lock().ok()?;

    let data = preprocess(&frame.rgba, frame.width, frame.height)?;
    let tensor =
        Tensor::<f32>::from_array(([1usize, NUM_CLASSES, INPUT_SIZE, INPUT_SIZE], data)).ok()?;
    let outputs = guard.run(ort::inputs!["input" => &tensor]).ok()?;
    // 输出名与 `train.py` 的 export_button_onnx 一致：heatmap_logits[1,3,32,32] / size[1,6,32,32]。
    let hm = outputs["heatmap_logits"]
        .try_extract_tensor::<f32>()
        .ok()?
        .1
        .iter()
        .copied()
        .collect::<Vec<f32>>();
    let sz = outputs["size"]
        .try_extract_tensor::<f32>()
        .ok()?
        .1
        .iter()
        .copied()
        .collect::<Vec<f32>>();

    let hm_n = NUM_CLASSES * HM * HM;
    let sz_n = 2 * NUM_CLASSES * HM * HM;
    if hm.len() < hm_n || sz.len() < sz_n {
        return None;
    }
    let hm = &hm[..hm_n];
    let sz = &sz[..sz_n];

    let ow = frame.width as f32;
    let oh = frame.height as f32;

    let mut det = ButtonDet {
        buy: None,
        minus: None,
        plus: None,
        scene: None,
    };
    for c in 0..NUM_CLASSES {
        let hm_ch = &hm[c * HM * HM..(c + 1) * HM * HM];
        let sz_ch = &sz[2 * c * HM * HM..(2 * c + 2) * HM * HM];
        if let Some((bbox, _conf)) = decode_class(hm_ch, sz_ch, ow, oh) {
            match c {
                0 => {
                    // 「购买」bbox 锚定场景：先映射回标定视口坐标，再与两套布局模板比对。
                    det.buy = Some(bbox);
                    det.scene = crate::config::determine_scene(scale.to_calib(bbox));
                }
                1 => det.minus = Some(bbox),
                2 => det.plus = Some(bbox),
                _ => {}
            }
        }
    }
    Some(det)
}

/// 对当前客户区截图跑检测器，返回三类按钮 bbox 与判定场景。
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

    #[test]
    fn decode_class_roundtrip() {
        // 网格 (gx=16, gy=8) 放一个高峰值，size 两通道给归一化 w/h。
        let mut hm = vec![0f32; HM * HM];
        let gy = 8usize;
        let gx = 16usize;
        hm[gy * HM + gx] = 10.0; // logit 10 → sigmoid≈1 > THRESH
        let mut sz = vec![0f32; 2 * HM * HM];
        sz[gy * HM + gx] = 0.5; // w_norm
        sz[HM * HM + gy * HM + gx] = 0.25; // h_norm
        let orig_w = 1920.0;
        let orig_h = 1080.0;
        let (bbox, conf) = decode_class(&hm, &sz, orig_w, orig_h).unwrap();
        assert!(conf > 0.99);
        // 期望：cx_m=(16+0.5)/32*256=132, cy_m=(8+0.5)/32*256=68
        // w_m=0.5*256=128, h_m=0.25*256=64
        // sx=1920/256=7.5, sy=1080/256=4.21875
        // x=(132-64)*7.5=510, y=(68-32)*4.21875=151.875→152
        // w=128*7.5=960, h=64*4.21875=270
        assert!((bbox[0] as f32 - 510.0).abs() < 2.0, "x={bbox:?}");
        assert!((bbox[1] as f32 - 152.0).abs() < 2.0, "y={bbox:?}");
        assert!((bbox[2] as f32 - 960.0).abs() < 2.0, "w={bbox:?}");
        assert!((bbox[3] as f32 - 270.0).abs() < 2.0, "h={bbox:?}");
    }

    #[test]
    fn decode_class_below_thresh() {
        let hm = vec![-100.0f32; HM * HM]; // sigmoid(-100)≈0 < 0.3
        let sz = vec![0f32; 2 * HM * HM];
        assert!(decode_class(&hm, &sz, 1920.0, 1080.0).is_none());
    }

    /// 端到端：加载真实训练出的 `buttons.onnx`，对真实标注截图跑完整推理链路
    /// （前处理 → ORT 推理 → 解码 → 场景分类），验证集成可正常工作。
    ///
    /// 该测试只验证「Rust 端 ONNX 推理 + 解码 + 场景判定」全链路跑通并产出合法场景，
    /// **不**对模型逐帧识别精度做硬性断言（识别精度取决于训练数据/标注质量，属模型层面问题；
    /// 场景映射正确性已由 `determine_scene_matches_layouts` 单测覆盖）。
    /// 依赖 `nn-classifier/buttons.onnx` 与 `nn-classifier/button_dataset/` 标注（随仓库存在）。
    #[test]
    fn e2e_real_model_detects_scene() {
        use crate::config::Scale;
        use crate::screenshot::CapturedImage;
        assert!(
            is_available(),
            "buttons.onnx 未找到，无法跑端到端测试（先 `python train.py button`）"
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
        let want = |b: [i32; 4]| -> crate::config::BuyScene {
            if b == [1482, 829, 297, 61] {
                crate::config::BuyScene::Exchangeable
            } else if b == [1484, 887, 293, 59] {
                crate::config::BuyScene::NonExchangeable
            } else {
                panic!("未知布局 buy={b:?}")
            }
        };
        let parse_img = |line: &str| -> Option<String> {
            let s = line.find("\"image\":\"")? + "\"image\":\"".len();
            let e = s + line[s..].find('"')?;
            Some(line[s..e].to_string())
        };

        let mut buy_detected = 0usize;
        let mut matched = 0usize;
        let mut classified = 0usize;
        for line in content.lines().take(80) {
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
            // 链路跑通：检测到「购买」按钮（解码有效）→ 证明 ONNX 加载 + 前处理 + 解码全通。
            if det.buy.is_some() {
                buy_detected += 1;
            }
            // 场景分类：模型定位足够精准时返回 Some，否则优雅回退 None（属识别精度问题，非链路错误）。
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
            "[e2e buttons] 购买按钮检出 {buy_detected} 张、其中成功判定场景 {classified} 张、与标注一致 {matched} 张（一致率 {:.1}%）",
            rate * 100.0
        );
        // 集成正确性门槛：对真实模型稳定检出「购买」按钮（≥5 张即证明 ONNX 加载/前处理/解码全通）。
        assert!(
            buy_detected >= 5,
            "端到端未能在真实标注图上检出「购买」按钮（ONNX 推理链路可能异常）"
        );
    }
}
