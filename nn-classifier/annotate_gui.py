#!/usr/bin/env python3
"""交易行按钮布局标注工具（tkinter，零额外依赖，仅需 Pillow）。

为三分类「购买 / 减号 / 加号」按钮标注：支持鼠标拖拽框选、按类别标注、两种固定布局一键填充，
并把结果导出为 annotations.jsonl（统一 targets 格式，与 train.py 完全一致）。

数据集格式（annotations.jsonl，每行一条 JSON）：
    {"image": "images/shot_001.png",
     "targets": {"buy": [x,y,w,h], "minus": [x,y,w,h], "plus": [x,y,w,h]}}
    任一目标可为 null（该帧无此按钮 / 负样本）。

操作
----
    - 类别按钮（buy / minus / plus）：切换当前框选类别（高亮显示）。
    - 在画布上按住左键拖拽：为「当前类别」画一个框。
    - 布局① / 布局②：一键按固定坐标填充全部三个按钮（快速标注）。
    - ← / →：上一张 / 下一张（切换时自动保存）。
    - N：整图标为负样本（三类全 null）   D：删除本图标注
    - 数值键 1 / 2：直接套用布局① / 布局②

运行
----
    cd nn-classifier
    python annotate_gui.py
"""
import glob
import json
import os

import tkinter as tk
from PIL import Image, ImageTk

from train import BUTTON_CLASSES, LAYOUTS

HERE = os.path.dirname(os.path.abspath(__file__))
IMAGE_DIR = os.path.join(HERE, "button_dataset", "images")
ANN_PATH = os.path.join(HERE, "button_dataset", "annotations.jsonl")
IMG_EXTS = ("*.png", "*.jpg", "*.jpeg", "*.bmp")
CANVAS_W, CANVAS_H = 1040, 660

# 三类目标绘制配色（与 predict_layout.py 保持一致）。
COLORS = {"buy": "#2ecc40", "minus": "#e67e22", "plus": "#3498db"}


