//! 交易行截图坐标配置（代码化，带逻辑）。
//!
//! 设计要点：
//! - 交易行界面共用同一套 3×3 网格，共 9 个子弹槽位；
//! - `GAUGE12` 是**全部 9 个可能槽位的位置集合**（并非专属于 12 Gauge 口径），
//!   每个槽位持有 `name`（名称区域）与价格 4 个数位区域
//!   （`ones`/`tens`/`hundreds`/`thousands`，见 `BulletRects`）；
//! - 每个口径实际使用 `GAUGE12` 的**前 N 个槽位**（N = 该口径子弹的实际数量，
//!   按 `build_config` 中该口径子弹列表的书写顺序一一对应，见 `insert_caliber`）；
//!   12 Gauge 恰好有 9 枚子弹、覆盖全部槽位，故以其为完整参照。
//!   统一修改坐标只需改 `GAUGE12` 一处。
//!
//! 坐标契约（唯一真相）：`[x, y, w, h]` 以「客户区 / 无标题栏截图」左上角为原点，
//! 与默认全屏截图及 `screenshot::capture_client_rgba` 缓冲原点一致，调用方无需任何偏移换算。
//! 数值本身量自标定视口 `CALIB_VIEWPORT_W × CALIB_VIEWPORT_H`（见该常量说明），
//! 运行时经 `Scale` 映射到实际客户区。

use std::collections::HashMap;

/// 一个矩形区域 `[x, y, w, h]`：相对客户区（无标题栏截图）左上角。
pub type Rect = [i32; 4];

/// 目标游戏窗口标题关键字（用于查找 / 聚焦游戏窗口，标题包含匹配）。
pub const GAME_KEYWORD: &str = "三角洲行动";

/// **坐标标定基准视口**（宽 × 高，像素）：本文件所有坐标常量都直接量自该尺寸的游戏客户区截图，
/// 即**写进代码的就是截图上的像素值，不需要任何人工换算**。
///
/// ## 依据
/// 标定素材是实际游戏客户区截图（早期经观察流程落盘核验），实测尺寸 1920×1080。
///
/// ## 为什么不选一个「更规整」的参考值（如 1080）
/// 那等于凭空引入一把与真实素材无关的虚拟标尺，迫使每组坐标在「写入时除、运行时乘」之间往返：
/// 既因两次取整损失精度（10px 的数位框会被还原成 11px），又极易把某组坐标误写成另一帧的原值
/// 而被二次放大 —— 本项目已因此出现过两次「标注整体错位、框飞出画面」。
/// 基准与素材同帧后，当前环境下运行时因子恒为 `1.0`，所见即所得。
pub const CALIB_VIEWPORT_W: i32 = 1920;

/// 标定基准视口高度，配套 [`CALIB_VIEWPORT_W`]。
///
/// 仅用于**文档与人工核对**（确认标定素材与实际画面宽高比一致）；缩放推导只用宽度，
/// 原因见 [`Scale::for_client_width`]。刻意保留为具名常量而非注释里的数字，
/// 以便它与宽度成对出现、随时可查。
#[allow(dead_code)]
pub const CALIB_VIEWPORT_H: i32 = 1080;

/// 标定视口 → 实际客户区的**等比缩放因子**。
///
/// 由实际客户区尺寸动态推导（见 [`Scale::for_client_width`]），代码中**不存在任何硬编码系数**：
/// 实际客户区等于标定视口时因子为 `1.0`（坐标原样使用），分辨率 / DPI 变化时自动按比例缩放。
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    factor: f32,
}

impl Scale {
    /// 按实际客户区**宽度**推导缩放因子：`client_w / CALIB_VIEWPORT_W`。
    ///
    /// ## 为何只按宽度、不按高度
    /// 客户区高度会随窗口是否带标题栏、是否全屏而变化（同为 1920 宽时可能是 1080 或 1040）；
    /// 若把高度并入因子，一个标题栏就会让整张布局被无谓压缩。而交易行 UI 是横向铺满、
    /// 纵向可裁的，故以宽度为唯一推导基准。
    ///
    /// 宽度非法（≤ 0，如窗口最小化）时退化为中性因子 `1.0`，保证仍可按标定视口取值。
    pub fn for_client_width(client_w: i32) -> Self {
        let factor = client_w as f32 / CALIB_VIEWPORT_W as f32;
        Self {
            factor: if factor.is_finite() && factor > 0.0 {
                factor
            } else {
                1.0
            },
        }
    }

