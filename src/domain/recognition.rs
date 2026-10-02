//! 子弹识别共享模块：把「开始观察」与「OCR 测试」两个 Tab 共用的
//! **子弹名称 + 价格识别逻辑**收敛到一处，避免重复实现。
//!
//! 复用既有能力（均已在各自模块内做到即开即用、构造一次长期复用）：
//! - 名称：[`crate::ocr::NameOcr`]（Windows OCR，引擎按语言缓存复用）
//! - 价格：[`crate::digit::PriceRecognizer`]（千/百/十/个 逐位神经网络识别 + 合法性校验 + 非法重试）
//!
//! 本模块只做**纯识别**（给定一帧 + 该子弹的标定坐标，返回结构化结果），
//! 不触碰任何文件 I/O / 网络 / UI，因此可同时被后台观察线程与 UI 测试 Tab 安全调用。

use crate::config::{BulletRects, Config, Scale};
use crate::digit::{PriceRec, PriceRecognizer};
use crate::ocr::NameOcr;
use crate::screenshot::CapturedImage;

/// 单个数位的识别可视化详情（供 OCR 测试 Tab 展示）。
pub use crate::digit::DigitDetail;

/// 单枚子弹的识别结果。
pub struct BulletRec {
    /// 名称区 OCR 文本（按阅读顺序拼接）。
    pub name: String,
    /// 价格识别结果（枚举：合法价格 / 全空 / 非法断层）。
    pub price: PriceRec,
}

/// 子弹识别器：构造一次，长期复用（内部持有 OCR 引擎与价格识别器）。
///
/// 即开即用：观测开始时或测试 Tab 点击识别时构造一次即可，
/// 对每枚子弹调用 [`recognize_bullet`](Self::recognize_bullet)。
pub struct BulletRecognition {
    name_ocr: NameOcr,
    price_rec: PriceRecognizer,
}

impl BulletRecognition {
    pub fn new() -> Self {
        Self {
            name_ocr: NameOcr::new(),
            price_rec: PriceRecognizer::new(),
        }
    }

    /// 识别单枚子弹：名称走 OCR，价格走逐位神经网络识别，并额外返回每位数位的识别详情
    /// （裁剪图 + 各类概率），供 OCR 测试 Tab 直观展示与「开始观察」数字裁剪落盘使用。
    ///
    /// `rects` 为 `config` 中的**标定坐标**（非实际帧）；缩放由本方法依据
    /// `frame` 实际宽度推导，调用方无需预缩放。
    pub fn recognize_bullet_detail(
        &mut self,
        frame: &CapturedImage,
        rects: &BulletRects,
    ) -> (BulletRec, Vec<Option<DigitDetail>>) {
        let (w, h) = frame.dimensions();
        let scale = Scale::for_client_width(w);
        let name = self.name_ocr.read(&frame.rgba, w, h, scale.rect(rects.name));
        let digits = rects.digit_rects().map(|r| scale.rect(r));
        let (price, details) = self.price_rec.recognize_detail(&frame.rgba, w, h, digits);
        (BulletRec { name, price }, details)
    }
}

/// 同 [`recognize_caliber_detail`] 的命名语义，逐个子弹识别并额外附带其价格各数位的
/// 识别可视化详情（裁剪图 + 各类概率），供 OCR 测试 Tab 的「价格浏览」滑块逐位展示。
pub fn recognize_caliber_detail(
    frame: &CapturedImage,
    caliber: &str,
    cfg: &Config,
) -> Vec<(String, BulletRects, BulletRec, Vec<Option<DigitDetail>>)> {
    let mut rec = BulletRecognition::new();
    match cfg.get(caliber) {
        Some(bullets) => bullets
            .iter()
            .map(|(name, rects)| {
                let (b, d) = rec.recognize_bullet_detail(frame, rects);
                (name.clone(), *rects, b, d)
            })
            .collect(),
        None => Vec::new(),
    }
}
