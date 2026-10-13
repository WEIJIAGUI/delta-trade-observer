# 神经网络训练工程（nn-classifier，合并工程）

> 本目录是「三角洲行动」交易行相关的 **神经网络训练统一工程**，承担三类模型的训练与导出。
> 目录名沿用历史 `nn-classifier`，即便现在已不止数字分类也暂不重命名。
>
> 三类功能通过**物理文件夹 + 独立脚本**相互隔离：
> - 数字分类 → `train.py digit` + 数据 `dataset/`（输出 `model.onnx`）
> - 按钮布局检测 → `train.py button` + 数据 `button_dataset/`（输出 `buttons.onnx`）
> - 子弹「所选数量」整块区域分类 → `train_qty.py` + 数据 `qty_dataset/`（输出 `qty.onnx`）

| 任务 | 训练脚本 / 子命令 | 模型 | 输出（默认） | 用途 |
|------|--------|------|--------------|------|
| 数字分类 | `train.py digit`（默认） | `DigitCNN`（11 类：0-9 + x） | `model.pt` + `classes.json`，可选 `model.onnx` | 替代模板匹配识别价格数字，供 Rust/ORT 推理 |
| 购买按钮布局检测 | `train.py button` | `BuyButtonDetector`（CenterNet 风格三分类：buy/minus/plus） | `buttons.pt` + `buttons.onnx` | 检测购买界面 buy/minus/plus 三按钮，据「购买」bbox 判定布局①/②，供 Rust/ORT 点击 |
| 子弹「所选数量」整块区域分类 | `train_qty.py`（collect / train / infer） | `QtyRegionCNN`（200 类：1..200） | `qty.pt` + `qty.onnx` | 对购买界面「所选数量」整块区域做整体分类，直接输出整数值 1..200，供 Rust/ORT 推理与数量调整 |

两类任务共享同一份 `requirements.txt` 与同一套 PyTorch 训练骨架，互不冲突。

## 目录结构

```
nn-classifier/
├── dataset/              # 数字分类训练数据：每个类别一个子文件夹（0/1/.../9/x）
├── button_dataset/       # 购买按钮检测数据：annotations.jsonl + images/（由 make_dummy_dataset.py 或人工标注生成）
├── qty_dataset/          # 数量整块区域分类训练数据：每个数量值一个子文件夹（1/2/.../200），由采集工具或 collect 生成
├── train.py              # 数字分类 + 按钮检测训练入口（digit / button 两个子命令）
├── train_qty.py          # 数量整块区域分类训练入口（collect / train / infer 三个子命令）—— 与 train.py 独立
├── predict.py            # 数字分类单图推理验证
├── predict_layout.py     # 按钮布局检测推理验证（三分类 + 布局判定）
├── annotate_gui.py       # 可视化标注工具（拖拽框选 / 类别切换 / 布局快填）
├── label.py              # 命令行按类别逐条标注
├── make_dummy_dataset.py # 生成按钮检测合成数据（冒烟测试用，非真实画面）
├── requirements.txt      # 共享依赖
├── model.pt / model.onnx / classes.json   # 数字分类产物（Rust 端使用）
├── buttons.pt / buttons.onnx              # 按钮布局检测产物
├── qty.pt / qty.onnx                      # 数量整块区域分类产物（Rust 端 qty_classifier 使用）
└── README.md
```

## 依赖（共享）

```bash
pip install -r requirements.txt
```

```
torch>=2.0
torchvision>=0.15
pillow
numpy
onnx>=1.13
# onnxruntime 可选：仅 predict_layout.py 走 ONNX 推理时需要；缺失时自动回退到 PyTorch。
# onnxruntime>=1.13
```

---

## 任务 1：数字分类（digit）

固定 11 类：`0 1 2 3 4 5 6 7 8 9 x`
- `0-9`：单个数字。
- `x`：**非数字**（空槽、货币图标、千分位逗号等所有非 0-9 字形）。

