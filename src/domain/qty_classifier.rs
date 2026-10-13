//! 子弹「所选数量」整体识别（ONNX Runtime / `ort`）。
//!
//! 用 `nn-classifier/qty.onnx`（由 `train_qty.py train --export-onnx` 导出，
//! 输入 `input[1,1,32,140] f32`、输出 `logits[1,200]`）对**整块数量显示区域**做
//! 整体分类，直接输出整数值 1..200（模型输出索引 `i` 对应数值 `i + 1`，不含 0）。
//!
//! 与 `nn_classifier`（逐位数字分类 0-9+x）不同：数量在画面中居中显示，不同位数对应
//! 不同坐标位置，逐位裁剪极易错位；因此改为对**整块区域**一次性分类，规避逐位对齐问题。
//!
//! ## 前处理（必须与 `train_qty.py` 完全一致）
//! RGBA → 灰度（ITU-R 601，与 PIL `Grayscale` 一致）
//! → **纵向重心对齐 + 线性对比拉伸**（[`align_and_stretch`]，复刻 `train_qty.py::align_and_stretch`）
//! → 双线性缩放 32×140 → `[0,1]` → `(x-0.5)/0.5`。
//!
//! 对齐+拉伸的必要性（2026-10-05 实测定位）：可兑换/不可兑换两场景的数量行在全局坐标
//! 相差约 58px，各自区域裁剪后数字在 140×32 内的局部行仍相差 2px；细笔画数字错位 2px
//! 即令类内差超过类间差，模型完全无法收敛。对齐后类内差 0.63 / 类间差 18.4，1-NN 97.5%。
//!
//! ## 模型缺失时的行为
//! 若 `qty.onnx` 不存在，`classify` 返回 `None`，调用方据此判定「数量未识别」（保守降级）。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use image::imageops::FilterType;
use image::GrayImage;
use ort::session::Session;
use ort::value::Tensor;

use crate::config::Rect;
use crate::screenshot;

/// ONNX 模型文件名（与 `train_qty.py --export-onnx` 产物同名）。
const MODEL_NAME: &str = "qty.onnx";
/// 输入高（像素），须与 `train_qty.py` 的 `QTY_INPUT_H` 一致。
const INPUT_H: usize = 32;
/// 输入宽（像素），须与 `train_qty.py` 的 `QTY_INPUT_W` 一致。
const INPUT_W: usize = 140;
/// 分类种别数（1..200，共 200 类；模型索引 0 对应数值 1），须与 `train_qty.py` 的 `QTY_NUM_CLASSES` 一致。
/// 同时作为「购买目标数量」的有效上限（UI 与 `observe` 均据此钳制）。
pub const NUM_CLASSES: usize = 200;
/// 模型文件完整路径环境变量（优先级最高）。
const MODEL_ENV: &str = "DELTA_OBSERVER_QTY_MODEL";
/// 模型所在目录环境变量（其次），在该目录下寻找 `qty.onnx`。
const MODEL_DIR_ENV: &str = "DELTA_OBSERVER_QTY_MODEL_DIR";
/// 灰度权重（ITU-R 601），与 PIL `Grayscale` 一致。
const W_R: f32 = 0.299;
const W_G: f32 = 0.587;
const W_B: f32 = 0.114;
/// 置信度阈值：softmax 最高类概率须 ≥ 此值才采信，否则判为未识别。
const CONF_THRESHOLD: f32 = 0.5;
/// 数字笔画判定阈值，须与 `train_qty.py` 的 `QTY_STROKE_THRESH` 一致。
const STROKE_THRESH: f32 = 110.0;
/// 线性拉伸端点（背景≈37→0，笔画峰值≈152→255），须与 `QTY_STRETCH_LOW/HIGH` 一致。
const STRETCH_LOW: f32 = 37.0;
const STRETCH_HIGH: f32 = 152.0;
/// 对齐目标行（数字亮像素重心平移到该行），须与 `QTY_ALIGN_ROW` 一致。
const ALIGN_ROW: f32 = 16.0;

/// 已加载的 ONNX 会话（进程内仅加载一次；模型缺失则为 `None`）。
static SESSION: OnceLock<Option<Mutex<Session>>> = OnceLock::new();

