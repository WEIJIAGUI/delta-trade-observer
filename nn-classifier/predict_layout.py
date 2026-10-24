#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""交易行按钮布局检测推理（购买 / 减号 / 加号三按钮，含布局判定）。

流程：
  1. 用三分类 CenterNet 检测器（buttons.onnx / buttons.pt）定位 buy / minus / plus 三按钮。
  2. 以「购买」按钮 bbox 为锚点，调用 determine_case 判定所处布局（①/②）。
  3. 对三类目标：模型置信度达标则采用模型结果，否则回退到该布局的固定模板坐标，
     保证即便小按钮（减/加）漏检也能输出完整、精确的三类 bbox。

坐标映射（与 train.py 的 BUTTON_INPUT_SIZE / BUTTON_HM 必须一致）：
  模型在 32×32 网格上输出，输入 256×256；训练时对原图做拉伸缩放，还原需乘原图宽高比例。

用法
----
    python predict_layout.py path/to/shot.png
    python predict_layout.py path/to/shot.png --model buttons.onnx --thresh 0.3
    python predict_layout.py path/to/shot.png --draw out.png     # 输出带三类框标注图
"""

import argparse
import json
import math
import os

import numpy as np
import torch
from PIL import Image, ImageDraw

from train import (
    BUTTON_CLASSES,
    BUTTON_INPUT_SIZE,
    BUTTON_HM,
    LAYOUTS,
    determine_case,
    LAYOUT_INPUT_SIZE,
    layout_preprocess,
)

HERE = os.path.dirname(os.path.abspath(__file__))

# 三类目标绘制配色：购买=绿，减号=橙，加号=蓝。
COLORS = {"buy": (46, 204, 64), "minus": (230, 126, 34), "plus": (52, 152, 219)}
THRESH = 0.3


def preprocess(img: Image.Image):
    img = img.convert("RGB").resize((BUTTON_INPUT_SIZE, BUTTON_INPUT_SIZE), Image.BILINEAR)
    arr = np.asarray(img, dtype=np.float32) / 255.0
    tensor = arr.transpose(2, 0, 1)[None, ...].astype(np.float32)
    return tensor, img.size  # 返回原图尺寸（用于坐标还原）


def decode_class(hm_ch, sz_ch, orig_w, orig_h, thresh=THRESH):
    """单类解码：hm_ch [HM,HM]，sz_ch [2,HM,HM] → (bbox|None, conf)。

    conf 为热力图峰值经 sigmoid 后的置信度（∈ [0,1]）；低于 thresh 视为未检出。
    """
    peak_raw = float(hm_ch.max())
    conf = 1.0 / (1.0 + math.exp(-peak_raw))
    if conf < thresh:
        return None, conf
    gy, gx = np.unravel_index(int(hm_ch.argmax()), hm_ch.shape)
    wn = float(sz_ch[0, gy, gx])
    hn = float(sz_ch[1, gy, gx])
    # 模型空间像素中心 / 尺寸。
    cx_m = (gx + 0.5) / BUTTON_HM * BUTTON_INPUT_SIZE
    cy_m = (gy + 0.5) / BUTTON_HM * BUTTON_INPUT_SIZE
    w_m = wn * BUTTON_INPUT_SIZE
    h_m = hn * BUTTON_INPUT_SIZE
    # 还原到原图（拉伸缩放逆运算）。
    sx, sy = orig_w / BUTTON_INPUT_SIZE, orig_h / BUTTON_INPUT_SIZE
    x = (cx_m - w_m / 2) * sx
    y = (cy_m - h_m / 2) * sy
    w = w_m * sx
    h = h_m * sy
    x = max(0.0, min(x, orig_w - 1))
    y = max(0.0, min(y, orig_h - 1))
    w = max(1.0, min(w, orig_w - x))
    h = max(1.0, min(h, orig_h - y))
    return [float(x), float(y), float(w), float(h)], conf


# 减/加按钮布局固定：当模型预测与其所属布局模板中心偏差超过该阈值（像素）时，
# 视为不可信，回退到模板坐标。既保留「模型检测」能力，又避免误检覆盖精确模板。
CASE_TOL_PX = 100.0


def detect_layout(hm, sz, orig_w, orig_h, thresh=THRESH):
    """输入模型原始输出（hm [1,C,HM,HM] / sz [1,2C,HM,HM]），返回 (targets, case)。

    targets: {label: {"bbox":[x,y,w,h]|None, "score":float|None, "from_case":bool?}}
    case: 判定布局 0/1，无法判定为 None。

    逻辑：
      1. 三类均由模型独立解码（buy / minus / plus）。
      2. 以「购买」bbox 为锚点判定布局（①/②）。
      3. 减/加按钮布局固定：模型检出且与模板中心偏差 ≤ 阈值 → 采用模型结果；
         否则回退到该布局的固定模板坐标（score 置 null 表示来自模板而非检测），
         保证输出完整、精确，且不会因小幅误检而偏离已知布局。
    """
    hm = hm[0]
    sz = sz[0]
    dets = {}
    for c, label in enumerate(BUTTON_CLASSES):
        hm_ch = hm[c]
        sz_ch = sz[2 * c: 2 * c + 2]
        bbox, score = decode_class(hm_ch, sz_ch, orig_w, orig_h, thresh)
        dets[label] = {"bbox": bbox, "score": score}

    case = determine_case(dets["buy"]["bbox"])
    if case is None:
        return dets, None

    for label in ("minus", "plus"):
        mb = dets[label]["bbox"]
        if mb is not None:
            tx, ty, tw, th = LAYOUTS[case][label]
            tcx, tcy = tx + tw / 2.0, ty + th / 2.0
            mcx, mcy = mb[0] + mb[2] / 2.0, mb[1] + mb[3] / 2.0
            dist = ((mcx - tcx) ** 2 + (mcy - tcy) ** 2) ** 0.5
            if dist > CASE_TOL_PX:
                mb = None  # 偏离模板，视为不可信
        if mb is None:
            dets[label]["bbox"] = list(LAYOUTS[case][label])
            dets[label]["from_case"] = True
            dets[label]["score"] = None
    return dets, case


def run_onnx(model_path, tensor):
    import onnxruntime as ort
    sess = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])
    hm, sz = sess.run(None, {"input": tensor})
    return hm, sz


def run_torch(model_path, tensor):
    from train import BuyButtonDetector
    model = BuyButtonDetector()
    model.load_state_dict(torch.load(model_path, map_location="cpu"))
    model.eval()
    with torch.no_grad():
        hm, sz = model(torch.from_numpy(tensor))
    return hm.numpy(), sz.numpy()


def predict_layout_case(img: Image.Image, model_path: str):
    """方案 A：布局二分类推理（模型输出 logits [2]），返回 0/1。"""
    tensor = layout_preprocess(img)
    if model_path.endswith(".onnx"):
        import onnxruntime as ort

        sess = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])
        logits = sess.run(None, {"input": tensor})[0][0]
    else:
        from train import LayoutClassifier

        model = LayoutClassifier()
        model.load_state_dict(torch.load(model_path, map_location="cpu"))
        model.eval()
        with torch.no_grad():
            logits = model(torch.from_numpy(tensor))[0].numpy()
    return int(np.argmax(logits))


def _draw(img: Image.Image, dets, out_path: str):
    d = ImageDraw.Draw(img)
    try:
        from PIL import ImageFont
        font = ImageFont.load_default()
    except Exception:
        font = None
    for label, info in dets.items():
        bbox = info.get("bbox")
        if not bbox:
            continue
        color = COLORS.get(label, (255, 255, 0))
        x, y, w, h = bbox
        d.rectangle([x, y, x + w, y + h], outline=color, width=3)
        score = info.get("score")
        tag = label + (f" {score:.2f}" if score is not None else " (layout)")
        ty = max(0, y - 14)
        d.text((x, ty), tag, fill=color, font=font)
    img.save(out_path)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("image", help="待检测截图路径")
    ap.add_argument("--model", default=os.path.join(HERE, "buttons.onnx"), help="模型文件（.onnx 或 .pt）")
    ap.add_argument("--thresh", type=float, default=THRESH, help="热力图峰值阈值（低于则判为无该按钮）")
    ap.add_argument("--draw", default="", help="可选：把检测框画到该路径输出 PNG")
    ap.add_argument(
        "--layout",
        action="store_true",
        help="方案A布局二分类模式（模型输出 logits，坐标纯查表）",
    )
    args = ap.parse_args()

    if not os.path.isfile(args.image):
        raise SystemExit(f"未找到图片：{args.image}")
    img = Image.open(args.image)

    if args.layout:
        case = predict_layout_case(img, args.model)
        result = {
            "image": args.image,
            "case": case,
            "targets": {k: list(v) for k, v in LAYOUTS[case].items()},
        }
        print(json.dumps(result, ensure_ascii=False))
        if args.draw:
            dets = {k: {"bbox": list(v)} for k, v in LAYOUTS[case].items()}
            _draw(img, dets, args.draw)
        return

    orig_w, orig_h = img.size
    tensor, _ = preprocess(img)

    if args.model.endswith(".onnx"):
        try:
            hm, sz = run_onnx(args.model, tensor)
        except ImportError:
            print("[warn] onnxruntime 未安装，回退到 PyTorch（buttons.pt）。")
            hm, sz = run_torch(args.model.replace(".onnx", ".pt"), tensor)
    else:
        hm, sz = run_torch(args.model, tensor)

    dets, case = detect_layout(hm, sz, orig_w, orig_h, thresh=args.thresh)
    result = {
        "image": args.image,
        "case": case,  # 0/1/None
        "targets": dets,
    }
    print(json.dumps(result, ensure_ascii=False))

    if args.draw:
        _draw(img, dets, args.draw)
        print(f"已保存标注图：{args.draw}")


if __name__ == "__main__":
    main()
