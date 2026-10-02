#!/usr/bin/env python3
"""三角洲行动交易行 — 数字分类训练（类别：0-9 + x）。

数据来源
--------
Rust 端「开始观察」勾选「保存数字用于训练」后，每枚子弹的 4 个数位裁剪会按识别结果
自动落入 `dataset/<label>/`：
  - 被模板匹配识别为 0-9 的裁剪 -> 对应数字文件夹；
  - 未识别（含空槽、货币图标、千分位逗号等所有非数字字形）-> `x/` 文件夹。
用户再人工校正 / 重新分类这些裁剪图，得到干净的训练集。

类别顺序固定为 0..9 然后 x（见 `CLASSES`），训练后 `classes.json` 记录该映射，
供推理端（含未来 Rust 端 ONNX 集成）复现同一顺序。

用法
----
    pip install -r requirements.txt
    python train.py                          # 训练并保存 model.pt + classes.json
    python train.py --epochs 30 --batch-size 64 --lr 1e-3
    python train.py --export-onnx model.onnx # 导出 ONNX（供 Rust 端 ORT 推理，dynamo=False 走传统导出器，无需 onnxscript）

输入约定（与 Rust 端集成推理时必须保持一致）
------------------------------------------------
灰度 -> 32x32 -> [0,1] -> 归一化到 [-1,1]（(x - 0.5) / 0.5）。
"""

import argparse
import json
import os

import torch
import torch.nn as nn
from torch.utils.data import DataLoader
from torchvision import datasets, transforms

HERE = os.path.dirname(os.path.abspath(__file__))
DATASET_DIR = os.path.join(HERE, "dataset")
MODEL_PATH = os.path.join(HERE, "model.pt")
CLASSES_PATH = os.path.join(HERE, "classes.json")

# 固定类别顺序：0..9 然后 x（x = 非数字）。
CLASSES = [str(i) for i in range(10)] + ["x"]
CLASS_TO_IDX = {c: i for i, c in enumerate(CLASSES)}
NUM_CLASSES = len(CLASSES)

# 输入尺寸（与 Rust 端 ONNX 推理前处理必须一致）。
INPUT_SIZE = 32

# 预处理：灰度 -> 32x32 -> [0,1] -> 归一化到 [-1,1]。
TRANSFORM = transforms.Compose(
    [
        transforms.Grayscale(num_output_channels=1),
        transforms.Resize((INPUT_SIZE, INPUT_SIZE)),
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
            nn.Linear(32 * (INPUT_SIZE // 4) * (INPUT_SIZE // 4), 64),
            nn.ReLU(),
            nn.Linear(64, num_classes),
        )

    def forward(self, x):
        return self.classifier(self.features(x))


def build_loader(batch_size: int):
    # 移除 class_to_idx 参数
    ds = datasets.ImageFolder(DATASET_DIR, transform=TRANSFORM)
    # 手动将 ImageFolder 的默认类别索引映射为您定义的 CLASS_TO_IDX
    map_fn = {ds.class_to_idx[c]: CLASS_TO_IDX[c] for c in CLASSES}
    ds.targets = [map_fn[t] for t in ds.targets]
    
    loader = DataLoader(ds, batch_size=batch_size, shuffle=True)
    return ds, loader



def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--epochs", type=int, default=20)
    ap.add_argument("--batch-size", type=int, default=64)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument(
        "--export-onnx",
        type=str,
        default="",
        help="非空则训练后导出 ONNX 到该路径（供 Rust 端 ORT 推理，本回合未集成）",
    )
    ap.add_argument(
        "--only-export",
        action="store_true",
        help="仅导出 ONNX（跳过训练，需已存在 model.pt）；可与 --export-onnx 组合",
    )
    args = ap.parse_args()

    # 仅导出模式：复用已有 model.pt，避免每次导出都重新训练。
    if args.only_export:
        if not os.path.isfile(MODEL_PATH):
            raise SystemExit(
                f"未找到 {MODEL_PATH}，请先完成一次训练，或去掉 --only-export。"
            )
        model = DigitCNN()
        model.load_state_dict(torch.load(MODEL_PATH, map_location="cpu"))
        model.eval()
        out_path = args.export_onnx or "model.onnx"
        dummy = torch.randn(1, 1, INPUT_SIZE, INPUT_SIZE)
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
        return

    if not os.path.isdir(DATASET_DIR) or not any(
        os.listdir(DATASET_DIR)
    ):
        raise SystemExit(
            f"未找到非空数据集：{DATASET_DIR}\n"
            "请先在 Rust 端勾选「保存数字用于训练」采集数据，并人工校正标签。"
        )

    ds, loader = build_loader(args.batch_size)
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

    torch.save(model.state_dict(), MODEL_PATH)
    with open(CLASSES_PATH, "w", encoding="utf-8") as f:
        json.dump(CLASSES, f, ensure_ascii=False, indent=2)
    print(f"已保存模型：{MODEL_PATH}")
    print(f"已保存类别映射：{CLASSES_PATH} -> {CLASSES}")

    if args.export_onnx:
        dummy = torch.randn(1, 1, INPUT_SIZE, INPUT_SIZE)
        # 使用传统 TorchScript 导出器（dynamo=False）：不依赖 onnxscript，兼容性更好，
        # 且导出的 ONNX 与 ORT 推理端对齐稳定。
        torch.onnx.export(
            model,
            dummy,
            args.export_onnx,
            input_names=["input"],
            output_names=["logits"],
            dynamic_axes={"input": {0: "batch"}, "logits": {0: "batch"}},
            dynamo=False,
        )
        print(f"已导出 ONNX：{args.export_onnx}")


if __name__ == "__main__":
    main()
