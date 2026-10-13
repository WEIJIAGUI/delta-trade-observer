#!/usr/bin/env python3
"""单张图片推理（快速验证训练结果，非 Rust 集成路径）。

用法
----
    python predict.py <image.png>
"""

import json
import os
import sys

import torch
from PIL import Image

from train import CLASSES, DigitCNN, DIGIT_TRANSFORM

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_PATH = os.path.join(HERE, "model.pt")
CLASSES_PATH = os.path.join(HERE, "classes.json")


def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit("用法：python predict.py <image.png>")
    img_path = sys.argv[1]

    if not os.path.isfile(MODEL_PATH):
        raise SystemExit(f"未找到模型：{MODEL_PATH}，请先运行 train.py")

    classes = (
        json.load(open(CLASSES_PATH, encoding="utf-8"))
        if os.path.isfile(CLASSES_PATH)
        else CLASSES
    )

    model = DigitCNN()
    model.load_state_dict(torch.load(MODEL_PATH, map_location="cpu"))
    model.eval()

    img = Image.open(img_path).convert("RGB")
    x = DIGIT_TRANSFORM(img).unsqueeze(0)
    with torch.no_grad():
        logits = model(x)[0]
    probs = torch.softmax(logits, dim=0)

    top = int(probs.argmax())
    label = classes[top]
    print(f"预测类别：{label}  (置信度 {probs[top]:.3f})")
    for i, p in enumerate(probs):
        print(f"  {classes[i]}: {p:.3f}")


if __name__ == "__main__":
    main()
