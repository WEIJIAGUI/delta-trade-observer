#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""三角洲行动交易行 — 神经网络训练（合并工程 / 唯一训练入口）。

本文件是 `nn-classifier` 目录下的 **唯一训练入口**，同时承担两类模型的训练：

  1. digit   —— 价格数字分类（11 类：0-9 + x），替代模板匹配；
               输出 model.pt + classes.json，可选导出 model.onnx 供 Rust/ORT 推理。
  2. button  —— 「交易行按钮布局」三分类目标检测（CenterNet 风格，按钮位置随布局浮动）：
               buy / minus / plus 三类按钮定位；模型输出后以「购买」按钮 bbox 判定布局
               ①/②，minus/plus 漏检时回退到该布局的固定模板坐标。
               输出 buttons.pt，训练后默认导出 buttons.onnx。

通过子命令选择任务：

    python train.py digit  [--epochs 20] [--batch-size 64] [--lr 1e-3] [--export-onnx model.onnx]
    python train.py button [--epochs 30] [--batch-size 16] [--lr 1e-3] [--data button_dataset]
    python train.py                        # 无子命令时等同于 digit（保持旧调用兼容）

依赖（共享 requirements.txt）：torch / torchvision / pillow / numpy / onnx。
（onnxruntime 仅 predict_layout.py 走 ONNX 推理时需要，可选安装。）