类别顺序由 `train.py` 的 `CLASSES` 固定，训练后写入 `classes.json`，推理端必须按此顺序解释模型输出。

### 数据采集（来自 Rust 端）

1. 在「开始观察」面板勾选 **「保存识别到的数字」**（默认关闭）；勾选后动态出现「数字输出目录」输入框，默认 `<结果保存目录>/digits`。
2. 启动观察后，每枚子弹的 4 个数位裁剪统一写入该目录，文件名形如 `d{序号:08}_{label}.png`；标签仅作人工校正参考。
3. **人工校正**：将裁剪图移动到 `dataset/{0,1,...,9,x}/` 正确类别文件夹，确保标签干净后再训练。

### 训练

```bash
python train.py                                  # 等同 python train.py digit
python train.py digit --epochs 30 --batch-size 64
python train.py digit --export-onnx model.onnx   # 额外导出 ONNX（供 Rust 端集成）
python train.py digit --only-export              # 仅导出 ONNX（需已存在 model.pt）
```

### 推理验证

```bash
python predict.py path/to/digit_crop.png
```

### 与 Rust 端集成（已实现）

- `python train.py digit --export-onnx model.onnx`（传统导出器 `dynamo=False`，无需 onnxscript）。
- Rust 端 `src/nn_classifier.rs` 惰性加载 `model.onnx` + `classes.json`，前处理与训练端一致
  （RGBA → 灰度 ITU-R 601 → 双线性 32×32 → `[0,1]` → `(x-0.5)/0.5`），取 11 类 softmax；
  `x` 类与低于 90% 置信度判「无数字」。
- `dist.bat` 打包时仅复制 `nn-classifier/model.onnx`（数字模型）。

---

## 任务 2：交易行按钮布局检测（button）

游戏交易行**购买确认界面**中存在三枚需点击的按钮：`buy`（购买）/ `minus`（子弹数量减号）/ `plus`（加号）。
其中 `buy` 位置随窗口/布局浮动，需模型检测；`minus`/`plus` 在两种固定布局中坐标恒定
（见 `train.py` 的 `LAYOUTS`）。本任务训练轻量 **CenterNet 风格三分类检测器**：输入整幅截图，
独立回归三类按钮的归一化边界框；推理时以「购买」bbox 为锚点判定布局（①/②），
`minus`/`plus` 模型检出且可信时采用模型结果，否则回退到该布局的固定模板坐标，输出完整、精确。

```
输入 [N,3,256,256] (RGB, [0,1])
  → 3×(Conv+ReLU+MaxPool)         # 下采样 8×，得 32×32 特征图
  → heatmap_head : [N,3,32,32]    # 三类按钮中心热力图（原始 logits，通道顺序 buy/minus/plus）
  → size_head    : [N,6,32,32]    # 三类 (w_norm, h_norm) ∈ [0,1]，通道排布 [w0,h0,w1,h1,w2,h2]
```

推理：对每类取 heatmap 最大值格 → 中心；同格读 `(w,h)` → 完整 bbox。
坐标映射：`格(gy,gx) ↔ 模型像素 (gx+0.5)/32*256`。因训练对原图**拉伸**缩放，
还原原图需乘以 `scale_x = orig_w/256, scale_y = orig_h/256`。
布局判定（`determine_case`）比较「购买」bbox 中心与尺寸和两种布局模板的接近程度。

### 两种固定布局（LAYOUTS）

| 布局 | buy | minus | plus |
|------|-----|-------|------|
| ① | `[1482, 829, 297, 61]` | `[1461, 754, 38, 42]` | `[1764, 754, 38, 42]` |
| ② | `[1484, 887, 293, 59]` | `[1461, 815, 38, 42]` | `[1764, 815, 38, 42]` |

### 数据集格式（JSONL，每行一条）

