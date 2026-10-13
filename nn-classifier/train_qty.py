#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""三角洲行动交易行 — 子弹「所选数量」整体识别模型训练（整块数字区域分类）。

与 `train.py` 的价格/数量**逐位**识别（个/十/百分别裁剪、各自分类）不同，本脚本
对**整块数字区域**做**整体分类**：输入是一张裁剪好的数量显示区截图，模型直接输出
该区域表达的整数值（1..200，共 200 类；类别索引 = 数值 - 1，不含 0）。

之所以改为整块分类：购买界面中「所选子弹数量」是**居中显示**的，不同位数（1/2/3 位）
对应的字符位置并不固定在某一列——逐位裁剪会把同一数字切到不同坐标，坐标一旦偏差就
整位错乱。整块分类绕开了逐位对齐问题，鲁棒性更好。

## 两个场景的区域坐标（标定视口 1920×1080，[x,y,w,h] 客户区左上角原点）
- 可兑换子弹的场景： (1560, 722, 140, 32)
- 不可兑换子弹的场景：(1560, 782, 140, 32)

## 子命令
- `collect`：从「完整购买界面截图」按场景裁剪整块数量区域，并依标注写入
            `qty_dataset/<label>/`（`<label>` 即该帧整数值 1..200），产出可直接训练的数据集。
- `train`  ：在 `qty_dataset/` 上训练整块区域分类器，导出 `qty.onnx`（供 Rust/ORT 推理）。
- `infer`  ：对单张图片快速验证模型输出（用 PyTorch 权重 `qty.pt`，无需 onnxruntime）。

依赖（共享 requirements.txt）：torch / torchvision / pillow / numpy / onnx。

前处理契约（必须与 `src/domain/qty_classifier.rs` 的 `preprocess` 完全一致）：
    RGB → 灰度 (ITU-R 601) → **纵向重心对齐 + 线性对比拉伸**（见 `align_and_stretch`）
    → 双线性缩放 (QTY_INPUT_H×QTY_INPUT_W) → [0,1] → (x-0.5)/0.5

为什么必须「纵向对齐 + 拉伸」（2026-10-05 实测定位的真正根因）：
  1. 可兑换/不可兑换两个场景的数量行在全局截图上相差约 58px（y≈740 / y≈798），
     两个场景各自用己方区域坐标裁剪后，数字在 140×32 裁剪区内的局部行仍相差 2px
     （可兑换局部 y≈18，不可兑换 y≈16）。数字笔画仅 1px 宽、高约 13px，纵向错位 2px
     即令同值两张截图的逐像素差异（类内差 12.6）超过不同数值间的差异（类间差 10.2），
     CNN 完全无法收敛（acc≈1/200）。按数字亮像素重心做整行纵向平移对齐后：
     类内差 0.63，类间差 18.4（比值 29 倍），1-NN 留一法 97.5% —— 数据高度可分。
  2. 数字为细笔画、低对比（笔画峰值灰度≈152，面板背景≈37），直接归一化后信号微弱；
     线性拉伸把背景压到 0、笔画推到 255，显著提升可分性。