    /// 把标定视口下的矩形映射到实际客户区。
    ///
    /// 宽高下限钳到 1px：避免因子极小时退化成 0，使下游 `crop_region` 裁出空图。
    pub fn rect(&self, r: Rect) -> Rect {
        [
            (r[0] as f32 * self.factor).round() as i32,
            (r[1] as f32 * self.factor).round() as i32,
            ((r[2] as f32 * self.factor).round() as i32).max(1),
            ((r[3] as f32 * self.factor).round() as i32).max(1),
        ]
    }

    /// 把实际客户区矩形映射回**标定视口坐标**（与 [`Scale::rect`] 互逆）。
    ///
    /// 因子为 `1/factor`；标注坐标（如 [`BUY_LAYOUT_1`]）均定义在标定视口空间，
    /// 模型检测出的 bbox 经此方法还原到同一空间后，方可与模板做距离/误差比较。
    pub fn to_calib(&self, r: Rect) -> Rect {
        let inv = if self.factor > 0.0 { 1.0 / self.factor } else { 1.0 };
        [
            (r[0] as f32 * inv).round() as i32,
            (r[1] as f32 * inv).round() as i32,
            ((r[2] as f32 * inv).round() as i32).max(1),
            ((r[3] as f32 * inv).round() as i32).max(1),
        ]
    }
}

/// 购买界面按钮布局（标定视口坐标 `[x, y, w, h]`，与 [`GAUGE12`] 同一坐标契约）。
///
/// 交易行购买界面存在两种固定布局，由「购买」按钮的落点区分；检测器定位到
/// 购买按钮后据此判定当前界面属于哪一种场景（见 [`BuyScene`] / [`determine_scene`]）。
#[derive(Clone, Copy)]
pub struct BuyBtnLayout {
    pub buy: Rect,
    pub minus: Rect,
    pub plus: Rect,
}

/// 购买阶段「所选子弹数量」的**整块数字区域**坐标（标定视口 `[x, y, w, h]`，
/// 与 [`GAUGE12`] 同一坐标契约）。
///
/// 与购买界面价格数位（`BuyDigitRects`，y=174）**互不相关**——这是购买界面上独立的
/// 「数量显示框」UI。数字在画面中**居中显示**，不同位数对应不同坐标位置，逐位裁剪极易
/// 错位，故识别改为对**整块区域**一次性分类（见 `qty_classifier`），此处仅给出区域包围盒。
///
/// - 场景1 · 可兑换子弹的场景： (1560, 722, 140, 32)
/// - 场景2 · 不可兑换子弹的场景：(1560, 782, 140, 32)
pub const BUY_QTY_REGION_1: Rect = [1560, 722, 140, 32];
/// 场景2 · 不可兑换子弹的场景：所选子弹数量整块区域位于下方一排（y=782）。
pub const BUY_QTY_REGION_2: Rect = [1560, 782, 140, 32];

/// 场景1 · 可兑换子弹的场景：购买按钮位于上方一排（y≈829）。
pub const BUY_LAYOUT_1: BuyBtnLayout = BuyBtnLayout {
    buy: [1482, 829, 297, 61],
    minus: [1461, 754, 38, 42],
    plus: [1764, 754, 38, 42],
};

/// 场景2 · 不可兑换子弹的场景：购买按钮位于下方一排（y≈887）。
pub const BUY_LAYOUT_2: BuyBtnLayout = BuyBtnLayout {
    buy: [1484, 887, 293, 59],
    minus: [1461, 815, 38, 42],
    plus: [1764, 815, 38, 42],
};

/// 购买界面「数量滑块」锚点 x（标定视口坐标 `[x, y, w, h]`，与 [`GAUGE12`] 同一坐标契约）。
///
/// 滑块横向铺设于「减 / 加」按钮之间；点击滑块上不同 x 即可快速定位到不同数量。
/// `y` 不在此固定，而由场景动态取「减 / 加」按钮所在行的中心（二按钮 y 相同），
/// 见 `buy::slider_click_rect`。锚点 x = 1555 即「已知数量 31」所在的大致位置。
pub const SLIDER_ANCHOR_X: i32 = 1555;