```json
{"image": "images/shot_001.png",
 "targets": {"buy": [1482, 829, 297, 61], "minus": [1461, 754, 38, 42], "plus": [1764, 754, 38, 42]}}
{"image": "images/shot_002.png",
 "targets": {"buy": null, "minus": null, "plus": null}}
```

- `image`：相对数据集根目录（`button_dataset/`）的路径。
- `targets`：三类目标在**原图像素坐标系**的 `[x, y, w, h]`（左上角 + 宽高）；任一为 `null` 表示本帧无该按钮。
- 旧版 `{"bbox":...}` 单按钮格式仍被兼容（仅含 `buy`），但建议统一为 `targets`。

采集建议：用 Rust 端「保存购买界面」导出整幅截图，再用 `annotate_gui.py`（拖拽框选 / 类别切换 / 布局快填）
或 `label.py` 标注三类按钮。也可用合成数据先打通链路：

```bash
python make_dummy_dataset.py --num 200 --out button_dataset
```

### 训练

```bash
python train.py button                                 # 训练并导出 buttons.pt + buttons.onnx
python train.py button --epochs 40 --batch-size 16 --lr 1e-3
python train.py button --data /path/to/button_dataset --out-dir /path/to/out
python train.py button --only-export                  # 仅导出 ONNX（需已存在 buttons.pt）
```

> 注意：按钮检测默认导出 `buttons.onnx`，**不会覆盖**数字模型的 `model.onnx`。

### 推理校验（优先 ONNX，缺 onnxruntime 时回退 PyTorch）

```bash
python predict_layout.py dataset/images/shot_0000.png --draw out.png
python predict_layout.py shot.png --model buttons.onnx --thresh 0.3
# 输出 {"image":..., "case":0|1|null, "targets":{...}}（含三类的 bbox / score / from_case）
```

### 与 Rust 端集成的前处理约定（必须一致）

| 步骤 | Python (train/predict) | Rust 端需复现 |
|------|------------------------|---------------|
| 通道 | RGB | RGBA → 丢 Alpha |
| 缩放 | `resize(256,256)` BILINEAR | `image::resize` Triangle |
| 归一化 | `/255.0` → `[0,1]` | 每通道 `/255.0`（**不做均值减法**）|
| 布局 | NCHW | NCHW |

输出：Rust 端 `Session::run` 得 `heatmap_logits[1,3,32,32]` 与 `size[1,6,32,32]`；
对每类 heatmap 做 `sigmoid`，峰值 `< 阈值` 判为该类别无目标，否则 `argmax` 取中心格并按上表还原坐标，
再经 `config` 的 `Scale` 由客户区映射到屏幕坐标（复用现有点击逻辑）。
以「购买」bbox 判定布局①/②后，`minus`/`plus` 取模板坐标即可精确点击。

---

## 任务 3：子弹「所选数量」整块区域分类（qty）

> 与「任务 1」的价格/数量**逐位**识别（个/十/百分别裁剪、各自分类）不同，本任务对
> **整块数量显示区域**做**整体分类**：输入一张裁剪好的数量显示区截图，模型直接输出该区域表达的
> **整数值（1..200，共 200 类，不含 0）**。
>
> 之所以整块分类：购买界面「所选子弹数量」是**居中显示**的，不同位数（1/2/3 位）对应字符位置
> 并不固定，逐位裁剪会把同一数字切到不同坐标，坐标一偏就整位错乱；整块分类绕开逐位对齐，鲁棒性更好。

### 模型与前处理契约

```
输入  [B, 1, 32, 140]（灰度整块数量区域，NCHW，f32）
  → 3×(Conv+ReLU+MaxPool)         # 32×140 → 16×70 → 8×35 → 4×17
  → Flatten(4352) → FC(128) → FC(200)
输出  [B, 200]（logits，类别索引 = 数值 - 1；索引 0 → 数值 1，索引 199 → 数值 200）
```

**类别映射（务必两端一致）**：目录名 `<value>`（1..200）= 数量值；训练标签 = `value - 1`；
模型输出索引 `i` 对应的数量值 = `i + 1`（Rust 端 `qty_classifier::classify` 已做 `+1`）。