/// 定位 `qty.onnx`：环境变量 → exe 向上追溯 `nn-classifier/`、`assets/` → exe 同目录。
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

/// 惰性加载模型（仅首次调用触发；模型缺失则永久记 `None`，之后该次识别一直判为未识别）。
fn load_once() {
    SESSION.get_or_init(|| {
        let model_path = locate_model()?;
        let session = Session::builder().ok()?.commit_from_file(&model_path).ok()?;
        Some(Mutex::new(session))
    });
}

/// 模型是否可用（已成功加载 `qty.onnx`）。首次调用触发加载。
pub fn is_available() -> bool {
    load_once();
    SESSION.get().map(|o| o.is_some()).unwrap_or(false)
}

/// 银行家舍入（half to even），与 Python `round()` 一致，保证与训练端平移量完全相同。
fn round_half_even(v: f32) -> i32 {
    let f = v.floor();
    let diff = v - f;
    if diff > 0.5 {
        f as i32 + 1
    } else if diff < 0.5 {
        f as i32
    } else if (f as i32) % 2 == 0 {
        f as i32
    } else {
        f as i32 + 1
    }
}

/// 纵向重心对齐 + 线性对比拉伸（逐步复刻 `train_qty.py::align_and_stretch`）。
///
/// - 对齐：亮像素(>= `STROKE_THRESH`)行重心为锚，整幅图纵向平移 `round(ALIGN_ROW - 重心)`
///   像素，越界行补背景值，消除两场景数字 2px 的纵向错位；
/// - 拉伸：`clip((x - LOW) * 255 / (HIGH - LOW), 0, 255)`，拉开细数字的低对比信号。
fn align_and_stretch(gray: &mut Vec<u8>, w: usize, h: usize) {
    let mut sum_y = 0.0f32;
    let mut cnt = 0usize;
    for (i, &p) in gray.iter().enumerate() {
        if p as f32 >= STROKE_THRESH {
            sum_y += (i / w) as f32;
            cnt += 1;
        }
    }
    if cnt > 0 {
        let shift = round_half_even(ALIGN_ROW - sum_y / cnt as f32);
        if shift != 0 {
            let mut filled = vec![STRETCH_LOW as u8; w * h];
            for y in 0..h as i32 {
                let sy = y - shift;
                if sy >= 0 && sy < h as i32 {
                    let (dst, src) = (y as usize * w, sy as usize * w);
                    filled[dst..dst + w].copy_from_slice(&gray[src..src + w]);
                }
            }
            *gray = filled;
        }
    }
    let scale = 255.0 / (STRETCH_HIGH - STRETCH_LOW);
    for p in gray.iter_mut() {
        let v = (*p as f32 - STRETCH_LOW) * scale;
        // 与 Python `astype(np.uint8)` 一致：截断而非四舍五入。
        *p = v.clamp(0.0, 255.0) as u8;
    }
}

/// 对整块数量区域裁剪做与 `train_qty.py` 完全一致的预处理，输出 `INPUT_H*INPUT_W` 个归一化 f32。
fn preprocess(rgba: &[u8], w: i32, h: i32, rect: Rect) -> Option<Vec<f32>> {
    let (img, iw, ih) = screenshot::crop_region(rgba, w, h, rect)?;
    if iw <= 0 || ih <= 0 {
        return None;
    }
    let n = (iw as usize) * (ih as usize);
    if img.len() < n * 4 {
        return None;
    }
    // 灰度（ITU-R 601），丢弃 Alpha，与 PIL Grayscale 一致。
    let mut gray = Vec::with_capacity(n);
    for chunk in img[..n * 4].chunks_exact(4) {
        let l = (W_R * chunk[0] as f32 + W_G * chunk[1] as f32 + W_B * chunk[2] as f32)
            .round()
            .clamp(0.0, 255.0) as u8;
        gray.push(l);
    }
    // 纵向对齐 + 对比拉伸（与训练端 train_qty.py 一致；须在缩放前做）。
    align_and_stretch(&mut gray, iw as usize, ih as usize);
    let gray_img = GrayImage::from_raw(iw as u32, ih as u32, gray)?;
    // 双线性缩放至 32×140（与 torchvision Resize 默认一致）。
    let resized =
        image::imageops::resize(&gray_img, INPUT_W as u32, INPUT_H as u32, FilterType::Triangle);
    let mut out = Vec::with_capacity(INPUT_H * INPUT_W);
    for p in resized.into_raw() {
        out.push((p as f32 / 255.0 - 0.5) / 0.5);
    }
    Some(out)
}