/// 数量滑块参考点表：`(数量, 滑块点击相对锚点的横向偏移 dx)`。
///
/// 当前仅提供 `(31, 0)`（偏移 0，即锚点本身）一个已知点。后续可在此**直接追加**更多已知点
/// （如 `(10, -200)`、`(100, 300)` 等），数量调整时自动选取离目标最近的已知点先快速跳转，
/// 再以此为基准用「减 / 加」按钮微调至目标值，避免从 1 起逐点累加的大幅连点。
/// 该表只描述「已知数量 → 滑块点击位置」的映射，扩展后无需改动调用逻辑
/// （见 `buy::nearest_ref_point` 与 `buy::adjust_buy_qty`）。
pub const QTY_SLIDER_REFS: &[(u32, i32)] = &[(31, 0)];

/// 购买界面场景：由检测器定位到的「购买」按钮 bbox 据两套固定布局判定。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum BuyScene {
    /// 场景1：可兑换子弹（购买按钮位于上方一排）。
    Exchangeable,
    /// 场景2：不可兑换子弹（购买按钮位于下方一排）。
    NonExchangeable,
}

impl BuyScene {
    /// 全部场景变体（核查「购买后判断」接入时遍历用）。
    pub fn all() -> &'static [BuyScene] {
        &[BuyScene::Exchangeable, BuyScene::NonExchangeable]
    }

    /// 中文场景标签（毛玻璃面板 / 日志展示用）。
    pub fn label(&self) -> &'static str {
        match self {
            BuyScene::Exchangeable => "可兑换子弹的场景",
            BuyScene::NonExchangeable => "不可兑换子弹的场景",
        }
    }

    /// 该场景对应的按钮布局模板（标定视口坐标）。
    pub fn layout(&self) -> &'static BuyBtnLayout {
        match self {
            BuyScene::Exchangeable => &BUY_LAYOUT_1,
            BuyScene::NonExchangeable => &BUY_LAYOUT_2,
        }
    }

    /// 该场景对应的「所选子弹数量」**整块区域**坐标（标定视口坐标）。
    ///
    /// 识别时对这块区域做整体分类（见 `qty_classifier::classify`），不再逐位裁剪。
    pub fn qty_region(&self) -> Rect {
        match self {
            BuyScene::Exchangeable => BUY_QTY_REGION_1,
            BuyScene::NonExchangeable => BUY_QTY_REGION_2,
        }
    }
}

/// 按「购买」按钮 bbox（标定视口坐标）判定场景；无法匹配（含 `None` 语义的空框）则返回 `None`。
///
/// 复刻 `nn-classifier/train.py` 的 `determine_case`：比较中心点距离与宽高相对误差，
/// 二者均足够接近某一布局的「购买」坐标时判定成功（`dist < 40 && sr < 0.15`）。
pub fn determine_scene(buy_bbox: Rect) -> Option<BuyScene> {
    let (x, y, w, h) = (buy_bbox[0] as f32, buy_bbox[1] as f32, buy_bbox[2] as f32, buy_bbox[3] as f32);
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let candidates: [(BuyScene, Rect); 2] = [
        (BuyScene::Exchangeable, BUY_LAYOUT_1.buy),
        (BuyScene::NonExchangeable, BUY_LAYOUT_2.buy),
    ];
    let mut best = BuyScene::Exchangeable;
    let mut best_score = f32::NEG_INFINITY;
    for (scene, bb) in candidates {
        let (bx, by, bw, bh) = (bb[0] as f32, bb[1] as f32, bb[2] as f32, bb[3] as f32);
        let bcx = bx + bw / 2.0;
        let bcy = by + bh / 2.0;
        let dist = ((cx - bcx).powi(2) + (cy - bcy).powi(2)).sqrt();
        let sr = ((w - bw).abs() / bw + (h - bh).abs() / bh) / 2.0;
        let score = -dist / 30.0 - sr * 5.0;
        if score > best_score {
            best_score = score;
            best = scene;
        }
    }
    // 用最优候选重算阈值判定（与 Python 一致：中心距 < 40 且宽高相对误差 < 0.15）。
    let [bx, by, bw, bh] = match best {
        BuyScene::Exchangeable => BUY_LAYOUT_1.buy,
        BuyScene::NonExchangeable => BUY_LAYOUT_2.buy,
    };
    let bcx = bx as f32 + bw as f32 / 2.0;
    let bcy = by as f32 + bh as f32 / 2.0;
    let dist = ((cx - bcx).powi(2) + (cy - bcy).powi(2)).sqrt();
    let sr = ((w - bw as f32).abs() / bw as f32 + (h - bh as f32).abs() / bh as f32) / 2.0;
    if dist < 40.0 && sr < 0.15 {
        Some(best)
    } else {
        None
    }
}