**前处理（必须与 `src/domain/qty_classifier.rs` 的 `preprocess` 完全一致）**：

| 步骤 | Python (train_qty.py) | Rust 端需复现 |
|------|------------------------|---------------|
| 通道 | RGB → `Grayscale(num_output_channels=1)` | RGBA → 丢 Alpha，按 ITU-R 601 灰度化 |
| **对齐+拉伸** | `align_and_stretch()`：按亮像素(≥110)行重心整幅纵向平移至第 16 行（越界补背景 37），再线性拉伸 `clip((x-37)*255/(152-37))` | `align_and_stretch()`（`qty_classifier.rs`，含银行家舍入与 Python `round` 对齐） |
| 缩放 | `Resize((32,140))` BILINEAR | `image::resize` Triangle → 32×140 |
| 归一化 | `ToTensor()` → `[0,1]` → `Normalize((0.5,),(0.5,))` → `(x-0.5)/0.5` | `(p/255 - 0.5)/0.5` |
| 布局 | NCHW `[1,1,32,140]` | NCHW `[1,1,32,140]` |

> **为什么必须「对齐+拉伸」**：可兑换/不可兑换两场景的数量行全局 y 相差约 58px，各自区域
> 裁剪后数字在 140×32 内局部行仍差 2px；细笔画数字错位 2px 即令类内差（12.6）> 类间差
> （10.2），模型无法收敛（acc≈1/200）。对齐+拉伸后类内差 0.63 / 类间差 18.4，1-NN 97.5%，
> 训练 acc 可达 1.0。两端常量：`QTY_STROKE_THRESH=110`、`QTY_STRETCH_LOW=37`、
> `QTY_STRETCH_HIGH=152`、`QTY_ALIGN_ROW=16`（Rust 端同名常量）。

### 两个场景的整块区域坐标（标定视口 1920×1080，[x,y,w,h]，客户区左上角原点）

| 场景 | 整块数量区域 | Rust 端常量 |
|------|--------------|-------------|
| 可兑换子弹 | `(1560, 722, 140, 32)` | `BUY_QTY_REGION_1` |
| 不可兑换子弹 | `(1560, 782, 140, 32)` | `BUY_QTY_REGION_2` |

Rust 端按 `BuyScene::qty_region()` 取对应矩形，再经 `Scale::rect` 映射到实际客户区。

### 数据集格式

```
qty_dataset/
├── 1/        # 数量值 = 1 的整块区域截图（*.png）
├── 2/
├── ...
└── 200/      # 数量值 = 200
```

- 每个子目录名即**数量值**（1..200）；目录名即标注，无需额外映射文件。
- 截图必须是**整块数量显示区域**裁剪图（非完整界面），尺寸任意（训练时会缩放到 32×140）。
- 来源有两种：
  1. **采集工具（推荐）**：Rust 端「数量采集」Tab，手动把游戏切到购买界面后自动循环——
     截整块区域 → 点加号 → 等画面刷新 → 截下一数量值，按真实数量值落入 `qty_dataset/<value>/`，
     每个数量值只截一次，天然带标注。
  2. **`collect` 子命令**：从已有的「完整购买界面截图」按场景坐标裁剪整块区域并标注写入。

### 子命令与参数

`train_qty.py` 提供三个子命令：`collect` / `train` / `infer`。

#### `collect` —— 从完整截图裁剪整块区域

```bash
python train_qty.py train --epochs 30 --batch-size 32 --lr 1e-3 --export-onnx
```

| 参数 | 说明 |
|------|------|
| `--anno` | 标注 JSONL，每行 `{"image":"shots/001.png","scene":"exchangeable","label":31}` |
| `--from-screenshots` | 整目录截图统一处理（配合 `--scene` / `--label`） |
| `--scene` | `exchangeable`（可兑换）/ `non-exchangeable`（不可兑换），决定裁剪坐标 |
| `--label` | 统一标签，必须 ∈ **1..200**，越界（含 0）直接报错 |
| `--out` | 输出数据集根目录（默认 `qty_dataset/`） |