/// softmax（数值稳定：减最大值）。
fn softmax(x: &[f32]) -> Vec<f32> {
    if x.is_empty() {
        return Vec::new();
    }
    let max = x.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = x.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|v| v / sum).collect()
}

/// 对整块数量区域矩形做神经网络分类，返回识别到的整数值（1..200）。
///
/// 返回 `None` 表示模型未加载 / 裁剪无效 / 置信度不足（视为未识别）。裁剪矩形建议为
/// 场景对应的整块数量区域（见 [`crate::config::BuyScene::qty_region`]），经 `Scale::rect`
/// 映射到实际客户区。
///
/// 类别索引 `i` 对应数量 `i + 1`（不含 0，分类区间为 1..200，见 `train_qty.py`）。
pub fn classify(rgba: &[u8], w: i32, h: i32, rect: Rect) -> Option<u32> {
    load_once();
    let mut guard = SESSION.get().and_then(|o| o.as_ref())?.lock().ok()?;

    let data = preprocess(rgba, w, h, rect)?;
    let tensor = Tensor::<f32>::from_array(([1usize, 1, INPUT_H, INPUT_W], data)).ok()?;
    let outputs = guard.run(ort::inputs!["input" => &tensor]).ok()?;
    let (_shape, view) = outputs["logits"].try_extract_tensor::<f32>().ok()?;
    let probs = softmax(view);
    if probs.len() != NUM_CLASSES {
        return None;
    }

    // 整体最高类（索引即数值）。
    let mut top = 0usize;
    let mut top_score = probs[0];
    for i in 1..probs.len() {
        if probs[i] > top_score {
            top_score = probs[i];
            top = i;
        }
    }
    if top_score >= CONF_THRESHOLD {
        // 类别索引 `i` 对应数值 `i + 1`（分类区间 1..200，不含 0）。
        Some(top as u32 + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_half_even_banker() {
        // 银行家舍入（half to even），与 Python round() 一致。
        assert_eq!(round_half_even(2.5), 2); // 2 为偶数
        assert_eq!(round_half_even(3.5), 4); // 4 为偶数
        assert_eq!(round_half_even(0.5), 0);
        assert_eq!(round_half_even(1.5), 2);
        assert_eq!(round_half_even(-0.5), 0);
        assert_eq!(round_half_even(-1.5), -2);
        assert_eq!(round_half_even(2.4), 2);
        assert_eq!(round_half_even(2.6), 3);
    }

    #[test]
    fn align_shifts_centroid_to_align_row() {
        // 造一张 5×32 灰度图：仅第 5 行（centroid=5）为亮笔划(200)，其余为背景(37)。
        let w = 5usize;
        let h = 32usize;
        let mut g = vec![STRETCH_LOW as u8; w * h]; // 背景 ≈ 37
        for x in 0..w {
            g[5 * w + x] = 200; // 第 5 行亮
        }
        align_and_stretch(&mut g, w, h);
        // 重心 5 → 平移量 round(16-5)=11 → 亮行应落在第 16 行（ALIGN_ROW）。
        assert_eq!(g[16 * w], 255, "第16行应被亮笔划填充");
        // 原第 5 行此时应被背景（拉伸后 = 0）回填。
        assert_eq!(g[5 * w], 0, "第5行应变回背景");
    }

    #[test]
    fn align_is_noop_when_already_centered() {
        let w = 5usize;
        let h = 32usize;
        let mut g = vec![STRETCH_LOW as u8; w * h];
        for x in 0..w {
            g[(ALIGN_ROW as usize) * w + x] = 200; // 已居中
        }
        align_and_stretch(&mut g, w, h);
        // 居中时不应平移：第 ALIGN_ROW 行仍为亮（拉伸后 255），其余为背景 0。
        assert_eq!(g[(ALIGN_ROW as usize) * w], 255);
        assert_eq!(g[0], 0);
    }
}