/// 商品检索栏（口径过滤输入框）矩形 `[x, y, w, h]`，标定基准同 [`CALIB_VIEWPORT_W`]
/// （实测截图中含「.357 Magnum」文本的输入框，点击其中心即聚焦）。
pub const SEARCH_BAR: Rect = [78, 151, 313, 50];

/// 单枚子弹的区域坐标：名称区域 + 价格的 **4 个数位独立区域**。
///
/// 价格不再用一个整框，而是拆成 个 / 十 / 百 / 千 四个独立坐标区域
/// （`ones` / `tens` / `hundreds` / `thousands`），每个区域只含一位数字。
/// 识别时对每个区域单独截图 → 神经网络逐位分类 → 按 **千→百→十→个** 顺序拼成最终价格
/// （见 `digit::recognize_price`）。这种方式从根本上规避整框 OCR 把彩色价格整片吞掉的问题。
/// 坐标均为标定视口（[`CALIB_VIEWPORT_W`]）下的原始像素，运行时由 [`Scale::rect`] 映射到实际客户区。
#[derive(Clone, Copy)]
pub struct BulletRects {
    pub name: Rect,
    pub ones: Rect,
    pub tens: Rect,
    pub hundreds: Rect,
    pub thousands: Rect,
    /// 可点击的购买区域（标定视口坐标 `[x, y, w, h]`，运行时经 `Scale::rect` 映射）。
    ///
    /// 购买阶段「打开购买界面」的点击落点：扫描价低于阈值时，导航点击不再基于价格框偏移，
    /// 而是**在该区域范围内随机取一点**点击，避免每次落在同一像素、更接近人类操作、降低被风控概率。
    /// 与 `name` / 价格数位共用同一坐标契约（客户区左上角原点）。
    pub buy_area: Rect,
}

impl BulletRects {
    /// 价格的 4 个数位矩形，按 **千 / 百 / 十 / 个**（从左到右、由高位到低位）顺序返回。
    /// 该顺序即「识别后拼接价格」的顺序。
    pub fn digit_rects(&self) -> [Rect; 4] {
        [self.thousands, self.hundreds, self.tens, self.ones]
    }

    /// 价格整体包围盒：4 个数位矩形的最小外接矩形（用于调试落盘整块价格图）。
    ///
    /// 直接复用 `digit_rects()` 取全部数位，避免二次列举造成顺序不一致；
    /// 取最小外接矩形与列举顺序无关。
    pub fn price_box(&self) -> Rect {
        let rects = self.digit_rects();
        let x0 = rects.iter().map(|r| r[0]).min().unwrap_or(0);
        let y0 = rects.iter().map(|r| r[1]).min().unwrap_or(0);
        let x1 = rects.iter().map(|r| r[0] + r[2]).max().unwrap_or(0);
        let y1 = rects.iter().map(|r| r[1] + r[3]).max().unwrap_or(0);
        [x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)]
    }
}

/// 购买阶段（确认/购买界面）的价格数位坐标。
///
/// 与扫描阶段 [`BulletRects`] 共用同一套 `[x, y, w, h]` 契约（标定视口、运行时经
/// [`Scale::rect`] 映射），但坐标数值取自**购买/确认界面**——该界面与扫描界面布局不同，
/// 价格数位落点并不一致，故单独成表、不可复用扫描坐标（见 `buy` 模块的重验证逻辑）。
///
/// 数位顺序同样为 **千 / 百 / 十 / 个**（高位到低位）；3 位价格时 `thousands` 槽位不读取。
#[derive(Clone, Copy)]
pub struct BuyDigitRects {
    pub thousands: Rect,
    pub hundreds: Rect,
    pub tens: Rect,
    pub ones: Rect,
}

/// 购买界面数位坐标（≤3 位价格）：百 / 十 / 个 三槽，千位槽位占位不读取。
/// 坐标量自购买界面实测帧（y=174，单槽宽 14 高 22，相邻间距 13px，左对齐）。
pub const BUY_DIGITS_3: BuyDigitRects = BuyDigitRects {
    thousands: [0, 0, 0, 0], // ≤3 位不读取千位
    hundreds: [298, 174, 14, 22],
    tens: [311, 174, 14, 22],
    ones: [324, 174, 14, 22],
};