#### `train` —— 训练并导出 `qty.onnx`

```bash
# 推荐：数量 UI 为确定性渲染（对齐后同值截图逐像素一致），关闭增强直接记忆即可，
# 且每类样本少（2 张）时增强反而令模型无法收敛（实测 acc≈1/200）。
python train_qty.py train --no-augment --epochs 60 --export-onnx
python train_qty.py train --epochs 30 --batch-size 32 --lr 1e-3 --no-augment
python train_qty.py train --only-export          # 仅导出 ONNX（需已存在 qty.pt）
```

| 参数 | 说明 |
|------|------|
| `--epochs` | 训练轮数（默认 30） |
| `--batch-size` | 批大小（默认 32） |
| `--lr` | 学习率（默认 1e-3） |
| `--no-augment` | 关闭在线数据增强（亮度/对比度抖动） |
| `--export-onnx` | 训练后导出 `qty.onnx`（供 Rust/ORT 推理） |
| `--only-export` | 仅导出 ONNX（需已存在 `qty.pt`） |

> 导出使用传统导出器 `torch.onnx.export(..., dynamo=False)`，并通过 `onnx.checker.check_model`
> 校验；依赖 `onnx` 包（无需 `onnxscript`）。

#### `infer` —— 单图快速验证（用 PyTorch 权重，无需 onnxruntime）

```bash
python train_qty.py infer shots/buyui_001.png --scene exchangeable
# 输出：[infer] shots/buyui_001.png -> 预测数量 = <value>（类别索引 <idx>，置信度 0.xxx）
```

若为**完整购买界面截图**，用 `--scene` 指定场景以先裁剪整块区域；若已是裁剪好的整块区域图，
直接传入即可，无需 `--scene`。输出为**实际数量值**（已加回 QTY_MIN_LABEL=1）。

### 与 Rust 端集成（已实现）

- `python train_qty.py train --export-onnx` 产出 `nn-classifier/qty.onnx`（输入 `1×1×32×140`，输出 `logits[1,200]`）。
- Rust 端 `src/domain/qty_classifier.rs` 惰性加载 `qty.onnx`，前处理与 `train_qty.py` 一致
  （RGBA → 灰度 ITU-R 601 → **纵向重心对齐 + 线性对比拉伸** → 双线性 32×140 → `[0,1]` → `(x-0.5)/0.5`），
  取 200 类 softmax；
  最高类概率 ≥ 阈值（0.5）采信，输出值 = 索引 + 1（**不含 0**），否则判为未识别（`None`）。
- 模型定位优先级：环境变量 `DELTA_OBSERVER_QTY_MODEL` → `DELTA_OBSERVER_QTY_MODEL_DIR` →
  exe 向上追溯 `nn-classifier/`、`assets/`、`exe 同目录` 寻找 `qty.onnx`。
- `dist.bat` 打包时已复制 `nn-classifier/qty.onnx` 至 `dist/nn-classifier/`，随 Release 产物分发。
- 购买阶段（`src/domain/buy.rs`）据此识别「所选子弹数量」，用减/加按钮调到面板设定的目标值后待命。

### 完整训练流程（端到端）

1. 在 Rust 端「数量采集」Tab 设定**起始数量**（1..200）与场景，开始采集 → 自动生成带标注的
   `qty_dataset/<value>/*.png`（每个数量值只截一次）。
2. （可选）用 `collect` 补充从完整截图裁剪的样本，或人工校正 `qty_dataset/` 子目录。
3. `python train_qty.py train --epochs 30 --batch-size 32 --export-onnx` 产出 `qty.onnx`。
4. 将 `qty.onnx` 放入 `nn-classifier/`（或 `dist/nn-classifier/`、exe 同目录），Rust 端识别即生效。
