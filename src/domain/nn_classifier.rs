//! 神经网络数字分类器（ONNX Runtime / `ort`）。
//!
//! 用 `nn-classifier/model.onnx`（由 `train.py` `--export-onnx` 导出，
//! 输入 `input[1,1,32,32] f32`、输出 `logits[1,11]`）对每个数位裁剪做 11 类（0-9 + x）分类。
//!
//! ## 前处理（必须与 `train.py` 的 `TRANSFORM` 完全一致）
//! RGBA → 灰度（ITU-R 601，与 PIL `Grayscale` 一致）→ 双线性缩放 32×32 → `[0,1]` → `(x-0.5)/0.5`。
//!
//! ## 模型缺失时的行为
//! 若 `model.onnx` 不存在，`classify` 返回 `None`，对应数位判为「未识别」。
//! 程序已不再保留模板匹配兜底——分发时务必随附 `model.onnx`。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use image::imageops::FilterType;
use image::GrayImage;
use ort::session::Session;
use ort::value::Tensor;

use crate::config::Rect;
use crate::screenshot;

/// ONNX 模型文件名（与 `train.py --export-onnx` 产物同名）。
const MODEL_NAME: &str = "model.onnx";
/// 输入边长（正方形，像素），须与 `train.py` 的 `INPUT_SIZE` 一致。
const INPUT_SIZE: usize = 32;
/// 模型文件完整路径环境变量（优先级最高）。
const MODEL_ENV: &str = "DELTA_OBSERVER_MODEL";
/// 模型所在目录环境变量（其次），在该目录下寻找 `model.onnx`。
const MODEL_DIR_ENV: &str = "DELTA_OBSERVER_MODEL_DIR";
/// 灰度权重（ITU-R 601），与 PIL `Grayscale` 一致。
const W_R: f32 = 0.299;
const W_G: f32 = 0.587;
const W_B: f32 = 0.114;
/// 置信度阈值（沿用原 90% 硬规则语义），作用于 softmax 概率；
/// `x` 类（非数字）恒判为「无数字」，不计入此阈值。
const CONF_THRESHOLD: f32 = 0.90;

/// 已加载的 ONNX 会话（进程内仅加载一次；模型缺失则为 `None`）。
/// 用 `Mutex` 包裹以满足 `Session::run` 对 `&mut self` 的要求（观察在后台线程并发调用）。
static SESSION: OnceLock<Option<Mutex<Session>>> = OnceLock::new();

/// 单次推理输出：识别字符（已按阈值判定）+ 11 类 softmax 概率（顺序固定为 0..9 然后 x）。
pub struct NnOut {
    pub recognized: Option<char>,
    pub probs: Vec<f32>,
}

/// 定位 `model.onnx`：环境变量 → exe 向上追溯 `nn-classifier/`、`assets/` → exe 同目录。
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

/// 惰性加载模型（仅首次调用触发；模型缺失则永久记 `None`，之后该数位一直判为未识别，无兜底）。
fn load_once() {
    SESSION.get_or_init(|| {
        let model_path = locate_model()?;
        let session = Session::builder().ok()?.commit_from_file(&model_path).ok()?;
        Some(Mutex::new(session))
    });
}

/// 模型是否可用（已成功加载 `model.onnx`）。首次调用触发加载。
pub fn is_available() -> bool {
    load_once();
    SESSION.get().map(|o| o.is_some()).unwrap_or(false)
}

/// 对单个 RGBA 数位裁剪做与 `train.py` 完全一致的预处理，输出 1024 个归一化 f32。
fn preprocess(rgba: &[u8], w: i32, h: i32) -> Option<Vec<f32>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let n = (w as usize) * (h as usize);
    if rgba.len() < n * 4 {
        return None;
    }
    // 灰度（ITU-R 601），丢弃 Alpha，与 PIL Grayscale 一致。
    let mut gray = Vec::with_capacity(n);
    for chunk in rgba[..n * 4].chunks_exact(4) {
        let l = (W_R * chunk[0] as f32 + W_G * chunk[1] as f32 + W_B * chunk[2] as f32)
            .round()
            .clamp(0.0, 255.0) as u8;
        gray.push(l);
    }
    let gray_img = GrayImage::from_raw(w as u32, h as u32, gray)?;
    // 双线性缩放至 32×32（与 torchvision Resize 默认一致）。
    let resized =
        image::imageops::resize(&gray_img, INPUT_SIZE as u32, INPUT_SIZE as u32, FilterType::Triangle);
    let mut out = Vec::with_capacity(INPUT_SIZE * INPUT_SIZE);
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

/// 对单个数位矩形做神经网络分类。
///
/// 返回 `None` 表示模型未加载 / 裁剪无效；否则给出识别字符（已按 90% 阈值 + `x` 非数字规则判定）
/// 与 11 类概率（供 OCR 测试 Tab 可视化）。裁剪矩形建议先用 `digit::inset_digit_rect` 内缩，
/// 以与训练时保存的数字裁剪同构。
///
/// 类别顺序固定为 `0,1,...,9,x`（见 `train.py` 的 `CLASSES`）；索引 10 即 `x` = 非数字。
pub fn classify(rgba: &[u8], w: i32, h: i32, rect: Rect) -> Option<NnOut> {
    load_once();
    let mut guard = SESSION.get().and_then(|o| o.as_ref())?.lock().ok()?;

    let (img, iw, ih) = screenshot::crop_region(rgba, w, h, rect)?;
    let data = preprocess(&img, iw, ih)?;
    let tensor = Tensor::<f32>::from_array(([1usize, 1, INPUT_SIZE, INPUT_SIZE], data)).ok()?;
    let outputs = guard.run(ort::inputs!["input" => &tensor]).ok()?;
    let (_shape, view) = outputs["logits"].try_extract_tensor::<f32>().ok()?;
    let probs = softmax(view);
    if probs.len() != 11 {
        return None;
    }

    // 整体最高类（含 x）。
    let mut top = 0usize;
    let mut top_score = probs[0];
    for i in 1..probs.len() {
        if probs[i] > top_score {
            top_score = probs[i];
            top = i;
        }
    }
    let recognized = if top == 10 {
        None // x 类 = 非数字
    } else if top_score >= CONF_THRESHOLD {
        Some((b'0' + top as u8) as char)
    } else {
        None
    };

    Some(NnOut { recognized, probs })
}