两个任务共享同一份第三方依赖、同一套 PyTorch 训练骨架，互不冲突。
"""

import argparse
import json
import math
import os
import random

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.utils.data import DataLoader, Dataset
from torchvision import datasets, transforms
from PIL import Image, ImageEnhance

HERE = os.path.dirname(os.path.abspath(__file__))


# =========================================================================== #
# 任务 1：数字分类（digit）
# =========================================================================== #
# 数据来源：Rust 端「保存数字用于训练」后，每枚子弹的 4 个数位裁剪落入
# `dataset/<label>/`（0-9 数字目录 + x 非数字目录），人工校正后得到干净训练集。

DIGIT_DATASET_DIR = os.path.join(HERE, "dataset")
DIGIT_MODEL_PATH = os.path.join(HERE, "model.pt")
DIGIT_CLASSES_PATH = os.path.join(HERE, "classes.json")

# 固定类别顺序：0..9 然后 x（x = 非数字）。推理端必须按此顺序解释模型输出。
CLASSES = [str(i) for i in range(10)] + ["x"]
CLASS_TO_IDX = {c: i for i, c in enumerate(CLASSES)}
NUM_CLASSES = len(CLASSES)

DIGIT_INPUT_SIZE = 32  # 与 Rust 端 ONNX 推理前处理必须一致。

# 预处理：灰度 -> 32x32 -> [0,1] -> 归一化到 [-1,1]（(x-0.5)/0.5）。
DIGIT_TRANSFORM = transforms.Compose(
    [
        transforms.Grayscale(num_output_channels=1),
        transforms.Resize((DIGIT_INPUT_SIZE, DIGIT_INPUT_SIZE)),
        transforms.ToTensor(),
        transforms.Normalize((0.5,), (0.5,)),
    ]
)


class DigitCNN(nn.Module):
    """轻量 CNN：2 个卷积块（conv+relu+maxpool）+ 2 层全连接分类头。"""

    def __init__(self, num_classes: int = NUM_CLASSES):
        super().__init__()
        self.features = nn.Sequential(
            nn.Conv2d(1, 16, 3, padding=1),
            nn.ReLU(),
            nn.MaxPool2d(2),
            nn.Conv2d(16, 32, 3, padding=1),
            nn.ReLU(),
            nn.MaxPool2d(2),
        )
        self.classifier = nn.Sequential(
            nn.Flatten(),
            nn.Linear(32 * (DIGIT_INPUT_SIZE // 4) * (DIGIT_INPUT_SIZE // 4), 64),
            nn.ReLU(),
            nn.Linear(64, num_classes),
        )

    def forward(self, x):
        return self.classifier(self.features(x))


def build_digit_loader(batch_size: int):
    ds = datasets.ImageFolder(DIGIT_DATASET_DIR, transform=DIGIT_TRANSFORM)
    # 将 ImageFolder 的默认类别索引映射为固定 CLASS_TO_IDX 顺序。
    map_fn = {ds.class_to_idx[c]: CLASS_TO_IDX[c] for c in CLASSES}
    ds.targets = [map_fn[t] for t in ds.targets]
    loader = DataLoader(ds, batch_size=batch_size, shuffle=True)
    return ds, loader


def export_digit_onnx(model, out_path: str):
    model.eval()
    dummy = torch.randn(1, 1, DIGIT_INPUT_SIZE, DIGIT_INPUT_SIZE)
    # 传统 TorchScript 导出器（dynamo=False）：不依赖 onnxscript，与 ORT 推理端对齐稳定。
    torch.onnx.export(
        model,
        dummy,
        out_path,
        input_names=["input"],
        output_names=["logits"],
        dynamic_axes={"input": {0: "batch"}, "logits": {0: "batch"}},
        dynamo=False,
    )
    print(f"已导出 ONNX：{out_path}")


def train_digit(args) -> None:
    if args.only_export:
        if not os.path.isfile(DIGIT_MODEL_PATH):
            raise SystemExit(
                f"未找到 {DIGIT_MODEL_PATH}，请先完成一次训练，或去掉 --only-export。"
            )
        model = DigitCNN()
        model.load_state_dict(torch.load(DIGIT_MODEL_PATH, map_location="cpu"))
        out_path = args.export_onnx or "model.onnx"
        export_digit_onnx(model, out_path)
        return

    if not os.path.isdir(DIGIT_DATASET_DIR) or not any(os.listdir(DIGIT_DATASET_DIR)):
        raise SystemExit(
            f"未找到非空数据集：{DIGIT_DATASET_DIR}\n"
            "请先在 Rust 端勾选「保存数字用于训练」采集数据，并人工校正标签。"
        )

    ds, loader = build_digit_loader(args.batch_size)
    print(f"类别（固定顺序）：{CLASSES}")
    print(f"实际样本类别：{sorted(set(ds.classes))}")
    print(f"样本总数：{len(ds)}")

    model = DigitCNN()
    criterion = nn.CrossEntropyLoss()
    optimizer = torch.optim.Adam(model.parameters(), lr=args.lr)

    model.train()
    for epoch in range(1, args.epochs + 1):
        total = 0
        correct = 0
        loss_sum = 0.0
        for xb, yb in loader:
            optimizer.zero_grad()
            out = model(xb)
            loss = criterion(out, yb)
            loss.backward()
            optimizer.step()
            loss_sum += loss.item() * xb.size(0)
            correct += (out.argmax(dim=1) == yb).sum().item()
            total += yb.size(0)
        print(f"epoch {epoch:03d}  loss={loss_sum / total:.4f}  acc={correct / total:.4f}")

    torch.save(model.state_dict(), DIGIT_MODEL_PATH)
    with open(DIGIT_CLASSES_PATH, "w", encoding="utf-8") as f:
        json.dump(CLASSES, f, ensure_ascii=False, indent=2)
    print(f"已保存模型：{DIGIT_MODEL_PATH}")
    print(f"已保存类别映射：{DIGIT_CLASSES_PATH} -> {CLASSES}")

    if args.export_onnx:
        export_digit_onnx(model, args.export_onnx)


# =========================================================================== #
# 任务 2：购买按钮检测（button）
# =========================================================================== #
# 游戏交易行界面中「购买」按钮位置不固定（随窗口尺寸/布局/滚动变化），无法用硬编码
# 坐标命中。本任务训练一个轻量 CenterNet 风格单目标检测器：输入整幅交易行截图，
# 直接回归按钮的归一化边界框 (x_center, y_center, w, h)。

BUTTON_DATASET_DIR = os.path.join(HERE, "button_dataset")
BUTTON_MODEL_PT = "buttons.pt"          # 仅文件名，与 --out-dir 拼接决定落盘位置
BUTTON_DEFAULT_ONNX = "buttons.onnx"

# —— 模型超参（与 Rust 端 ORT 推理必须保持一致）——
BUTTON_INPUT_SIZE = 256  # 输入边长（正方形，像素）
BUTTON_HM = 32           # 热力图/尺寸图边长（= INPUT_SIZE / 8，由 3 次 maxpool 决定）
BUTTON_IN_CH = 3         # 输入通道（RGB）

# 三类按钮（输出通道顺序固定，推理端按此解释）。
BUTTON_CLASSES = ["buy", "minus", "plus"]
NUM_BUTTON_CLASSES = len(BUTTON_CLASSES)

# 交易行购买界面两种固定布局（原图像素坐标 [x, y, w, h]）。
# 检测器先定位「购买」按钮，据其 bbox 判定所处布局，再输出三类目标的类别标签与坐标；
# minus/plus 模型漏检时回退到该布局的固定模板坐标（保证输出完整、精确）。
LAYOUT_CASE_1 = {
    "buy":   [1482, 829, 297, 61],
    "minus": [1461, 754, 38, 42],
    "plus":  [1764, 754, 38, 42],
}
LAYOUT_CASE_2 = {
    "buy":   [1484, 887, 293, 59],
    "minus": [1461, 815, 38, 42],
    "plus":  [1764, 815, 38, 42],
}
LAYOUTS = [LAYOUT_CASE_1, LAYOUT_CASE_2]


def determine_case(bbox):
    """按「购买」按钮 bbox 判定所处布局（返回 0/1）；无法匹配（含 None）则返回 None。

    比较中心点距离与宽高相对误差，二者均足够接近某一布局的「购买」坐标时判定成功。
    """
    if not bbox:
        return None
    x, y, w, h = bbox
    cx, cy = x + w / 2.0, y + h / 2.0
    best, best_score = 0, -1e9
    for i, ly in enumerate(LAYOUTS):
        bx, by, bw, bh = ly["buy"]
        bcx, bcy = bx + bw / 2.0, by + bh / 2.0
        dist = ((cx - bcx) ** 2 + (cy - bcy) ** 2) ** 0.5
        sr = (abs(w - bw) / bw + abs(h - bh) / bh) / 2.0
        score = -dist / 30.0 - sr * 5.0
        if score > best_score:
            best_score, best = score, i
    bx, by, bw, bh = LAYOUTS[best]["buy"]
    dist = ((cx - (bx + bw / 2.0)) ** 2 + (cy - (by + bh / 2.0)) ** 2) ** 0.5
    sr = (abs(w - bw) / bw + abs(h - bh) / bh) / 2.0
    if dist < 40 and sr < 0.15:
        return best
    return None


# 训练超参默认值
BUTTON_DEFAULT_EPOCHS = 30
BUTTON_DEFAULT_BATCH = 16
BUTTON_DEFAULT_LR = 1e-3
LAMBDA_HM = 1.0          # 热力图损失权重
LAMBDA_SIZE = 0.1        # 尺寸损失权重
HM_SIGMA = 1.5           # 目标热力图高斯半径（单位：网格格）


class BuyButtonDetector(nn.Module):
    """轻量三分类检测器：下采样骨干 + 多类热力图头 + 多类尺寸头。

    输出：
        hm： [B, NUM_BUTTON_CLASSES, HM, HM]  各类别未归一化热力图 logits
        sz： [B, 2*NUM_BUTTON_CLASSES, HM, HM]  各类别 (w_norm, h_norm) ∈ [0,1]
            通道排布：类 c 的 w/h 位于 [2c, 2c+1]。
    """

    def __init__(self, in_ch: int = BUTTON_IN_CH, hm: int = BUTTON_HM):
        super().__init__()
        self.stem = nn.Sequential(
            nn.Conv2d(in_ch, 16, 3, padding=1), nn.ReLU(inplace=True), nn.MaxPool2d(2),
            nn.Conv2d(16, 32, 3, padding=1), nn.ReLU(inplace=True), nn.MaxPool2d(2),
            nn.Conv2d(32, 64, 3, padding=1), nn.ReLU(inplace=True), nn.MaxPool2d(2),
        )
        self.hm_head = nn.Conv2d(64, NUM_BUTTON_CLASSES, 1)       # 各类别 logits
        self.size_head = nn.Conv2d(64, 2 * NUM_BUTTON_CLASSES, 1) # 各类别 (w,h)

    def forward(self, x):
        f = self.stem(x)
        hm = self.hm_head(f)                    # [B,C,HM,HM]
        sz = torch.sigmoid(self.size_head(f))   # [B,2C,HM,HM]
        return hm, sz


def focal_heatmap_loss(pred_logits, target, alpha: float = 2.0, beta: float = 4.0):
    """CenterNet 风格 focal loss（pred_logits 为未归一化 logits，target ∈ [0,1]）。"""
    pred = torch.sigmoid(pred_logits)
    pos_mask = target > 0
    neg_mask = ~pos_mask
    pos_loss = -(1.0 - pred) ** alpha * torch.log(pred.clamp_min(1e-6)) * target
    neg_loss = -(pred ** alpha) * torch.log((1.0 - pred).clamp_min(1e-6)) * ((1.0 - target) ** beta)
    loss = pos_loss.sum() + neg_loss.sum()
    denom = pos_mask.sum().clamp_min(1).float()
    return loss / denom


def masked_size_loss(pred, target, hm_target):
    """仅在 heatmap 目标 > 0 的格上计算 L1（pred/target: [B,2,HM,HM]）。"""
    loss = F.l1_loss(pred, target, reduction="none")  # [B,2,HM,HM]
    w = hm_target.expand_as(loss)
    return (loss * w).sum() / w.sum().clamp_min(1.0)


def build_target(bbox, img_w: int, img_h: int):
    """由原图像素 bbox 构造 (heatmap [HM,HM], size [2,HM,HM]) 训练目标。

    bbox=None 表示负样本：返回全 0 目标。
    """
    heat = torch.zeros(BUTTON_HM, BUTTON_HM, dtype=torch.float32)
    size = torch.zeros(2, BUTTON_HM, BUTTON_HM, dtype=torch.float32)
    if bbox is None:
        return heat, size

    x, y, w, h = bbox
    # 归一化中心与尺寸（落在 [0,1]）。
    cx = (x + w / 2.0) / max(img_w, 1)
    cy = (y + h / 2.0) / max(img_h, 1)
    wn = w / max(img_w, 1)
    hn = h / max(img_h, 1)
    # 映射到 32×32 网格坐标。
    gx = cx * BUTTON_HM
    gy = cy * BUTTON_HM
    # 高斯目标热力图。
    for j in range(BUTTON_HM):
        for i in range(BUTTON_HM):
            d2 = (i + 0.5 - gx) ** 2 + (j + 0.5 - gy) ** 2
            heat[j, i] = math.exp(-d2 / (2.0 * HM_SIGMA * HM_SIGMA))
    # 尺寸目标广播到全图（仅正样本格参与 loss）。
    size[0].fill_(wn)
    size[1].fill_(hn)
    return heat, size


def build_targets(targets, img_w: int, img_h: int):
    """由三类目标 bbox 字典构造多类训练目标。

    targets: {"buy": [x,y,w,h]|None, "minus": ..., "plus": ...}
    返回 (heat [C,HM,HM], size [2C,HM,HM], has [C])。
    """
    heat = torch.zeros(NUM_BUTTON_CLASSES, BUTTON_HM, BUTTON_HM, dtype=torch.float32)
    size = torch.zeros(2 * NUM_BUTTON_CLASSES, BUTTON_HM, BUTTON_HM, dtype=torch.float32)
    has = torch.zeros(NUM_BUTTON_CLASSES, dtype=torch.float32)
    for c, label in enumerate(BUTTON_CLASSES):
        bbox = targets.get(label)
        if not bbox:
            continue
        h, s = build_target(bbox, img_w, img_h)
        heat[c] = h
        size[2 * c] = s[0]
        size[2 * c + 1] = s[1]
        has[c] = 1.0
    return heat, size, has


def _normalize_sample(obj):
    """兼容旧版单 bbox 标注，统一为 targets 字典 {buy,minus,plus}（值为 bbox 或 None）。"""
    if "targets" in obj:
        t = obj["targets"]
        return {label: t.get(label) for label in BUTTON_CLASSES}
    # 旧格式：{ "image": ..., "bbox": [...] | null } 仅含购买按钮。
    return {"buy": obj.get("bbox"), "minus": None, "plus": None}


class BuyButtonDataset(Dataset):
    """读取 JSONL 标注（三类目标），做基础在线增强后产出「图像张量 + 多类目标」。

    数据集格式（JSONL，每行一条）：
        {"image": "images/shot_001.png",
         "targets": {"buy": [x,y,w,h], "minus": [x,y,w,h], "plus": [x,y,w,h]}}
    任一目标可为 null（负样本 / 该帧无此按钮）。
    旧版 {"image":..., "bbox":...} 格式亦兼容（仅含购买按钮）。
    """

    def __init__(self, data_root: str, annotations: str = "annotations.jsonl", augment: bool = True):
        self.root = data_root
        self.augment = augment
        path = os.path.join(data_root, annotations)
        if not os.path.isfile(path):
            raise SystemExit(f"未找到标注文件：{path}")
        self.samples = []
        with open(path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                obj = json.loads(line)
                if "image" not in obj:
                    continue
                self.samples.append({"image": obj["image"], "targets": _normalize_sample(obj)})

    def __len__(self):
        return len(self.samples)

    def _load_and_augment(self, obj):
        img_path = os.path.join(self.root, obj["image"])
        img = Image.open(img_path).convert("RGB")
        targets = dict(obj["targets"])
        if self.augment:
            # 颜色抖动（亮度/对比度）：不改变 bbox。
            if random.random() < 0.5:
                img = ImageEnhance.Brightness(img).enhance(random.uniform(0.85, 1.15))
            if random.random() < 0.5:
                img = ImageEnhance.Contrast(img).enhance(random.uniform(0.85, 1.15))
            # 随机水平翻转：所有目标的 x 需镜像（x' = W - x - w）。
            if random.random() < 0.5:
                img = img.transpose(Image.FLIP_LEFT_RIGHT)
                for k, bbox in targets.items():
                    if bbox is None:
                        continue
                    x, y, w, h = bbox
                    targets[k] = [img.width - x - w, y, w, h]
        return img, targets

    def __getitem__(self, idx):
        obj = self.samples[idx]
        img, targets = self._load_and_augment(obj)
        w, h = img.size
        img = img.resize((BUTTON_INPUT_SIZE, BUTTON_INPUT_SIZE), Image.BILINEAR)
        # 缩放到 256×256 后，归一化到 [0,1]（与 Rust 端一致：仅 /255，无均值减法）。
        tensor = torch.from_numpy(
            (np.asarray(img, dtype=np.float32) / 255.0).transpose(2, 0, 1)
        ).contiguous()
        heat, size, has = build_targets(targets, w, h)
        return tensor, heat, size, has


def export_button_onnx(model, out_path: str):
    """导出三分类检测器 ONNX。

    输出张量（与 Rust 端 ORT 推理必须一致）：
        heatmap_logits: [1, NUM_BUTTON_CLASSES, HM, HM]  各类别 logits（推理时 sigmoid）
        size:           [1, 2*NUM_BUTTON_CLASSES, HM, HM]  各类别 (w_norm, h_norm) ∈ [0,1]
    """
    model.eval()
    dummy = torch.randn(1, BUTTON_IN_CH, BUTTON_INPUT_SIZE, BUTTON_INPUT_SIZE)
    torch.onnx.export(
        model,
        dummy,
        out_path,
        input_names=["input"],
        output_names=["heatmap_logits", "size"],
        dynamic_axes={
            "input": {0: "batch"},
            "heatmap_logits": {0: "batch"},
            "size": {0: "batch"},
        },
        dynamo=False,
    )
    # 用 onnx checker 校验图结构（无需 onnxruntime 即可验证导出合法）。
    import onnx as _onnx
    _onnx.checker.check_model(_onnx.load(out_path))
    print(f"已导出 ONNX（已通过 checker 校验）：{out_path}")


def train_button(args) -> None:
    model_pt = os.path.join(args.out_dir, BUTTON_MODEL_PT)
    onnx_path = os.path.join(args.out_dir, args.export_onnx or BUTTON_DEFAULT_ONNX)

    # —— 仅导出模式 ——
    if args.only_export:
        if not os.path.isfile(model_pt):
            raise SystemExit(f"未找到 {model_pt}，请先训练或去掉 --only-export。")
        model = BuyButtonDetector()
        model.load_state_dict(torch.load(model_pt, map_location="cpu"))
        export_button_onnx(model, onnx_path)
        return

    ds = BuyButtonDataset(args.data, augment=not args.no_augment)
    print(f"样本总数：{len(ds)}")
    loader = DataLoader(ds, batch_size=args.batch_size, shuffle=True, num_workers=0, drop_last=False)

    model = BuyButtonDetector()
    optimizer = torch.optim.Adam(model.parameters(), lr=args.lr)

    model.train()
    for epoch in range(1, args.epochs + 1):
        loss_sum, hm_sum, sz_sum, n = 0.0, 0.0, 0.0, 0
        for xb, hm_t, sz_t, has_t in loader:
            optimizer.zero_grad()
            hm_p, sz_p = model(xb)
            loss_hm = focal_heatmap_loss(hm_p, hm_t)
            # 尺寸 loss 只在有正样本（has_t 含 1）的图像上计入，避免负样本污染。
            if has_t.sum() > 0:
                # presence: 该图像是否含任意正样本（[B,1,1,1]），用于丢弃全负样本的尺寸 loss。
                presence = (has_t.sum(dim=1) > 0).to(sz_p.dtype).view(-1, 1, 1, 1)
                # 每类热力图广播到其 (w,h) 两个尺寸通道：[B,6,HM,HM]。
                sz_w = hm_t.repeat_interleave(2, dim=1) * presence
                loss_sz = masked_size_loss(sz_p * presence, sz_t * presence, sz_w)
            else:
                loss_sz = torch.zeros((), device=hm_p.device)
            loss = LAMBDA_HM * loss_hm + LAMBDA_SIZE * loss_sz
            loss.backward()
            optimizer.step()
            loss_sum += loss.item() * xb.size(0)
            hm_sum += loss_hm.item() * xb.size(0)
            sz_sum += loss_sz.item() * xb.size(0)
            n += xb.size(0)
        print(
            f"epoch {epoch:03d}  loss={loss_sum / n:.4f}  "
            f"hm={hm_sum / n:.4f}  size={sz_sum / n:.4f}"
        )

    torch.save(model.state_dict(), model_pt)
    print(f"已保存模型：{model_pt}")
    export_button_onnx(model, onnx_path)


# =========================================================================== #
# CLI
# =========================================================================== #
def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="train.py",
        description="三角洲行动交易行神经网络训练（数字分类 + 购买按钮检测）",
    )
    sub = parser.add_subparsers(dest="task")

    # —— digit 子命令 ——
    p_digit = sub.add_parser("digit", help="价格数字分类训练（默认任务）")
    p_digit.add_argument("--epochs", type=int, default=20)
    p_digit.add_argument("--batch-size", type=int, default=64)
    p_digit.add_argument("--lr", type=float, default=1e-3)
    p_digit.add_argument(
        "--export-onnx",
        type=str,
        default="",
        help="非空则训练后导出 ONNX 到该路径（供 Rust 端 ORT 推理）",
    )
    p_digit.add_argument(
        "--only-export",
        action="store_true",
        help="仅导出 ONNX（跳过训练，需已存在 model.pt）",
    )

    # —— button 子命令 ——
    p_button = sub.add_parser("button", help="购买按钮目标检测训练")
    p_button.add_argument("--data", default=BUTTON_DATASET_DIR, help="数据集根目录（含 annotations.jsonl 与 images/）")
    p_button.add_argument("--out-dir", default=HERE, help="模型输出目录（默认当前目录）")
    p_button.add_argument("--epochs", type=int, default=BUTTON_DEFAULT_EPOCHS)
    p_button.add_argument("--batch-size", type=int, default=BUTTON_DEFAULT_BATCH)
    p_button.add_argument("--lr", type=float, default=BUTTON_DEFAULT_LR)
    p_button.add_argument("--no-augment", action="store_true", help="关闭在线数据增强")
    p_button.add_argument("--export-onnx", default=BUTTON_DEFAULT_ONNX, help="训练完成后导出的 ONNX 文件名（相对 --out-dir）")
    p_button.add_argument("--only-export", action="store_true", help="仅导出 ONNX（需已存在 buttons.pt）")

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    task = args.task or "digit"  # 无子命令时默认 digit，兼容旧调用 `python train.py`
    if task == "digit":
        train_digit(args)
    else:
        train_button(args)


if __name__ == "__main__":
    main()


# python train.py digit --epochs 20 --export-onnx model.onnx
# python train.py button --data button_dataset --epochs 30 --batch-size 16 --lr 1e-3
#   -> 三分类（buy/minus/plus）检测器，输出 buttons.pt + buttons.onnx
#   -> 推理示例见 predict_layout.py：python predict_layout.py shot.png --model buttons.onnx
