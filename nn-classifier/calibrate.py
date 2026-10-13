#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""坐标校准工具：用一张「购买界面完整截图」确认数量数字区域的裁剪框是否正确。

用法：
    python calibrate.py <完整截图.png> [--scene exchangeable|non-exchangeable] [--out calib_out]

输出（保存到 --out 目录）：
    full_with_rect.png ：原图上用红框标出当前配置的裁剪区域，便于肉眼判断框是否落在数字上；
    crop.png           ：裁下来的区域（灰度），直接看里面是不是你要识别的数量数字。

若 crop.png 里不是数字（是背景/标签），说明 train_qty.py 顶部的
QTY_REGION_EXCHANGEABLE / QTY_REGION_NONEXCHANGEABLE 坐标不对，
请按实际数字位置修改后再重新采集与训练。
"""

import argparse
import os

from PIL import Image, ImageDraw

import train_qty as T


def main() -> None:
    p = argparse.ArgumentParser(description="数量区域坐标校准")
    p.add_argument("image", help="购买界面完整截图（png/jpg）")
    p.add_argument(
        "--scene",
        default="exchangeable",
        help="场景：exchangeable（可兑换） / non-exchangeable（不可兑换）",
    )
    p.add_argument("--out", default="calib_out", help="输出目录（默认 calib_out/）")
    args = p.parse_args()

    if not os.path.isfile(args.image):
        raise SystemExit(f"未找到截图：{args.image}")

    if args.scene in ("non", "non-exchangeable", "不可兑换"):
        region = T.QTY_REGION_NONEXCHANGEABLE
    else:
        region = T.QTY_REGION_EXCHANGEABLE
    x, y, w, h = region

    img = Image.open(args.image).convert("RGB")
    iw, ih = img.size
    print(f"截图尺寸：{iw}×{ih}；当前裁剪区域 [x={x}, y={y}, w={w}, h={h}]")

    if x + w > iw or y + h > ih:
        print("[警告] 裁剪区域超出截图边界，坐标可能整体偏移（注意 DPI/缩放/窗口位置）。")

    # 原图叠加红框
    drawn = img.copy()
    d = ImageDraw.Draw(drawn)
    d.rectangle([x, y, x + w, y + h], outline=(255, 0, 0), width=3)

    # 裁下的区域（灰度）
    crop = img.crop((x, y, x + w, y + h)).convert("L")

    os.makedirs(args.out, exist_ok=True)
    full_path = os.path.join(args.out, "full_with_rect.png")
    crop_path = os.path.join(args.out, "crop.png")
    drawn.save(full_path)
    crop.save(crop_path)
    print(f"已保存：{full_path}（红框为裁剪区）")
    print(f"已保存：{crop_path}（裁剪结果）")
    print("请目视 crop.png：若是数量数字 → 坐标正确；若是背景/标签 → 修改坐标后重跑本工具。")


if __name__ == "__main__":
    main()