/// 购买界面数位坐标（>3 位价格）：千 / 百 / 十 / 个 四槽（千位与百位间留逗号位）。
/// 坐标量自购买界面实测帧，与 [`BUY_DIGITS_3`] 同帧基准，仅槽位布局因位数不同而异。
pub const BUY_DIGITS_4: BuyDigitRects = BuyDigitRects {
    thousands: [298, 174, 14, 22],
    hundreds: [317, 174, 14, 22],
    tens: [330, 174, 14, 22],
    ones: [343, 174, 14, 22],
};

/// 全部 9 个子弹槽位的位置集合（行优先：第 0/1/2 行各 3 列）。
///
/// 并非专属于 12 Gauge：每个口径按 `build_config` 中子弹列表的书写顺序，
/// 使用本表的**前 N 个槽位**（N = 该口径子弹实际数量）；12 Gauge 恰好填满 9 槽。
/// 统一修改坐标只需改这里。
///
/// 坐标依据：2026-10-01 重新标定。实测「交易行」界面中，**价格数字并不在名称条下方，
/// 而是右对齐于所属子弹格的右下角**（带货币图标，千位数字与百位之间有千分位逗号）。
/// 逐格量自 1920×1080 客户区（= [`CALIB_VIEWPORT_W`] × [`CALIB_VIEWPORT_H`]），
/// 即下表数字与真实截图像素一一对应。
///
/// ⚠ 坐标帧约束（曾导致标注整体错位，务必遵守）：
/// 本表**所有**坐标（含 `name` 与四个数位）必须与标定视口同帧——直接填截图上量到的像素。
///
/// ## 数位坐标的定位规则（右对齐、落于格子右下角）
/// 价格文字**右对齐**：个位数字的**右缘**约在 `cell_x + 455`（cell_x 为所属格的左列起点
/// 432 / 901 / 1369；个位框宽 12 → 右缘 = cell_x + 443 + 12）。四个数位框自右向左紧贴对齐位数字，
/// 各宽 12px、高 20px，相邻（十/百、百/千）间距 ~11px；`thousands` 与 `hundreds` 间多留约 16px
/// 以容纳千分位逗号：
/// - `ones`      : `[cell_x + 443, <digit_y>, 12, 20]`（首列实测 +444）
/// - `tens`      : `[cell_x + 432, <digit_y>, 12, 20]`
/// - `hundreds`  : `[cell_x + 421, <digit_y>, 12, 20]`
/// - `thousands` : `[cell_x + 405, <digit_y>, 12, 20]`（3 位价格时此处为货币图标，识别为空）
///
/// 行起点 `row_y` 取 144 / 308 / 469；数位框 y 取 **278 / 438 / 596**（即各 `row_y` 再下约 130px，
/// 落在格子底部、而非名称下方）。因此本表数位 y 为 278 / 438 / 596，而非早期误标的 183 / 347 / 508。
/// 注：各行相对 `row_y` 的纵向偏移约 127~134px（非严格相等，已按最新实测逐行填表）。
pub const GAUGE12: [BulletRects; 9] = [
    // 第 0 行（row_y = 144）
    BulletRects { name: [432, 144, 391, 39], ones:[876, 278, 12, 20],tens:[865, 278, 12, 20],hundreds:[854, 278, 12, 20],thousands:[837, 278, 12, 20],buy_area:[470,172, 478, 100] },  // 0: 第0行第1列
    BulletRects { name: [901, 144, 391, 39], ones:[1344, 278, 12, 20],tens:[1333, 278, 12, 20],hundreds:[1322, 278, 12, 20],thousands:[1306, 278, 12, 20],buy_area:[936,172, 478, 100] }, // 1: 第0行第2列
    BulletRects { name: [1369, 144, 391, 39], ones:[1812, 278, 12, 20],tens:[1802, 278, 12, 20],hundreds:[1790, 278, 12, 20],thousands:[1774, 278, 12, 20],buy_area:[1400,172, 478, 100]  }, // 2: 第0行第3列
    // 第 1 行（row_y = 308）
    BulletRects { name: [432, 308, 391, 39], ones:[876, 438, 12, 20],tens:[865, 438, 12, 20],hundreds:[854, 438, 12, 20],thousands:[837, 438, 12, 20],buy_area:[470,340, 478, 100]  },  // 3: 第1行第1列
    BulletRects { name: [901, 308, 391, 39], ones:[1344, 438, 12, 20],tens:[1333, 438, 12, 20],hundreds:[1322, 438, 12, 20],thousands:[1306, 438, 12, 20],buy_area:[936,340, 478, 100]  }, // 4: 第1行第2列
    BulletRects { name: [1369, 308, 391, 39], ones:[1812, 438, 12, 20],tens:[1802, 438, 12, 20],hundreds:[1790, 438, 12, 20],thousands:[1774, 438, 12, 20],buy_area:[1400,340, 478, 100]  }, // 5: 第1行第3列
    // 第 2 行（row_y = 469）
    BulletRects { name: [432, 469, 391, 39], ones:[876, 596, 12, 20],tens:[865, 596, 12, 20],hundreds:[854, 596, 12, 20],thousands:[837, 596, 12, 20],buy_area:[470,490, 478, 100] },  // 6: 第2行第1列
    BulletRects { name: [901, 469, 391, 39], ones:[1344, 596, 12, 20],tens:[1333, 596, 12, 20],hundreds:[1322, 596, 12, 20],thousands:[1306, 596, 12, 20],buy_area:[936,490, 478, 100] }, // 7: 第2行第2列
    BulletRects { name: [1369, 469, 391, 39], ones:[1812, 596, 12, 20],tens:[1802, 596, 12, 20],hundreds:[1790, 596, 12, 20],thousands:[1774, 596, 12, 20],buy_area:[1400,490, 478, 100] }, // 8: 第2行第3列
];