class Annotator:
    def __init__(self, root: tk.Tk):
        self.root = root
        self.root.title("交易行按钮布局标注（拖拽框选 / 类别切换 / 布局快填）")
        self.images = []
        self.idx = 0
        # rel_path -> {"buy":[...]|None, "minus":..., "plus":...}
        self.ann = {}
        self.active = BUTTON_CLASSES[0]  # 当前框选类别
        self.scale = 1.0
        self.off_x = 0.0
        self.off_y = 0.0
        self.tk_img = None
        self.drag_start = None   # 画布坐标
        self.drag_cur = None

        self._build_ui()
        self._reload()

    # —— UI ——
    def _build_ui(self):
        top = tk.Frame(self.root)
        top.pack(fill=tk.X)
        tk.Button(top, text="上一张 (←)", command=self.prev).pack(side=tk.LEFT)
        tk.Button(top, text="下一张 (→)", command=self.next).pack(side=tk.LEFT)
        tk.Button(top, text="负样本 (N)", command=self.mark_neg).pack(side=tk.LEFT)
        tk.Button(top, text="删除标注 (D)", command=self.delete_ann).pack(side=tk.LEFT)

        # 类别切换
        self.cat_btns = {}
        cat_frame = tk.Frame(self.root)
        cat_frame.pack(fill=tk.X)
        tk.Label(cat_frame, text="框选类别：").pack(side=tk.LEFT)
        for label in BUTTON_CLASSES:
            b = tk.Button(
                cat_frame, text=label, bg=COLORS[label], fg="white",
                command=lambda l=label: self.set_active(l),
            )
            b.pack(side=tk.LEFT, fill=tk.X, expand=True)
            self.cat_btns[label] = b

        self.canvas = tk.Canvas(self.root, width=CANVAS_W, height=CANVAS_H, bg="#1e1e1e")
        self.canvas.pack(fill=tk.BOTH, expand=True)
        self.canvas.bind("<ButtonPress-1>", self.on_press)
        self.canvas.bind("<B1-Motion>", self.on_drag)
        self.canvas.bind("<ButtonRelease-1>", self.on_release)

        bottom = tk.Frame(self.root)
        bottom.pack(fill=tk.X)
        tk.Button(bottom, text="布局① 全填充", command=lambda: self.fill_layout(0),
                  bg="#2e7d32", fg="white").pack(side=tk.LEFT, fill=tk.X, expand=True)
        tk.Button(bottom, text="布局② 全填充", command=lambda: self.fill_layout(1),
                  bg="#1565c0", fg="white").pack(side=tk.LEFT, fill=tk.X, expand=True)

        self.status = tk.Label(self.root, text="", anchor=tk.W)
        self.status.pack(fill=tk.X)

        self.root.bind("<Left>", lambda e: self.prev())
        self.root.bind("<Right>", lambda e: self.next())
        self.root.bind("<n>", lambda e: self.mark_neg())
        self.root.bind("<d>", lambda e: self.delete_ann())
        self.root.bind("1", lambda e: self.fill_layout(0))
        self.root.bind("2", lambda e: self.fill_layout(1))
        self._refresh_cat_btns()

    def _refresh_cat_btns(self):
        for label, b in self.cat_btns.items():
            if label == self.active:
                b.config(relief=tk.SUNKEN, bd=4)
            else:
                b.config(relief=tk.RAISED, bd=1)

    # —— 数据加载 ——
    def _reload(self):
        if not os.path.isdir(IMAGE_DIR):
            tk.messagebox.showerror("错误", f"未找到目录：{IMAGE_DIR}\n请先把截图放进 button_dataset/images/")
            return
        files = []
        for ext in IMG_EXTS:
            files.extend(glob.glob(os.path.join(IMAGE_DIR, ext)))
        self.images = sorted(files)
        self.ann = {}
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
                    self.ann[obj["image"]] = {l: t.get(l) for l in BUTTON_CLASSES}
        self.idx = 0
        self.show()

    def _rel(self, path):
        return os.path.relpath(path, os.path.join(HERE, "button_dataset")).replace("\\", "/")

    # —— 显示 ——
    def show(self):
        self.canvas.delete("all")
        self.drag_start = self.drag_cur = None
        if not self.images:
            self.status.config(text="images/ 下没有图片，请先放入截图")
            return
        path = self.images[self.idx]
        img = Image.open(path).convert("RGB")
        iw, ih = img.size
        self.scale = min(CANVAS_W / iw, CANVAS_H / ih)
        self.off_x = (CANVAS_W - iw * self.scale) / 2
        self.off_y = (CANVAS_H - ih * self.scale) / 2
        disp = img.resize((int(iw * self.scale), int(ih * self.scale)), Image.BILINEAR)
        self.tk_img = ImageTk.PhotoImage(disp)
        self.canvas.create_image(self.off_x, self.off_y, image=self.tk_img, anchor=tk.NW)

        rel = self._rel(path)
        targets = self.ann.get(rel, {})
        for label in BUTTON_CLASSES:
            bbox = targets.get(label)
            if bbox:
                self._draw_box(*bbox, COLORS[label])

        total = len(self.images)
        done = sum(1 for v in self.ann.values() if any(v.get(l) for l in BUTTON_CLASSES))
        summary = "  ".join(
            f"{l}:{targets.get(l) if targets.get(l) else '—'}" for l in BUTTON_CLASSES
        )
        self.status.config(
            text=f"[{self.idx + 1}/{total}] {os.path.basename(path)}  原图 {iw}x{ih}  "
                 f"已标 {done}  | 当前类别: {self.active}\n{summary}"
        )

    def _draw_box(self, x, y, w, h, color):
        cx = self.off_x + x * self.scale
        cy = self.off_y + y * self.scale
        self.canvas.create_rectangle(cx, cy, cx + w * self.scale, cy + h * self.scale,
                                     outline=color, width=2)

    # —— 拖拽框选 ——
    def on_press(self, event):
        if not self.images:
            return
        self.drag_start = (event.x, event.y)
        self.drag_cur = (event.x, event.y)

    def on_drag(self, event):
        if self.drag_start is None:
            return
        self.drag_cur = (event.x, event.y)
        self._redraw_with_drag()

    def _redraw_with_drag(self):
        # 仅重画当前拖拽矩形（在原图之上）。
        self.show()
        if self.drag_start and self.drag_cur:
            x0, y0 = self.drag_start
            x1, y1 = self.drag_cur
            self.canvas.create_rectangle(
                min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1),
                outline="#ffffff", width=2, dash=(4, 2),
            )

    def on_release(self, event):
        if self.drag_start is None:
            return
        x0, y0 = self.drag_start
        x1, y1 = event.x, event.y
        self.drag_start = self.drag_cur = None
        # 过小的框视为误触，忽略。
        if abs(x1 - x0) < 4 or abs(y1 - y0) < 4:
            self.show()
            return
        # 画布坐标 -> 原图像素坐标。
        ox0 = (min(x0, x1) - self.off_x) / self.scale
        oy0 = (min(y0, y1) - self.off_y) / self.scale
        ow = abs(x1 - x0) / self.scale
        oh = abs(y1 - y0) / self.scale
        bbox = [round(ox0), round(oy0), round(ow), round(oh)]
        rel = self._rel(self.images[self.idx])
        if rel not in self.ann:
            self.ann[rel] = {l: None for l in BUTTON_CLASSES}
        self.ann[rel][self.active] = bbox
        self._save(True)
        self.show()

    # —— 标注操作 ——
    def set_active(self, label):
        self.active = label
        self._refresh_cat_btns()
        self.show()

    def fill_layout(self, case_idx: int):
        if not self.images:
            return
        rel = self._rel(self.images[self.idx])
        self.ann[rel] = {l: list(LAYOUTS[case_idx][l]) for l in BUTTON_CLASSES}
        self._save(True)
        self.show()

    def _save(self, quiet):
        with open(ANN_PATH, "w", encoding="utf-8") as f:
            for p in self.images:
                rel = self._rel(p)
                if rel in self.ann:
                    targets = self.ann[rel]
                    f.write(json.dumps(
                        {"image": rel, "targets": {l: targets.get(l) for l in BUTTON_CLASSES}},
                        ensure_ascii=False,
                    ) + "\n")
        if not quiet:
            self.status.config(text=self.status.cget("text") + "  [已保存]")

    def mark_neg(self):
        if not self.images:
            return
        rel = self._rel(self.images[self.idx])
        self.ann[rel] = {l: None for l in BUTTON_CLASSES}
        self._save(True)
        self.show()

    def delete_ann(self):
        if not self.images:
            return
        self.ann.pop(self._rel(self.images[self.idx]), None)
        self._save(True)
        self.show()

    def next(self):
        if not self.images:
            return
        self._save(True)
        self.idx = (self.idx + 1) % len(self.images)
        self.show()

    def prev(self):
        if not self.images:
            return
        self._save(True)
        self.idx = (self.idx - 1) % len(self.images)
        self.show()


def main():
    root = tk.Tk()
    root.geometry(f"{CANVAS_W}x{CANVAS_H + 110}")
    Annotator(root)
    root.mainloop()


if __name__ == "__main__":
    main()