"""

import argparse
import json
import os
import random
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.utils.data import DataLoader, Dataset
from torchvision import transforms
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))

# =========================================================================== #
# 常量（与 Rust 端 qty_classifier.rs / config.rs 必须一致）
# =========================================================================== #
# 整块数量区域：标定视口坐标 [x, y, w, h]，客户区左上角原点。
# 键名用于 JSONL 标注的 `scene` 字段。
QTY_REGION_EXCHANGEABLE = (1560, 722, 140, 32)
QTY_REGION_NONEXCHANGEABLE = (1560, 782, 140, 32)

# ONNX / 推理输入尺寸（高 × 宽）。140×31 区域的宽高比约 4.5:1，
# 这里用 32×140 近等比缩放，仅引入极小形变，避免正方形拉伸把数字压扁。
QTY_INPUT_H = 32
QTY_INPUT_W = 140

# 分类种别数：数量取值 1..200（不含 0），共 200 类。类别索引 = 数值 - 1。
# 即目录名 `<value>`(1..200) → 训练标签 `value - 1`(0..199) → 模型输出索引。
QTY_NUM_CLASSES = 200
QTY_MIN_LABEL = 1
QTY_MAX_LABEL = 200

QTY_DATASET_DIR = os.path.join(HERE, "qty_dataset")
QTY_MODEL_PT = os.path.join(HERE, "qty.pt")
QTY_ONNX = os.path.join(HERE, "qty.onnx")

# =========================================================================== #
# 前处理：纵向重心对齐 + 线性对比拉伸（训练与推理共用，须与 Rust 端实现一致）
# =========================================================================== #
# 数字笔画判定阈值：笔画灰度约 110-153，面板背景约 30-45，取 110 可稳定只留笔画。
QTY_STROKE_THRESH = 110
# 线性拉伸端点（实测标定）：背景≈37 → 0，笔画峰值≈152 → 255。
QTY_STRETCH_LOW = 37
QTY_STRETCH_HIGH = 152
# 对齐目标行：把数字亮像素重心平移到该行（裁剪区高 32 的中线附近）。
QTY_ALIGN_ROW = 16


def align_and_stretch(gray: np.ndarray) -> np.ndarray:
    """对单张灰度图做「纵向重心对齐 + 线性对比拉伸」，消除两场景间的纵向错位。

    - 对齐：以亮像素(>= QTY_STROKE_THRESH)的行重心为锚，整幅图纵向平移
      `round(QTY_ALIGN_ROW - 重心行)` 像素（越界补背景值），消除可兑换/不可兑换
      两场景裁剪区内数字 2px 的纵向错位。
    - 拉伸：`clip((x - LOW) * 255 / (HIGH - LOW), 0, 255)`，把细数字的低对比信号拉开。

    Rust 端 `qty_classifier.rs::preprocess` 必须逐步复刻本函数，训练/推理才一致。
    """
    gray = gray.astype(np.float32)
    mask = gray >= QTY_STROKE_THRESH
    if mask.any():
        cy = float(np.nonzero(mask)[0].mean())
        shift = int(round(QTY_ALIGN_ROW - cy))
        if shift != 0:
            filled = np.full_like(gray, QTY_STRETCH_LOW)
            if shift > 0:
                filled[shift:] = gray[: len(gray) - shift]
            else:
                filled[:shift] = gray[-shift:]
            gray = filled
    lo, hi = float(QTY_STRETCH_LOW), float(QTY_STRETCH_HIGH)
    out = np.clip((gray - lo) * (255.0 / (hi - lo)), 0.0, 255.0)
    return out.astype(np.uint8)


def load_qty_gray(path) -> Image.Image:
    """读取数据集图片并完成对齐+拉伸，返回 PIL 'L' 图（供 transform 继续处理）。"""
    img = Image.open(path).convert("L")
    return Image.fromarray(align_and_stretch(np.asarray(img)), mode="L")


# 预处理：灰度 → 32×140 → [0,1] → 归一化到 [-1,1]（(x-0.5)/0.5）。
# 注意 torchvision.transforms.Resize 接收的是 (h, w)。
# 显式指定 BILINEAR，与 Rust 端 `image::imageops::resize(..., FilterType::Triangle)`（双线性）一致，
# 避免 torchvision 默认 BICUBIC 带来的训练/推理前处理偏差（即便当前裁剪尺寸恰为 32×140 使缩放为恒等，
# 一旦更换输入尺寸或坐标，该一致性即生效）。
_QTY_RESIZE = dict(interpolation=transforms.InterpolationMode.BILINEAR)
QTY_TRANSFORM = transforms.Compose(
    [
        transforms.Grayscale(num_output_channels=1),
        transforms.Resize((QTY_INPUT_H, QTY_INPUT_W)),
        transforms.ToTensor(),
        transforms.Normalize((0.5,), (0.5,)),
    ]
)

# 训练期在线增强（关键但须克制）：游戏 UI 数字为确定性渲染，同一数值两次截图几乎像素一致，
# 若不增强，模型会「背」下那几张精确像素而非学抽象数字特征 → 推理时坐标亚像素偏移/压缩即失灵。
# 增强只引入「真实会发生的弱扰动」：裁剪位置抖动（教位置不变性）、亮度/对比度微漂移。
# ⚠️ 严禁强模糊/强几何：输入仅 32 像素高，强高斯模糊会把细数字糊成噪声、强变换让单张样本无法学习。
# 增强仅在训练集应用，验证/推理用 QTY_TRANSFORM。
QTY_TRAIN_TRANSFORM = transforms.Compose(
    [
        transforms.Grayscale(num_output_channels=1),
        # 平移/缩放抖动：模拟裁剪框与真实渲染位置的亚像素偏差（横 3%≈4px，纵 3%≈1px）。
        # 取值保守，确保每张被增强后仍清晰可辨。
        transforms.RandomAffine(
            degrees=0,
            translate=(0.03, 0.03),
            scale=(0.98, 1.02),
            fill=0,
        ),
        # 亮度/对比度微漂移：模拟截图曝光与压缩差异。
        transforms.ColorJitter(brightness=0.12, contrast=0.12),
        transforms.Resize((QTY_INPUT_H, QTY_INPUT_W), **_QTY_RESIZE),
        transforms.ToTensor(),
        transforms.Normalize((0.5,), (0.5,)),
    ]
)


# =========================================================================== #
# 模型
# =========================================================================== #
class QtyRegionCNN(nn.Module):
    """轻量 CNN：3 个卷积块（conv+relu+maxpool）+ 2 层全连接分类头（200 类）。

    输入： [B, 1, 32, 140]（灰度整块数量区域）
    输出： [B, 200]（logits，索引 = 数值 - 1，即索引 0 → 数值 1，索引 199 → 数值 200；不含 0）
    """

    def __init__(self, num_classes: int = QTY_NUM_CLASSES):
        super().__init__()
        self.features = nn.Sequential(
            nn.Conv2d(1, 16, 3, padding=1),
            nn.ReLU(inplace=True),
            nn.MaxPool2d(2),  # 32×140 -> 16×70
            nn.Conv2d(16, 32, 3, padding=1),
            nn.ReLU(inplace=True),
            nn.MaxPool2d(2),  # 16×70 -> 8×35
            nn.Conv2d(32, 64, 3, padding=1),
            nn.ReLU(inplace=True),
            nn.MaxPool2d(2),  # 8×35 -> 4×17
        )
        # 64 * 4 * 17 = 4352
        self.classifier = nn.Sequential(
            nn.Flatten(),
            nn.Linear(64 * 4 * 17, 128),
            nn.ReLU(inplace=True),
            nn.Linear(128, num_classes),
        )

    def forward(self, x):
        return self.classifier(self.features(x))


# =========================================================================== #
# 数据集
# =========================================================================== #
class QtyDataset(Dataset):
    """读取 `qty_dataset/<label>/<img>`：目录名即**整数数量值**（1..200）。

    训练标签 = `value - QTY_MIN_LABEL`（= value - 1），即模型输出索引（0..199）；
    过滤掉非 [1,200] 区间的目录，确保数量值与模型索引正确对应（索引 0 不表示 0）。

    - `augment=True`：使用 `QTY_TRAIN_TRANSFORM`（含在线增强），用于训练集；
    - `augment=False`：使用 `QTY_TRANSFORM`（无增强），用于验证/推理。
    - `samples` 可传入预切分的样本列表（(path, label)），用于训练/验证按类分拆，
      不传则扫描 `root` 目录自动构建（兼容原用法）。
    """

    def __init__(self, root: str, transform=None, augment: bool = True, samples=None):
        self.root = Path(root)
        self.augment = augment
        self.transform = transform or (QTY_TRAIN_TRANSFORM if augment else QTY_TRANSFORM)
        if samples is not None:
            self.samples = list(samples)
        else:
            self.samples = []  # (path: Path, label: int)
            if not self.root.is_dir():
                raise SystemExit(f"未找到数据集目录：{self.root}")
            for label_dir in sorted(self.root.iterdir()):
                if not label_dir.is_dir():
                    continue
                try:
                    label = int(label_dir.name)
                except ValueError:
                    continue
                if label < QTY_MIN_LABEL or label > QTY_MAX_LABEL:
                    continue
                for img in label_dir.iterdir():
                    if img.suffix.lower() in (".png", ".jpg", ".jpeg", ".bmp"):
                        self.samples.append((img, label))
        if not self.samples:
            raise SystemExit(
                f"数据集为空：{self.root}\n"
                "请先用 `python train_qty.py collect` 从购买界面截图裁剪整块数量区域，"
                "或直接把标注好的整块区域图放入 qty_dataset/<label>/。"
            )

    def __len__(self):
        return len(self.samples)

    def __getitem__(self, idx):
        path, label = self.samples[idx]
        # 纵向对齐 + 对比拉伸（消除两场景 2px 纵向错位、拉开细数字对比度），
        # 之后走常规 transform（缩放/归一化）。
        img = load_qty_gray(path)
        # 目录名是数量值（1..200），转为类别索引（0..199 = value - 1）。
        return self.transform(img), label - QTY_MIN_LABEL


def split_samples_by_class(samples, val_ratio: float, seed: int = 42):
    """按类分层抽样切分训练/验证集，保证每个出现的类在两端都有样本。

    返回 (train_samples, val_samples)。用于在小样本下也能得到可参考的验证准确率。

    小数据保护：当某类样本 < 4 时，该类**全部归入训练集、不切验证**（避免每类仅 2 张被切到
    只剩 1 张训练样本，导致单张样本经增强后无法学习）。验证集仅由「样本充足的类」贡献。
    """
    rng = random.Random(seed)
    by_class = {}
    for s in samples:
        by_class.setdefault(s[1], []).append(s)
    train, val = [], []
    for label, items in by_class.items():
        if len(items) < 4:
            # 样本过少：全留给训练，跳过验证，避免饿死训练集。
            train.extend(items)
            continue
        rng.shuffle(items)
        n_val = max(1, round(len(items) * val_ratio))
        n_val = min(n_val, len(items) - 2)  # 训练侧至少留 2 张
        val.extend(items[:n_val])
        train.extend(items[n_val:])
    return train, val


def dataset_signal_report(samples, max_per_class: int = 8, n_inter_pairs: int = 400):
    """粗略估计数据集是否含可分信号：比较「类内平均像素差」与「类间平均像素差」。

    正常数据：不同数值的截图应比同数值两张**差异更大**（类间差 ≫ 类内差）。
    若类间差 ≤ 类内差：说明裁剪区域未捕获有效数字（截到静止背景/标签），训练无意义。
    返回 (intra_mean, inter_mean)，像素值范围 0-255。
    """
    from collections import defaultdict

    by_class: dict = defaultdict(list)
    for p, l in samples:
        by_class[l].append(p)

    def _load(p):
        # 与训练一致：先对齐+拉伸再比差，否则 2px 纵向错位会淹没数字信号造成误报。
        return np.asarray(load_qty_gray(p)).astype(np.int32)

    intra = []
    for label, paths in by_class.items():
        if len(paths) < 2:
            continue
        arrs = [_load(p) for p in paths[:max_per_class]]
        for i in range(len(arrs)):
            for j in range(i + 1, len(arrs)):
                intra.append(float(np.abs(arrs[i] - arrs[j]).mean()))
    inter = []
    classes = list(by_class.keys())
    rng = random.Random(0)
    tried = 0
    while len(inter) < n_inter_pairs and tried < n_inter_pairs * 4:
        tried += 1
        a, b = rng.sample(classes, 2)
        inter.append(float(np.abs(_load(by_class[a][0]) - _load(by_class[b][0])).mean()))
    intra_mean = sum(intra) / len(intra) if intra else 0.0
    inter_mean = sum(inter) / len(inter) if inter else 0.0
    return intra_mean, inter_mean


# =========================================================================== #
# 子命令：collect —— 从完整截图裁剪整块数量区域
# =========================================================================== #
def _crop_region(img: Image.Image, region) -> Image.Image:
    """按 [x, y, w, h] 在原图中裁剪（越界夹紧）。"""
    w, h = img.size
    x, y, rw, rh = region
    x0 = max(0, min(x, w))
    y0 = max(0, min(y, h))
    x1 = max(x0, min(x + rw, w))
    y1 = max(y0, min(y + rh, h))
    return img.crop((x0, y0, x1, y1))


def _scene_region(scene: str):
    if scene == "non" or scene == "non-exchangeable" or scene == "不可兑换":
        return QTY_REGION_NONEXCHANGEABLE
    # 默认按「可兑换」场景处理（"exchangeable" / "可兑换" / 未知）
    return QTY_REGION_EXCHANGEABLE


def cmd_collect(args) -> None:
    """从完整截图裁剪整块数量区域，按标注写入 qty_dataset/<label>/。

    标注来源（二选一）：
    1. `--anno qty_anno.jsonl`：每行 `{"image":"shots/001.png","scene":"exchangeable","label":31}`；
    2. `--from-screenshots DIR --scene exchangeable --label N`：目录下所有图统一场景与标签
       （用于整批同一数值的简单场景）。
    """
    out_root = Path(args.out)
    out_root.mkdir(parents=True, exist_ok=True)

    jobs = []  # (image_path, region, label)
    if args.anno:
        anno_path = Path(args.anno)
        if not anno_path.is_file():
            raise SystemExit(f"未找到标注文件：{anno_path}")
        with open(anno_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                obj = json.loads(line)
                img = obj["image"]
                scene = obj.get("scene", "exchangeable")
                label = int(obj["label"])
                if label < QTY_MIN_LABEL or label > QTY_MAX_LABEL:
                    print(f"[collect] 跳过越界标签 {label}（应 ∈ 1..200）：{img}")
                    continue
                region = _scene_region(scene)
                jobs.append((img, region, label))
    elif args.from_screenshots:
        d = Path(args.from_screenshots)
        if not d.is_dir():
            raise SystemExit(f"未找到截图目录：{d}")
        region = _scene_region(args.scene)
        label = args.label
        if label is None or label < QTY_MIN_LABEL or label > QTY_MAX_LABEL:
            raise SystemExit(f"--label 必须在 1..200 之间（当前 {label}）")
        for img in sorted(d.iterdir()):
            if img.suffix.lower() in (".png", ".jpg", ".jpeg", ".bmp"):
                jobs.append((str(img), region, label))
    else:
        raise SystemExit("collect 需要 --anno <jsonl> 或 --from-screenshots <dir> [--scene --label]")

    if not jobs:
        raise SystemExit("没有可裁剪的标注/图片。")

    saved = 0
    for img_rel, region, label in jobs:
        img_path = Path(img_rel)
        if not img_path.is_file():
            # 允许相对 nn-classifier 目录或绝对路径
            img_path = Path(HERE) / img_rel
        if not img_path.is_file():
            print(f"[collect] 跳过缺失图片：{img_rel}")
            continue
        try:
            img = Image.open(img_path).convert("RGB")
        except Exception as e:
            print(f"[collect] 读取失败 {img_rel}: {e}")
            continue
        crop = _crop_region(img, region)
        label_dir = out_root / str(label)
        label_dir.mkdir(parents=True, exist_ok=True)
        stem = img_path.stem
        out_name = f"{stem}.png"
        # 同名则加序号避免覆盖
        if (label_dir / out_name).exists():
            n = 1
            while (label_dir / f"{stem}_{n}.png").exists():
                n += 1
            out_name = f"{stem}_{n}.png"
        crop.save(label_dir / out_name)
        saved += 1

    print(f"[collect] 完成：从 {len(jobs)} 条标注中成功裁剪保存 {saved} 张整块数量区域图。")
    print(f"          数据集根目录：{out_root}")


# =========================================================================== #
# 子命令：train
# =========================================================================== #
def export_qty_onnx(model, out_path: str):
    model.eval()
    dummy = torch.randn(1, 1, QTY_INPUT_H, QTY_INPUT_W)
    torch.onnx.export(
        model,
        dummy,
        out_path,
        input_names=["input"],
        output_names=["logits"],
        dynamic_axes={"input": {0: "batch"}, "logits": {0: "batch"}},
        dynamo=False,
    )
    import onnx as _onnx

    _onnx.checker.check_model(_onnx.load(out_path))
    print(f"已导出 ONNX（已通过 checker 校验）：{out_path}")


def cmd_train(args) -> None:
    if args.only_export:
        if not os.path.isfile(QTY_MODEL_PT):
            raise SystemExit(f"未找到 {QTY_MODEL_PT}，请先训练或去掉 --only-export。")
        model = QtyRegionCNN()
        model.load_state_dict(torch.load(QTY_MODEL_PT, map_location="cpu"))
        export_qty_onnx(model, QTY_ONNX)
        return

    # 全量扫描拿到样本清单，再按类切分训练/验证，保证验证准确率可信（而非训练集自报）。
    full = QtyDataset(QTY_DATASET_DIR, augment=False)
    from collections import Counter

    cnt = Counter(l for _, l in full.samples)
    present = sum(1 for v in range(QTY_MIN_LABEL, QTY_MAX_LABEL + 1) if cnt.get(v, 0) > 0)
    min_per = min(cnt.values())
    max_per = max(cnt.values())
    print(f"样本总数：{len(full)}，覆盖标签数：{present}/{QTY_NUM_CLASSES}，"
          f"最少/最多类样本：{min_per}/{max_per}")
    if present < QTY_NUM_CLASSES:
        missing = [v for v in range(QTY_MIN_LABEL, QTY_MAX_LABEL + 1) if cnt.get(v, 0) == 0]
        print(f"[提示] 缺失标签（无样本，将识别不准）：{missing}")

    # 信号自检：若类间差异 ≤ 类内差异，数据集几乎不含可分信号（裁剪坐标多半框错位置）。
    intra_m, inter_m = dataset_signal_report(full.samples)
    print(f"[信号自检] 类内平均差={intra_m:.2f}，类间平均差={inter_m:.2f}（像素值 0-255）")
    if inter_m <= intra_m * 1.2:
        print("=" * 64)
        print("[严重] 对齐+拉伸后类间差异仍 ≤ 类内差异：数据集几乎不含可分信号！")
        print("  可能原因：① 裁剪坐标未框住数字显示区（截到静止背景/标签）；")
        print("            ② 数字过细/过暗且对齐后仍无法分辨（阈值不匹配）。")
        print("  请先用 calibrate.py 目视裁剪区，再检查 QTY_STROKE_THRESH / 拉伸端点。")
        print("=" * 64)
    # 数据量不足告警：每类样本过少时，增强会让模型无法学习（无冗余学不变性），
    # 只能死记精确像素 → 推理时坐标/压缩一变即失灵。须先多采集。
    if not args.no_augment and min_per < 5:
        print("=" * 64)
        print("[重要] 每类样本过少（最少 %d 张 < 5）：" % min_per)
        print("  - 在线增强会令模型无法收敛（实测 acc≈随机 1/200），本次训练大概率无效；")
        print("  - 请先用「数量采集」Tab 把每个数值攒到 10~20 张（重跑会追加、不覆盖），")
        print("    再开增强重训；或临时用 `--no-augment` 跑出可记忆模型（仅验证管线，不具泛化性）。")
        print("=" * 64)

    if args.no_augment:
        # 关闭增强：全量用于训练，不切分验证。
        train_ds = full
        val_ds = None
        print("[增强] 已关闭（--no-augment）")
    else:
        train_samples, val_samples = split_samples_by_class(
            full.samples, val_ratio=args.val_ratio, seed=args.seed
        )
        train_ds = QtyDataset(QTY_DATASET_DIR, augment=True, samples=train_samples)
        val_ds = QtyDataset(QTY_DATASET_DIR, augment=False, samples=val_samples) if val_samples else None
        if val_ds is not None:
            print(f"[增强] 已启用在线增强；训练 {len(train_ds)} / 验证 {len(val_ds)}")
        else:
            print(f"[增强] 已启用在线增强；每类样本<4，未切分验证集（训练 {len(train_ds)}）。"
                  f"建议多采集每类样本以启用验证。")

    train_loader = DataLoader(train_ds, batch_size=args.batch_size, shuffle=True, num_workers=0)
    val_loader = (
        DataLoader(val_ds, batch_size=args.batch_size, shuffle=False, num_workers=0)
        if val_ds is not None
        else None
    )

    model = QtyRegionCNN()
    criterion = nn.CrossEntropyLoss()
    optimizer = torch.optim.Adam(model.parameters(), lr=args.lr)

    def _eval(loader):
        model.eval()
        total, correct = 0, 0
        with torch.no_grad():
            for xb, yb in loader:
                out = model(xb)
                correct += (out.argmax(dim=1) == yb).sum().item()
                total += yb.size(0)
        model.train()
        return correct / total if total else 0.0

    for epoch in range(1, args.epochs + 1):
        total, correct, loss_sum = 0, 0, 0.0
        for xb, yb in train_loader:
            optimizer.zero_grad()
            out = model(xb)
            loss = criterion(out, yb)
            loss.backward()
            optimizer.step()
            loss_sum += loss.item() * xb.size(0)
            correct += (out.argmax(dim=1) == yb).sum().item()
            total += yb.size(0)
        train_acc = correct / total
        val_acc = _eval(val_loader) if val_loader is not None else float("nan")
        val_str = f"  val_acc={val_acc:.4f}" if val_loader is not None else ""
        print(f"epoch {epoch:03d}  loss={loss_sum / total:.4f}  acc={train_acc:.4f}{val_str}")

    torch.save(model.state_dict(), QTY_MODEL_PT)
    print(f"已保存模型：{QTY_MODEL_PT}")
    if args.export_onnx:
        export_qty_onnx(model, QTY_ONNX)


# =========================================================================== #
# 子命令：infer —— 快速验证（用 PyTorch 权重，无需 onnxruntime）
# =========================================================================== #
def cmd_infer(args) -> None:
    if not os.path.isfile(QTY_MODEL_PT):
        raise SystemExit(f"未找到 {QTY_MODEL_PT}，请先 `python train_qty.py train`。")
    model = QtyRegionCNN()
    model.load_state_dict(torch.load(QTY_MODEL_PT, map_location="cpu"))
    model.eval()

    img = Image.open(args.image).convert("RGB")
    # 单图推理：可选先按场景裁剪整块区域（与 collect 同款逻辑）。
    if args.scene:
        img = _crop_region(img, _scene_region(args.scene))
    # 与训练一致：先对齐+拉伸再走 transform。
    img = Image.fromarray(align_and_stretch(np.asarray(img.convert("L"))), mode="L")
    x = QTY_TRANSFORM(img).unsqueeze(0)
    with torch.no_grad():
        logits = model(x)[0]
        probs = F.softmax(logits, dim=0)
        pred_idx = int(probs.argmax().item())
        conf = float(probs[pred_idx].item())
    # 类别索引 → 数值：value = index + QTY_MIN_LABEL（QTY_MIN_LABEL=1）。
    value = pred_idx + QTY_MIN_LABEL
    print(f"[infer] {args.image} -> 预测数量 = {value}（类别索引 {pred_idx}，置信度 {conf:.3f}）")


# =========================================================================== #
# CLI
# =========================================================================== #
def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="train_qty.py",
        description="三角洲行动交易行 — 子弹「所选数量」整块区域分类训练（1..200）",
    )
    sub = p.add_subparsers(dest="task")

    pc = sub.add_parser("collect", help="从完整截图裁剪整块数量区域并标注")
    pc.add_argument("--anno", help="标注 JSONL：每行 {image, scene, label}")
    pc.add_argument("--from-screenshots", help="整目录截图统一处理（配合 --scene/--label）")
    pc.add_argument("--scene", default="exchangeable", help="场景：exchangeable / non-exchangeable")
    pc.add_argument("--label", type=int, default=None, help="统一标签（1..200，配合 --from-screenshots）")
    pc.add_argument("--out", default=QTY_DATASET_DIR, help="输出数据集根目录（默认 qty_dataset/）")

    pt = sub.add_parser("train", help="训练整块区域分类器并导出 qty.onnx")
    pt.add_argument("--epochs", type=int, default=60)
    pt.add_argument("--batch-size", type=int, default=32)
    pt.add_argument("--lr", type=float, default=1e-3)
    pt.add_argument("--val-ratio", type=float, default=0.15, help="验证集占比（按类分层抽样）")
    pt.add_argument("--seed", type=int, default=42, help="数据集切分随机种子")
    pt.add_argument("--no-augment", action="store_true", help="关闭在线数据增强")
    pt.add_argument("--export-onnx", action="store_true", help="训练后导出 qty.onnx")
    pt.add_argument("--only-export", action="store_true", help="仅导出 ONNX（需已存在 qty.pt）")

    pi = sub.add_parser("infer", help="对单张图片快速验证（用 PyTorch 权重）")
    pi.add_argument("image", help="图片路径（可为完整截图或已裁剪的整块区域）")
    pi.add_argument("--scene", default=None, help="若为完整截图，指定场景以先裁剪整块区域")

    return p


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    task = args.task or "train"
    if task == "collect":
        cmd_collect(args)
    elif task == "infer":
        cmd_infer(args)
    else:
        cmd_train(args)


if __name__ == "__main__":
    main()


# 用法示例：
#   # 1) 从标注好的完整截图裁剪整块数量区域（label = 该帧整数值）
#   python train_qty.py collect --anno qty_anno.jsonl --out qty_dataset
#   # 1b) 攒数据：重跑「数量采集」Tab 会追加 _1/_2… 不覆盖，建议每个数值攒到 10~20 张再训
#   # 2) 训练（默认 60 epoch + 在线增强 + 按类切分验证集，打印 val_acc 供参考）并导出 ONNX
#   python train_qty.py train --epochs 60 --batch-size 32 --export-onnx
#   #    关闭增强对比：python train_qty.py train --no-augment --epochs 60
#   # 3) 快速验证
#   python train_qty.py infer shots/buyui_001.png --scene exchangeable