/// 配置总表：口径 -> 子弹名 -> 区域坐标。
pub type Config = HashMap<String, HashMap<String, BulletRects>>;

/// 把一个口径写入配置：第 `i` 枚子弹按书写顺序复用 `GAUGE12[i]` 的 name/price。
fn insert_caliber(cfg: &mut Config, caliber: &str, bullets: &[&str]) {
    let mut m = HashMap::new();
    for (i, b) in bullets.iter().enumerate() {
        // 越界保护：某口径子弹数超过 9 时回绕复用（理论上不应发生）。
        let slot = GAUGE12[i % GAUGE12.len()];
        m.insert(b.to_string(), slot);
    }
    cfg.insert(caliber.to_string(), m);
}

/// 构建完整配置（带逻辑：所有口径按书写顺序复用 12 Gauge 公共基准）。
///
/// 注意：每个口径下方子弹列表的**书写顺序**即对应 `GAUGE12` 的网格序号，
/// 务必与交易行界面中该口径子弹从上到下、从左到右的排列一致。
pub fn build_config() -> Config {
    let mut cfg = Config::new();

    

    // —— 其余口径：仅列出子弹名称，坐标全部复用 GAUGE12 对应序号 ——
    insert_caliber(
        &mut cfg,
        ".357 Magnum",
        &[
            ".357 Magnum FMJ",
            ".357 Magnum JHP",
            ".357 Magnum 9号霰弹'鼠弹'",
            ".357 Magnum HP",
        ],
    );

    insert_caliber(
        &mut cfg,
        ".45 ACP",
        &[
            ".45 ACP Super",
            ".45 ACP AP",
            ".45 ACP FMJ",
            ".45 ACP RIP",
            ".45 ACP JHP",
            ".45 ACP HS",
        ],
    );

    insert_caliber(
        &mut cfg,
        ".50 AE",
        &[".50 AE FMJ", ".50 AE JHP", ".50 AE HP"],
    );

    // —— 12 Gauge：公共基准来源，覆盖完整 9 格网格 ——
    insert_caliber(
        &mut cfg,
        "12 Gauge",
        &[
            "12 Gauge DART",
            "12 Gauge 龙息弹",
            "12 Gauge 独头 AP-20",
            "12 Gauge 独头 FTX",
            "12 Gauge 箭形弹",
            "12 Gauge 8.5毫米鹿弹",
            "12 Gauge 独头RIP",
            "12 Gauge 独头 GT",
            "12 Gauge 7毫米鹿弹",
        ],
    );

    insert_caliber(
        &mut cfg,
        "12.7x55",
        &[
            "12.7x55mm PS12B",
            "12.7x55mm PD12双头弹",
            "12.7x55mm PS12",
            "12.7x55mm PS12A",
        ],
    );

    insert_caliber(
        &mut cfg,
        "5.45x39",
        &[
            "5.45x39mm BS",
            "5.45x39mm BT",
            "5.45x39mm PS",
            "5.45x39mm T",
            "5.45x39mm PRS",
        ],
    );

    insert_caliber(
        &mut cfg,
        "5.56x45",
        &[
            "5.56x45mm M995",
            "5.56x45mm M855A1",
            "5.56x45mm M855",
            "5.56x45mm FMJ",
            "5.56x45mm RRLP",
        ],
    );

    insert_caliber(
        &mut cfg,
        "5.7x28",
        &[
            "5.7x28mm SS190",
            "5.7x28mm HT+",
            "5.7x28mm SS193",
            "5.7x28mm HT",
            "5.7x28mm L191",
            "5.7x28mm R37.F",
            "5.7x28mm SS197SR",
            "5.7x28mm SS198LF",
        ],
    );

    insert_caliber(
        &mut cfg,
        "5.8x42",
        &[
            "5.8x42mm DVC12",
            "5.8x42mm DBP10",
            "5.8x42mm DVP88",
            "5.8x42mm DBP87",
        ],
    );

    insert_caliber(
        &mut cfg,
        "6.8x51",
        &["6.8x51mm Hybrid", "6.8x51mm FMJ"],
    );

    insert_caliber(
        &mut cfg,
        "7.62x39",
        &[
            "7.62x39mm AP",
            "7.62x39mm BP",
            "7.62x39mm PS",
            "7.62x39mm T45M",
            "7.62x39mm LP",
        ],
    );

    insert_caliber(
        &mut cfg,
        "7.62x51",
        &[  "7.62x51mm LS",
            "7.62x51mm M62",
            "7.62x51mm M80",
            "7.62x51mm BPZ",
            "7.62x51mm Ultra Nosler",
        ],
    );

    insert_caliber(
        &mut cfg,
        "7.62x54",
        &["7.62x54R BT", "7.62x54R LPS", "7.62x54R T46M"],
    );

    insert_caliber(
        &mut cfg,
        "9x19",
        &[
            "9x19mm PBP",
            "9x19mm AP6.3",
            "9x19mm RIP",
            "9x19mm Pst",
            "9x19mm PSO",
        ],
    );

    insert_caliber(
        &mut cfg,
        "9x39",
        &["9x39mm BP", "9x39mm SP6", "9x39mm SP5"],
    );

    insert_caliber(
        &mut cfg,
        "4.6x30",
        &[
            "4.6x30mm AP SX",
            "4.6x30mm AP ST",
            "4.6x30mm CT",
            "4.6x30mm FMJ ST",
            "4.6x30mm FMJ SX",
            "4.6x30mm Subsonic SX",
        ],
    );

    insert_caliber(
        &mut cfg,
        ".300 BLK",
        &[".300 BLK 5级", ".300 BLK 4级", ".300 BLK 3级"],
    );

    insert_caliber(
        &mut cfg,
        "箭矢",
        &["碳纤维穿甲箭矢", "碳纤维刺骨箭矢", "玻纤柳叶箭矢"],
    );

    insert_caliber(
        &mut cfg,
        "45-70 Govt",
        &["45-70 Govt FTX", "45-70 Govt FMJ", "45-70 Govt RN"],
    );

    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 核查遍历完整性：`BuyScene::all()` 必须覆盖全部场景变体，
    /// 否则「购买后判断 · 模式核查」面板会漏列某模式。
    #[test]
    fn buy_scene_all_covers_every_variant() {
        let all = BuyScene::all();
        assert_eq!(all.len(), 2, "场景变体数量变化：同步更新面板核查与 all()");
        assert!(all.contains(&BuyScene::Exchangeable));
        assert!(all.contains(&BuyScene::NonExchangeable));
        // 两种场景的购买点击坐标均来自各自 `layout().buy`，共享同一条已接入 toast 判定的购买路径，
        // 故二者都已接入「正常/低价购买」判定（架构保证，非逐模式重复实现）。
        for s in all {
            assert!(s.layout().buy[2] > 0 && s.layout().buy[3] > 0, "{:?} 缺少购买按钮坐标", s);
        }
    }
}
