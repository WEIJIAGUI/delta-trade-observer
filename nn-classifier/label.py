#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""交易行按钮布局数据标注辅助（零额外依赖，仅需标准库）。

按类别逐条录入「购买 / 减号 / 加号」按钮的边界框，写入 button_dataset/annotations.jsonl。

用法：
    python label.py <图片相对路径> <类别> [x y w h | null]
        类别 ∈ {buy, minus, plus}
        省略第三参数：交互输入 x y w h（空格分隔，原图像素坐标）
        第三参数为 null / none / neg：该类别标为负（本帧无此按钮）

示例：
    python label.py images/shot_0001.png buy 1482 829 297 61
    python label.py images/shot_0001.png minus 1461 754 38 42
    python label.py images/shot_0001.png plus 1764 754 38 42
    python label.py images/shot_0001.png buy null

图片应放在 button_dataset/images/ 下；标注写入 button_dataset/annotations.jsonl（targets 格式）。
"""
import json
import os
import sys

from train import BUTTON_CLASSES

HERE = os.path.dirname(os.path.abspath(__file__))
ANN_PATH = os.path.join(HERE, "button_dataset", "annotations.jsonl")


def _read_all():
    data = {}
    if os.path.isfile(ANN_PATH):
        with open(ANN_PATH, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                obj = json.loads(line)
                if "image" not in obj:
                    continue
                t = obj.get("targets") or {}
                data[obj["image"]] = {l: t.get(l) for l in BUTTON_CLASSES}
    return data


def _write_all(data):
    with open(ANN_PATH, "w", encoding="utf-8") as f:
        for img, targets in data.items():
            f.write(json.dumps(
                {"image": img, "targets": {l: targets.get(l) for l in BUTTON_CLASSES}},
                ensure_ascii=False,
            ) + "\n")


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return
    img_rel = sys.argv[1]
    cls = sys.argv[2].lower()
    if cls not in BUTTON_CLASSES:
        print(f"[取消] 类别必须是 {BUTTON_CLASSES} 之一，收到：{cls}")
        return
    if not os.path.isfile(os.path.join(HERE, "button_dataset", img_rel)):
        print(f"[跳过] 找不到图片：button_dataset/{img_rel}")
        return

    neg = len(sys.argv) >= 4 and sys.argv[3].lower() in ("null", "none", "neg")
    if neg:
        bbox = None
        print(f"{cls}：负样本（null）")
    else:
        if len(sys.argv) >= 6:
            try:
                x, y, w, h = (int(p) for p in sys.argv[3:7])
            except ValueError:
                print("[取消] 坐标必须为 4 个整数")
                return
        else:
            raw = input(f"请输入 {cls} 边界框 x y w h（空格分隔，原图像素坐标）：").strip()
            parts = raw.split()
            if len(parts) != 4:
                print("[取消] 需要 4 个整数")
                return
            try:
                x, y, w, h = (int(p) for p in parts)
            except ValueError:
                print("[取消] 坐标必须为整数")
                return
        bbox = [x, y, w, h]
        print(f"{cls}：{bbox}")

    data = _read_all()
    if img_rel not in data:
        data[img_rel] = {l: None for l in BUTTON_CLASSES}
    data[img_rel][cls] = bbox
    _write_all(data)
    print(f"已更新 {ANN_PATH}（图片 {img_rel}）")


if __name__ == "__main__":
    main()
