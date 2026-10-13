#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成「合成」数据集，用于冒烟测试三分类按钮检测的训练 / 导出 / 推理链路（非真实游戏画面）。

按真实交易行界面的两种固定布局（见 train.py 的 LAYOUTS）在 1920×1080 截图中精确放置
「购买 / 减号 / 加号」三按钮，并写出 targets 格式标注，使模型能学到三类目标与布局对应关系。

用法
----
    python make_dummy_dataset.py --num 200 --out button_dataset
"""

import argparse
import json
import os
import random

from PIL import Image, ImageDraw, ImageFont

from train import LAYOUTS

IMG_W, IMG_H = 1920, 1080          # 模拟交易行窗口分辨率


def _font(size=24):
    for name in ("msyh.ttc", "msyh.ttf", "arial.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except Exception:
            continue
    return ImageFont.load_default()


def make_one(path, idx):
    bg = (random.randint(20, 60), random.randint(20, 60), random.randint(30, 70))
    img = Image.new("RGB", (IMG_W, IMG_H), bg)
    d = ImageDraw.Draw(img)
    font = _font()

    # 干扰色块（不标注）。
    for _ in range(random.randint(3, 8)):
        x0 = random.randint(0, IMG_W - 200)
        y0 = random.randint(0, IMG_H - 120)
        d.rectangle(
            [x0, y0, x0 + random.randint(80, 200), y0 + random.randint(40, 120)],
            fill=(random.randint(40, 120), random.randint(40, 120), random.randint(40, 120)),
        )

    # 随机选一种固定布局，按精确坐标放置三按钮。
    case = random.randint(0, len(LAYOUTS) - 1)
    layout = LAYOUTS[case]
    targets = {}
    for label, (x, y, w, h) in layout.items():
        color = {"buy": (236, 140, 64), "minus": (120, 120, 130), "plus": (120, 120, 130)}[label]
        d.rectangle([x, y, x + w, y + h], fill=color)
        txt = {"buy": "购买", "minus": "−", "plus": "+"}[label]
        d.text((x + max(2, w // 2 - 12), y + max(2, h // 2 - 12)), txt, fill=(255, 255, 255), font=font)
        targets[label] = [x, y, w, h]

    # 约 10% 负样本（本帧无任何按钮）。
    if random.random() < 0.1:
        targets = {label: None for label in layout}

    img.save(path)
    return {
        "image": os.path.relpath(path, os.path.dirname(os.path.dirname(path))),
        "targets": targets,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--num", type=int, default=200)
    ap.add_argument("--out", default="button_dataset")
    args = ap.parse_args()

    img_dir = os.path.join(args.out, "images")
    os.makedirs(img_dir, exist_ok=True)
    ann_path = os.path.join(args.out, "annotations.jsonl")
    with open(ann_path, "w", encoding="utf-8") as f:
        for i in range(args.num):
            img_path = os.path.join(img_dir, f"shot_{i:04d}.png")
            obj = make_one(img_path, i)
            f.write(json.dumps(obj, ensure_ascii=False) + "\n")
    print(f"已生成 {args.num} 张合成样本 -> {ann_path}")


if __name__ == "__main__":
    main()
