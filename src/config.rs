//! 交易行截图坐标配置（代码化，带逻辑）。
//!
//! 设计要点：
//! - 所有口径的子弹在交易行界面共用同一套 3×3 网格坐标；
//! - 以 `12 Gauge`（9 种子弹，覆盖完整网格）作为**公共基准对象** `GAUGE12`，
//!   持有 9 个网格槽位各自的 `name`（名称区域）与 `price`（价格区域）坐标；
//! - 其余每个口径只需按「文件中书写的顺序」列出子弹名称，第 `i` 枚子弹
//!   **直接复用 `GAUGE12[i]` 的 name/price**，不再各自重复定义。
//!   统一修改坐标只需改 `GAUGE12` 一处。
//!
//! 坐标契约（唯一真相）：`[x, y, w, h]` 以「客户区 / 无标题栏截图」左上角为原点，
//! 与默认全屏截图及 `win::capture_client_rgba` 缓冲原点一致，调用方无需任何偏移换算。

use std::collections::HashMap;

/// 一个矩形区域 `[x, y, w, h]`：相对客户区（无标题栏截图）左上角。
pub type Rect = [i32; 4];

/// 目标游戏窗口标题关键字（用于查找 / 聚焦游戏窗口，标题包含匹配）。
pub const GAME_KEYWORD: &str = "三角洲行动";

/// 坐标标定参考宽度：以下所有坐标常量均按 1080px 宽客户区实测截图标定；
/// 运行时由 `observe::scale_rect` 按实际客户区宽度**等比缩放**，适配任意窗口分辨率 / DPI。
pub const CALIB_REF_W: i32 = 1080;

/// 商品检索栏（口径过滤输入框）矩形 `[x, y, w, h]`，标定基准同上
/// （实测截图中含「.357 Magnum」文本的输入框，点击其中心即聚焦）。
pub const SEARCH_BAR: Rect = [44, 85, 176, 28];

/// 单枚子弹的区域坐标（名称区域 + 价格区域）。
#[derive(Clone, Copy)]
pub struct BulletRects {
    pub name: Rect,
    pub price: Rect,
    /// 价格按数位切分的 5 个独立截图范围，顺序为 **万 / 千 / 百 / 十 / 个**（从左到右）。
    /// 深度学习（或逐位 OCR）对每个位独立识别一位数字，再按序拼接成最终价格，
    /// 避免整框 OCR 因字体/配色把价格数字整片吞掉。
    /// 坐标派生自 `price`（见 `split_price_5`），运行时随 `price` 一并 `scale_rect` 缩放。
    pub price_digits: [Rect; 5],
}

/// 将价格矩形水平均分为 5 个数位框（万/千/百/十/个，从左到右），用于分位识别。
/// 价格右对齐于 `price` 框，单数字位宽度 ≈ 框宽/5；最右侧（个位）吸收取整余量，
/// 避免分割点落在数字正中被切半。返回顺序固定为 [万, 千, 百, 十, 个]。
pub const fn split_price_5(price: Rect) -> [Rect; 5] {
    let step = price[2] / 5;
    let rem = price[2] - step * 4; // 个位吸收余量
    [
        [price[0], price[1], step, price[3]],               // 万（最左）
        [price[0] + step, price[1], step, price[3]],         // 千
        [price[0] + 2 * step, price[1], step, price[3]],     // 百
        [price[0] + 3 * step, price[1], step, price[3]],     // 十
        [price[0] + 4 * step, price[1], rem, price[3]],      // 个（最右，含余量）
    ]
}

/// 12 Gauge 的 9 种子弹 = 公共基准网格（行优先：第 0 行 3 列、第 1 行 3 列、第 2 行 3 列）。
/// 其余口径按书写顺序复用这里的对应序号槽位，统一修改坐标只需改这里。
///
/// 坐标依据：2026-09-27 实测交易行截图（1080×608 客户区）标定——
/// 3×3 网格列起点 x≈249/513/776，名称行 y≈87/179/270，价格行 y≈154/245/336，
/// 价格数字右对齐至「列起点 +246」（矩形已含边距）。
pub const GAUGE12: [BulletRects; 9] = [
    BulletRects { name: [243, 81, 220, 22], price: [405, 148, 96, 24], price_digits: split_price_5([405, 148, 96, 24]) },  // 0: 12 Gauge DART
    BulletRects { name: [507, 81, 220, 22], price: [669, 148, 96, 24], price_digits: split_price_5([669, 148, 96, 24]) },  // 1: 12 Gauge 龙息弹
    BulletRects { name: [770, 81, 220, 22], price: [932, 148, 96, 24], price_digits: split_price_5([932, 148, 96, 24]) },  // 2: 12 Gauge 独头 AP-20
    BulletRects { name: [243, 173, 220, 22], price: [405, 239, 96, 24], price_digits: split_price_5([405, 239, 96, 24]) }, // 3: 12 Gauge 独头 FTX
    BulletRects { name: [507, 173, 220, 22], price: [669, 239, 96, 24], price_digits: split_price_5([669, 239, 96, 24]) }, // 4: 12 Gauge 箭形弹
    BulletRects { name: [770, 173, 220, 22], price: [932, 239, 96, 24], price_digits: split_price_5([932, 239, 96, 24]) }, // 5: 12 Gauge 8.5毫米鹿弹
    BulletRects { name: [243, 264, 220, 22], price: [405, 330, 96, 24], price_digits: split_price_5([405, 330, 96, 24]) }, // 6: 12 Gauge 独头RIP
    BulletRects { name: [507, 264, 220, 22], price: [669, 330, 96, 24], price_digits: split_price_5([669, 330, 96, 24]) }, // 7: 12 Gauge 独头 GT
    BulletRects { name: [770, 264, 220, 22], price: [932, 330, 96, 24], price_digits: split_price_5([932, 330, 96, 24]) }, // 8: 12 Gauge 7毫米鹿弹
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
            "5.7x28mm SS193",
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
        &[
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
