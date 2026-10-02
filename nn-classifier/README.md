# 数字分类训练子项目（神经网络替代模板匹配）

本目录是「三角洲行动」交易行价格数字识别的 **神经网络分类器** 训练工程，目标是
取代 `src/template_match.rs` 的逐位模板匹配。

## 类别

固定 11 类：`0 1 2 3 4 5 6 7 8 9 x`

- `0-9`：单个数字。
- `x`：**非数字**（空槽、货币图标、千分位逗号等所有非 0-9 字形）。
  推理时预测为 `x` 即视为「该数位不是数字」，用于支撑价格结构判定。

类别顺序由 `train.py` 的 `CLASSES` 固定，训练后写入 `classes.json`，
推理端（含未来 Rust 端 ONNX 集成）必须按此顺序解释模型输出。

## 目录结构

```
nn-classifier/
├── dataset/            # 训练数据：每个类别一个子文件夹
│   ├── 0/ 1/ ... 9/    # 数字样本（由 Rust 端采集 + 人工校正）
│   └── x/              # 非数字样本
├── train.py            # 训练脚本（输出 model.pt + classes.json，可选导出 ONNX）
├── predict.py          # 单图推理验证
├── requirements.txt
└── README.md
```

## 数据采集（来自 Rust 端）

1. 在「开始观察」面板勾选 **「保存识别到的数字」**（默认关闭）；勾选后动态出现
   「数字输出目录」输入框，填入任意自定义目录（默认 `<结果保存目录>/digits`）。
2. 启动观察后，每枚子弹的 4 个数位裁剪（内缩后的数位框，RGBA）统一写入该目录，
   文件名形如 `d{序号:08}_{label}.png`：
   - `label` 为模板匹配的识别结果（0-9）；未识别（< 90%）或异常字形记为 `x`；
   - 标签仅作人工校正参考，**不做分类**——所有裁剪落在同一目录。
3. **人工校正**：模板匹配可能有误判，请按需要对裁剪图重命名 / 移动到
   `dataset/{0,1,...,9,x}/` 正确类别文件夹，确保标签干净后再训练。

## 训练

```bash
pip install -r requirements.txt
python train.py                          # 训练，保存 model.pt + classes.json
python train.py --epochs 30 --batch-size 64
python train.py --export-onnx model.onnx # 额外导出 ONNX（供 Rust 端集成）
```

## 推理验证

```bash
python predict.py path/to/digit_crop.png
```

## 与 Rust 端集成（已实现）

1. 训练后用传统 ONNX 导出器导出（**无需 onnxscript**）：
   `python train.py --export-onnx model.onnx`
   > 内部使用 `torch.onnx.export(..., dynamo=False)` 走 TorchScript 导出路径，
   > 因此只依赖 `onnx` 包，不依赖 `onnxscript`；requirements.txt 已声明 `onnx>=1.13`。
2. Rust 端已引入 `ort`（ONNX Runtime）crate，新增 `src/nn_classifier.rs`：
   - 启动时惰性加载 `model.onnx` + `classes.json`（搜索顺序：环境变量
     `DELTA_OBSERVER_MODEL` / `DELTA_OBSERVER_MODEL_DIR` → exe 向上追溯
     `nn-classifier/model.onnx`、`assets/model.onnx` → exe 同目录）；
   - 对每个数位裁剪做与 `train.py` **完全一致**的前处理
     （RGBA → 灰度 ITU-R 601 → 双线性 32×32 → `[0,1]` → `(x-0.5)/0.5`），
     喂入 ONNX，取 11 类 softmax 概率；
   - `x` 类（非数字）与低于 90% 置信度一律判「无数字」，与模板匹配 0.90 硬规则一致。
3. `src/digit.rs::match_one` 已接入：模型可用时优先走 NN，否则自动回退模板匹配，
   保证无 `model.onnx` 时程序仍可运行。`OCR 测试` Tab 复用同一逻辑并展示概率详情。

输入约定（前处理）必须两端一致，详见 `train.py` 顶部的 `INPUT_SIZE` 与 `TRANSFORM`，
以及 `src/nn_classifier.rs` 的 `preprocess`。
