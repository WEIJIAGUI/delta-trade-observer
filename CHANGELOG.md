# 变更记录（CHANGELOG.md）

> 每次修改后追加一条。后续会话先读 `PROJECT.md` + 本文件即可，无需重载全部上下文、无需重读整个项目。

## 2026-10-01（续2）— 模板匹配预处理归一化（精度从全空→0.9+）+ GAUGE12 注释同步
- **问题**：用户确认数字截图（坐标）无误，但模板匹配识别效果差。实测复刻 NCC 管线：真实游戏数字对 10 个模板的 NCC 仅 `0.21~0.34`（全部低于 0.65 阈值 → price 全空）。但逐位肉眼核对字形与模板高度相似 → 字体并非主因。
- **根因（定位）**：旧管线 `to_feature` 直接把 12×20 整框拉伸到 32×32。真实数字在裁剪框内**顶部对齐、下方大片黑**，而模板数字**居中**；二者拉伸后数字位置/留白不一致 → NCC 崩溃。属预处理缺陷，非模板字体问题。
- **修复（`template_match.rs`）**：`to_feature` 替换为 `norm_feature`——灰度→中值去噪→按 `(min+max)/2` 阈值取前景→剔除孤立点→求数字**前景包围盒**→等比缩放最长边至 `DIGIT_FIT=28` 居中贴入 32×32 黑底画布→NCC。空白/低对比框（`MIN_CONTRAST`/少量前景）直接返回 `None` 判空。阈值 `DEFAULT_MATCH_THRESHOLD` 0.65→0.72（真实数字现稳定 0.9+，噪声/图标通常 ≤0.78）。
- **验证（Python 复刻 + 真实 `1.png` 用当前 GAUGE12 坐标裁全部 9 槽×4 位）**：NCC 跃升至 `0.92~1.00`，价格全部正确读出（如龙息弹「1920」全 0.95+，DART「6440」等）。结论：**模板匹配方向完全可行，无需神经网络**。
- **config.rs 注释同步**：GAUGE12 用户已改为 12×20 框（原 11×18），更新「数位坐标定位规则」注释——ones≈cell_x+443、tens+432、hundreds+421、thousands+405（千/百间留~16px 容纳逗号）；数位 y=278/438/596（各 row_y 下约 130px，落格子底部）；明确标注首列 +444 等实测偏移，删除旧 282/446/607 与 11×18 描述。
- **编译踩坑**：LNK1104 本地运行 delta-trade-observer.exe(PID 251752)+残留 cargo(PID 254460) 持锁；taskkill 终止后链接。Rust 实现细节：`copy_from` 需 `use image::GenericImage` 且返回 `Result`（`.ok()?`）；`denoised` 需 `mut`。cargo build 0 error/0 warning。
- **待用户本地核验**：重跑观察/OCR 测试确认 price 已填；3 位价格的货币图标位阈值边缘（0.72）若误认成数字，上提到 0.8 并排查图标位。

## 2026-10-01（续3）— 匹配阈值落实为硬规则 0.90（低于 90% 一律判无数字）+ 8/0 混淆根因定位
- **用户新要求**：相似度必须达 90%，低于 90% 一律认为该位置没有数字；并反馈 8 与 0 经常混淆、测试 case 不足。
- **变更（`template_match.rs`）**：`DEFAULT_MATCH_THRESHOLD` `0.72 → 0.90`，注释改写为硬规则（只可上调不可下调，说明低于 0.90 直接拒绝即消除 8/0 因局部顶环误判）。
- **变更（`digit.rs`）**：删除 `RETRY_THRESHOLD_FACTOR=0.8` 与 `retry_missing`（旧逻辑会把阈值降到 0.72/0.576，直接违反 90% 规则）。`recognize`/`recognize_detail` 改为单遍、统一 0.90，**不存在放宽重试**；模块文档同步。
- **8/0 混淆根因（实证，非模板字体问题）**：用 `1.png` 真实截图逐位裁出 9 槽×4 位，配合价格条 ASCII 可视化确认——交易行数字实际约 **15px 宽、相邻间距仅 11px**，而数位框仅 **12px 宽**。固定框裁出的常是数字**局部或邻位碎边**：「8」被裁成上半截后顶环极像「0」。加宽框到 18px 反而更差（把相邻数字也框入，36/36 全低于 0.90）。
- **结论（回答「模板匹配方向是否可取」）**：模板匹配本身没问题——**干净完整裁出的数字 NCC 稳定 0.9~1.0**（如龙息弹 8 达 0.935、个位 0 达 1.000）。精度瓶颈在**固定框切分**，不在匹配器；神经网络替代方案同样要先过这一关（需足量真实数字标注样本），属过度工程，**不推荐**。`cargo build` 0 error/0 warning（仅被杀进程遗留增量锁告警，无害）。
- **下一步（待用户确认）**：0.90 规则 + 当前 12px 框下，被框切到局部的位仅 0.6~0.85 → 按规则判无数字，符合「宁可不认也不认错」，但召回约减半。要同时保住精度与召回，须把「固定 4 框」改为**整条价格带连通域切分**（用 GAUGE12 推导整条价格包围盒，按列投影找数字间空白切分）。此改动触及坐标使用方式，且用户曾表示数字截图无误，故先提方案待确认再实施。

## 2026-10-01（续）— 「价格浏览」滑块改为上一个/下一个按钮（修正回弹 0）
- **问题**：用户反馈「价格浏览」滑块每次调整完会回到 0、调整不生效。根因：`Slider::new(&mut sel, ...)` 绑定的是**局部变量** `sel`（main.rs 原 829/835 行），从未写回 `app.ocr_test_selected`；每帧 `sel` 被重新从 `app.ocr_test_selected`（仍为 0）初始化 → 滑块每次回弹 0。
- **修复**：删除 Slider，改为「◀ 上一个 / 下一个 ▶」按钮（`ui.add_enabled` 边界禁用），点击时直接 `app.ocr_test_selected = cur ± 1` 写回；按钮下方显示「（第 X / Y 个）」。改写后选中态由 `app.ocr_test_selected` 持久化，数字纹理重建逻辑（依赖 `app.ocr_test_selected` 与 `app.ocr_test_digit_sel` 比对）自然同步。同时更新相关注释（滑块→按钮）。
- **编译踩坑**：cargo build 再次报 LNK1104，实为本地运行中的 delta-trade-observer.exe(PID 247624) + 残留 cargo.exe(PID 254676) 持锁；`taskkill /F /PID` 终止后链接成功（warning: error deleting lock file for incremental 为被杀进程遗留的无害清理告警）。

## 2026-10-01（续4）— 神经网络分类训练子项目 + 「开始观察」勾选保存数字
- **用户决策**：把模板匹配改为神经网络分类任务。本轮落地**训练项目 + 数据收集**（模型训练与集成回 Rust 作为后续步骤）；x 类别定义由用户人工分类，Rust 端只负责把识别出的数字裁剪落到 `nn-classifier/dataset/<label>/`。
- **新增 Python 子项目 `nn-classifier/`**：`train.py`（11 类 0-9+x 的轻量 CNN，ImageFolder 加载 `dataset/<label>/`，固定 `CLASSES` 顺序，保存 `model.pt`+`classes.json`，支持 `--export-onnx`）、`predict.py`（单图推理验证）、`requirements.txt`（torch/torchvision/pillow/numpy）、`README.md`（采集/训练/集成说明）。已建 `dataset/{0..9,x}/` 共 11 个空文件夹（含 .gitkeep）。输入约定：灰度→32×32→`(x-0.5)/0.5` 归一化（与未来 Rust 端 ONNX 推理前处理必须一致，已在两端注释标注）。
- **Rust 端 `observe.rs`**：新增 `digits_dataset_dir()`（优先 env `DELTA_OBSERVER_DIGITS_DIR`，否则从 exe 向上查找含 `nn-classifier/` 的目录取其 `dataset/`，兜底 exe 同目录）。`run_observation` 新增 `save_digits: bool` 参数；观察主循环改用 `recognize_bullet_detail`（与 OCR 测试同源、零重复）拿到每位数位裁剪 `DigitDetail`，勾选时按识别结果归类落盘——识别为 0-9 入对应文件夹，未识别（<90%）入 `x/`；文件名 `d{序号:06}.png`，序号全局自增防覆盖。
- **Rust 端 `main.rs`**：`ObserverApp` 新增 `save_digits: bool`（默认 false）；「开始观察」面板新增复选框「保存识别到的数字用于训练」（默认关闭，仅勾选时落盘，零额外 IO）；`start_observation` 将勾选态传入观察线程。
- **清理**：删除因改走 `*_detail` 路径而变为死代码的 `PriceRecognizer::recognize`（`digit.rs`）与 `BulletRecognition::recognize_bullet`（`recognition.rs`），保持 0 warning。`cargo build` 0 error/0 warning。
- **待用户本地核验**：勾选复选框运行「开始观察」，确认 `nn-classifier/dataset/{0..9,x}/` 下出现裁剪图；人工校正标签后 `pip install -r requirements.txt && python train.py` 训练。模型训练与 ONNX 集成回 Rust 替换模板匹配为后续步骤。
- **验证**：cargo build 0 error / 0 warning。待用户重跑核验：点「识别」后点击「下一个/上一个」可在各子弹间切换并正确展示逐位数字模板匹配图。

## 2026-10-02 — 数字保留功能改为用户自定义输出目录（移除硬编码训练集路径）
- **用户新要求**：移除写死到 `nn-classifier/dataset` 的硬编码路径；开启保留功能时**动态显示**一个路径输入框供自定义输出目录；所有保留内容**统一输出到该目录、不做分类处理**。
- **`observe.rs`**：删除 `digits_dataset_dir()`（env 向上查 `nn-classifier/dataset` 的硬编码解析逻辑）；`run_observation` 新增 `digits_dir: String` 参数；原「按 0-9/x 建子文件夹」逻辑删除，改为所有数位裁剪统一写入用户指定目录（无分类）。`digits_dir` 仅在 `save_digits=true` 且路径非空时 `create_dir_all`，否则为 `None` 全跳过。`digit_seq` 文件名格式 `d{序号:08}_{label}.png`（label=识别出的 0-9，未识别<90% 为 x，仅作人工校正参考，不决定落盘位置）。
- **`main.rs`**：`ObserverApp` 新增 `digits_save_path: String`（默认 `<save_path>/digits`）；「开始观察」面板复选框「保存识别到的数字」勾选后**动态显示**「数字输出目录」输入框（`TextEdit::singleline`），未勾选时该框不渲染；`start_observation` 将 `digits_save_path` 传入观察线程。输入框**仅在开始观察时读取一次**（不在每次按键写盘），与现有 `save_path` 持久化原则一致，避免配置自我漂移。
- **编译踩坑**：cargo build 报 LNK1104（本地运行 delta-trade-observer.exe PID 228712 + 残留 cargo 持锁）；`taskkill /F` 释放后链接成功。
- **验证**：cargo build 0 error / 0 warning。待用户重跑核验：勾选复选框→出现目录输入框→填入路径→「开始观察」后所有数位裁剪落入该单一目录（文件名含序号与标签），不再出现 `nn-classifier/dataset` 子目录结构。

## 2026-10-01 — OCR 测试 Tab「价格浏览」滑块不可见修复 + LNK1104 锁进程
- **问题**：OCR 测试 Tab 识别后预览大图与「识别汇总」正常，但「价格浏览」滑块（egui::Slider）始终不出现。排查已排除 slots 为空（汇总已显示子弹/价格 → n>0）与识别失败（否则整块不渲染）→ 实为**布局根因**：结果区（预览图 + 280px 汇总 ScrollArea + 滑块）直接挂在 CentralPanel 垂直布局、无外层滚动容器，窗口高度不足时滑块被推到视口外且无滚动条可达。
- **修复**：OcrTestPanel::show 将结果区整体用 `egui::ScrollArea::vertical()` 包裹（输入卡片固定顶部），滑块置于可滚动区内，任意窗口高度均可滚到。
- **编译踩坑**：cargo build 报 LNK1104 无法打开 `delta_trade_observer.exe`，实为**本地运行中的 delta-trade-observer.exe(PID 255316) + 残留 cargo.exe(PID 251564) 持锁**；`taskkill /F /PID` 终止两者后链接成功。注：本机 PowerShell 工具输出管道异常（无回显），进程排查改用 `tasklist`/Bash 更可靠。
- **验证**：cargo build 0 error / 0 warning。待用户重跑核验滑块在任意窗口高度下可滚动到达。

## 2026-09-29（续32）— 数字识别重构为单字符模板匹配 + 静态资源目录
- **需求**：识别链路改为「调用方先按位切分出单张数字图 → 逐张传入识别函数」。函数**只识别一个字符**，返回 `"0"`~`"9"` 或 `""`（无法识别），**绝不处理位权 / 位数判断 / 多位拼接**。
- **新增 `src/template_match.rs`（模板匹配，内联实现 TM_CCOEFF_NORMED）**：
  - 算法等价 OpenCV `cv2.matchTemplate(..., TM_CCOEFF_NORMED)` 的归一化互相关：`Σ(T'·I') / sqrt(ΣT'² · ΣI'²)`（`T'`/`I'` 为减均值后的像素），取值 `[-1.0, 1.0]`。由于输入图与模板已**统一缩放到相同尺寸**，滑动窗口只余单一位置、响应图退化为单个标量，故实现上就是两条特征向量的一次点积，无需真正的逐像素滑窗。
  - **不引入 `opencv` crate**：其 Rust 绑定依赖本机 OpenCV SDK，编译成本与环境要求远超本模块收益；解码与缩放复用已在用的 `image` crate，**零新增依赖**（符合 `PROJECT.md` 的「依赖精简」约定）。
  - 预处理管线：灰度化 → 3×3 中值去噪 → `resize` 到 `32×32` → 拉平为 `f32` 特征。输入图与模板**共用同一管线**（`to_feature`），避免分布漂移。
  - 对外 API：`recognize_digit_image(rgba, w, h)`（默认阈值）/ `recognize_digit_image_th(..., threshold)`（自定义阈值），返回**字符串**（便于调用方直接拼接）。
  - **阈值可配置**：`pub const DEFAULT_MATCH_THRESHOLD: f32 = 0.65`，注释写明语义与调节经验——调低（<0.5）易把噪声/空位/半个数字误判成某数字（价格凭空多出位数，危害最大），调高（>0.85）易漏判导致少位；默认值取「宁可少认一位，也不认错一位」。
  - 模板缓存用 `OnceLock`（进程内仅加载一次）；单个模板缺失不影响其余数字，目录缺失则识别恒为空。
- **模板静态资源**：新建 `assets/digits/`（配 README 说明放置规范与缺失时的兜底做法）。定位顺序：`DELTA_OBSERVER_DIGITS` 环境变量 → 可执行文件目录**向上逐层**（最多 6 层）→ 当前工作目录（沿用历史上 `config/` 的追溯策略）。**换模板图需重启程序**。
- **`digit.rs` 接入**：`recognize_single_digit` 的后端由 OCR 改为调用 `template_match::recognize_digit_image`（自身只保留「裁出单字符图」这一步）；上层 `recognize_price_digits` 的逐位拼接逻辑与调用方 `observe` **完全未动**。`main.rs` 补 `mod template_match;`。
- **清理改造后的冗余**（遵循既有克降原则）：删除 `ocr::recognize_price`（整框价格 OCR，唯一调用者消失）、`observe::extract_price`（唯一调用者 `recognize_price` 已删，且价格不再来自 OCR 文本）、`template_match::templates_loaded`（自查辅助，无调用者）；同步更新 `observe.rs` 头部流程注释（第 3/4 步改为「名称走 OCR、价格走逐位模板匹配」）。
- **验证**：① `cargo build` 通过，**代码相关 0 warning**（仅剩与代码无关的 Windows 增量编译锁文件 `os error 5` 提示）；② 端到端自检 **10/10** —— 以临时生成的 Arial `0~9.png` 同时作为模板与输入，`0`→`"0"` … `9`→`"9"` 全部命中，纯白空图正确返回 `""`（拒识生效）。自检用的临时代码与临时图片**已全部删除**。
- **待真实环境核验**：实际游戏字体 / 抗锯齿下的命中率与阈值松紧（`0.65` 可能需按实测微调）；模板可取 `DELTA_OBSERVER_DEBUG=1` 运行观察后落盘的 `debug_test_digits/dN_raw.png`。


## 2026-09-27（续27）— 测试观察三处交互缺陷修复：Ctrl 拖拽 + F9 全局兜底 + 价格 OCR 阈值
- **问题确认（排查结论）**：
  1. 毛玻璃拖拽失效：`test_grid` 旧实现用 `ViewportCommand::StartDrag`（OS 级窗口移动），在透明 AlwaysOnTop 面板上不可靠且无法限定为 Ctrl 组合键，故「无法拖动」。
  2. F9 无法退出测试态：`stop()` 仅由 `global-hotkey`（RegisterHotKey）触发；`start()` 调用 `win::focus_window(game)` 把键盘焦点交给游戏，游戏以无边框/全屏原始输入独占焦点时系统不把 F9 投递到本进程 → `stop()` 永不执行。
  3. 测试浮层「无内容」：价格全部为空。`ocr.rs::is_price_text` 旧阈值要求饱和度 `mx-mn < 170`，而游戏价格常为亮黄/金（如纯黄 255,255,0 → mx-mn=255），被整片判为背景 → 二值化后空白 → `digit::recognize_price_digits` 全空 → `test_observe.rs` 跳过全空格 → 仅剩占位卡片。
- **修复 1 · Ctrl+左键拖拽（手动实现）**：`test_grid` 标题栏作拖拽手柄，`ui.ctx().input(|i| i.modifiers.ctrl)` 监测 Ctrl，按住 Ctrl+左键（`drag_started()`）记录 `(指针起点, 窗口起点)`，之后每帧按 `p - p0` 位移经 `ViewportCommand::OuterPosition` 移动窗口；左键松开即结束。新增 `ObserverApp::drag: Option<(egui::Pos2, egui::Pos2)>` 保存拖拽态（已在 `new()` 初始化）。该方案不依赖 OS 拖拽，且因限定 Ctrl+左键、手柄仅限标题栏，**不与卡片/按钮/网格等交互冲突**。
- **修复 2 · F9/F10 全局兜底（低级键盘钩子）**：新增 `src/hotkey_hook.rs`，用 `SetWindowsHookExW(WH_KEYBOARD_LL)` 安装低级钩子（主线程安装一次），于按键分发前捕获 F9/F10 并置位原子标志，**透传按键不影响游戏输入**；`poll_hotkeys` 在 `global-hotkey` 之后消费该标志，与原有逻辑合并为 `handle_f9`/`handle_f10`。即使游戏独占键盘焦点，F9 也能退出测试态。
- **修复 3 · 价格 OCR 阈值放宽**：`ocr.rs::is_price_text` 改为「明亮（max>170）且具颜色（mx-mn>25）即文字」，去除 `mx-mn<170` 上限，保留金黄硬币图标排除；价格框已分 5 数位小框，放宽不会引入数字外干扰。价格缺失问题基本消除。
- **调试辅助**：`digit.rs::recognize_price_digits` 新增 `DELTA_OBSERVER_DEBUG=1` 时把每个数位 `dN_raw.png` / `dN_bin.png`（二值化放大）落盘到 `debug_test_digits/`，便于真实环境校准坐标与阈值（默认关闭零开销）。`TestCell`/`TestResult` 派生 `Clone` 以支持结果帧克隆、释放借用便于拖拽逻辑修改 `app.drag`。
- **验证**：`cargo build` 0 error / 0 warning。待真实 Windows + 已启动《三角洲行动》核验：Ctrl+左键拖动手感、F9 退出是否生效、价格数字识别率与 `split_price_5` 数位分割点精度（必要时据 `debug_test_digits/` 落盘图微调内缩/边距）。

## 2026-09-27（续25）— 工具标签页 + 毛玻璃卡片精致化 + 价格分位深度学习方案
- **git 初始化**：项目根目录 `git init`；`.gitignore` 补全（Rust/IDE/本地 `save_path.cfg` 及 `training/` 产物目录）。**未执行任何 commit**（由用户自行提交）。
- **新增「工具」标签页**（控制面板第 4 个标签）：
  - `rfd` 图片选择（`add_filter` 图片类型）→ `image` crate 解码 → `ctx.load_texture` 生成纹理缓存于 `ObserverApp::tool_texture`；
  - 四个数值输入框 `x/y/w/h`（`DragValue`，clamp 0..=4096）→ `ObserverApp::tool_rect`；
  - 预览图按可用宽度等比缩放，并用 `ui.painter()` 实时叠加半透明红色矩形高亮当前 `tool_rect`，直观定位截图范围；「复制坐标」按钮写入剪贴板（格式 `[x,y,w,h]`）。
- **毛玻璃卡片精致化**：测试观察九格结果由朴素 3×3 网格改为 `price_card` 精致卡片——半透明白底圆角 + 柔和外描边 + 轻阴影；左上蓝色序号徽标 + 右上「价格」标签 + 名称小字 + 24px 金色大字号价格（未识别显示「—」）。视觉清晰优雅，融入浅色毛玻璃界面。
- **价格识别改为逐位（万/千/百/十/个）深度学习友好架构**：
  - `config.rs`：`BulletRects` 新增 `price_digits: [Rect;5]`；新增 `split_price_5(price)` 将价格框水平均分 5 份（个位吸收取整余量）；`GAUGE12` 每个槽位派生 `price_digits`。
  - `digit.rs`：`recognize_price_digits(rgba,w,h,digits:[Rect;5])` 对 5 个位分别 `recognize_single_digit` 后按序拼接（前导空位跳过、中间空位补 0）；`recognize_single_digit` 即为**深度学习接入点**（当前走 OCR 回退，替换函数体即可接 ONNX，调用方不变）；删除已无调用的单框 `recognize_price`。
  - `observe.rs` / `test_observe.rs`：价格识别改用 `recognize_price_digits`，坐标经 `price_digits.map(|r| scale_rect(r, cw))` 缩放。
- **训练工程目录 `training/`**：`README.md`（对接说明：Rust 端 `digit.rs::recognize_single_digit` 为接入点、目录约定）、`PROMPTS.md`（完整提示词：背景/技术方案/数据准备/模型训练/推理接口对接/一键提示词/验收标准）、`synth_digits.py`（合成数据生成骨架参考）。`.gitignore` 已忽略 `training/{data,models,exports,.cache}` 产物。
- **依赖**：新增 `image = "0.25"`（工具页图片解码）。
- **踩坑**：`[Rect;5].map` 稳定可用；`egui::ColorImage::from_rgba_unmultiplied` 需 unmultiplied RGBA（`image::RgbaImage::into_raw` 恰好满足）；`ctx.load_texture` 在 egui 0.27 直接返回 `TextureHandle`（非 Result）。
- **验证**：`cargo build` **0 error / 0 warning**（新增 `image` 依赖联网解析编译通过）。真实环境需核验：① 工具页图片预览叠加矩形交互；② 毛玻璃九格卡片美观度；③ 逐位价格识别准确率（分位坐标是否准切到各位数字，必要时调 `split_price_5` 内缩/边距）。

## 2026-09-26
- **创建项目骨架**：`Cargo.toml`、`src/main.rs`、`src/win.rs`。实现 Phase 1 GUI 框架（Idle/Observing 两态、F5 开始、F6 结束、置顶悬浮面板、聚焦游戏窗口）。
- **新增就近镜像**：`.cargo/config.toml`（rsproxy sparse），仅加速「首次 / 全新环境」依赖下载。对重复编辑的编译耗时基本无影响——瓶颈是 CPU 编译，不是网络。
- **建立索引与变更记录机制**：`PROJECT.md`（索引 / 状态 / 坑位）、`CHANGELOG.md`（本文件）、`.gitignore`（排除 `/target`，避免重复读取巨大编译产物）。
- **修复编译错误（适配 eframe 0.27 API）**：
  - `NativeOptions` 改用 `viewport: ViewportBuilder`（`with_title` / `with_inner_size`），去掉不存在的 `initial_window_size` 字段。
  - `ctx.input()` 改为闭包式 `ctx.input(|i| i.screen_rect)`。
  - 移除签名不符的 `App::setup`，窗口标题改由 `ViewportBuilder` 设置。
  - `main()` 返回 `eframe::Result<()>`；`AppCreator` 直接返回 `Box<dyn App>`（不再包 `Ok`）。
  - Win32 回调类型由 `WINDOWENUMPROC` 改为 `WNDENUMPROC`（实际由 `EnumWindows` 推断，无需显式 import）。
- **状态**：Phase 1 代码已通过 `cargo build`（dev）编译，产物 `target/debug/delta-trade-observer.exe`。沙箱无显示器，未做运行期验证。

## 2026-09-26（续）— 透明悬浮面板 + 中文乱码修复
- **修复中文乱码（tofu 方块）**：新增 `install_cjk_font()`，在首帧用 `ctx.set_fonts` 加载系统 CJK 字体（依次尝试 `msyh.ttc`/`msyh.ttf`/`simsun.ttc`/`simhei.ttf`），以 `FontData::from_owned(Vec<u8>)` 读入并追加到 `Proportional`/`Monospace` 字体族回退列表（英文仍用默认字体，中文回退到 CJK）。原因：egui 0.27 默认字体不含中文字形。
- **悬浮面板改为透明样式**：`NativeOptions` 启动即 `with_transparent(true)`（创建分层窗口，也是截图排除的前置条件）；观察态 `CentralPanel` 用 `Frame::default().fill(Color32::TRANSPARENT)`，仅文字可见、背景全透明，不遮挡游戏画面。
- **不遮挡游戏交互（鼠标穿透）**：观察态发送 `ViewportCommand::Decorations(false)` + `ViewportCommand::MousePassthrough(true)`；结束态恢复 `Decorations(true)` + `MousePassthrough(false)`。
- **不进入游戏截图像素（关键）**：新增 `win::set_exclude_from_capture(hwnd, bool)`，调用 Win32 `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`（用原始值 `0x11`，跨版本稳）。观察态启用、结束态恢复 `WDA_NONE`。覆盖 DWM 合成 / BitBlt / PrintWindow / 游戏栏 / OBS 等所有基于窗口的截图与录屏。
- **架构说明**：透明/无边框/鼠标穿透走 egui 原生 ViewportCommand（跨平台、低风险）；仅「截图排除」用 Win32（egui 无对应能力）。观察态不再放可点击按钮（穿透下无法点击），结束统一用全局热键 F6。
- **状态**：代码已通过 `cargo build`（dev）编译，0 warning。沙箱无显示器，透明/穿透/截图排除需真实 Windows + 已启动游戏环境验证（见 PROJECT.md 验证方法）。

## 2026-09-26（续2）— 快捷键改 F8/F9 + 双独立面板 + 毛玻璃
- **快捷键重定义**：将「观察」模式重定义为「截图」模式，开始=**F8**、结束=**F9**（替代原 F5/F6）；UI 文案同步改为「开始截图 / 正在截图 / 按 F9 结束截图」。全局热键 `f8_id`/`f9_id` 比对。
- **拆分为两个独立面板组件**（满足"不可共用同一组件"）：新增 `struct ControlPanel`（Idle 主控制面板）与 `struct FloatingPanel`（Capturing 毛玻璃悬浮面板），二者各自实现 `show(ui, app)`，`update()` 按 `mode` 分别调用，不再共用同一个 `CentralPanel` 内的 `match`。
- **毛玻璃（glassmorphism）悬浮面板**：`FloatingPanel` 用 `Frame` 绘制浅色半透明卡片（`from_rgba_unmultiplied(228,234,246,70)`）+ 圆角 16 + 亮色细边 + 柔和阴影；并叠加 Win32 `DwmEnableBlurBehindWindow(hwnd, DWM_BLURBEHIND{hRgnBlur=NULL})` 实现真正的背景模糊（frosted glass）。`windows` 依赖新增特性 `Win32_Graphics_Dwm`、`Win32_Graphics_Gdi`。
- **截图自动排除（关键约束）保持稳定**：`set_exclude_from_capture(hwnd, true)`（`WDA_EXCLUDEFROMCAPTURE`）在 `start()` 启用、`stop()` 恢复；同时叠加 `set_window_blur` 开/关。窗口为分层窗口（启动 `with_transparent(true)`），前置条件满足。
- **编译踩坑（已修，避免再踩）**：
  - windows 的 DWM 特性名是 `Win32_Graphics_Dwm`（模块 `Win32::Graphics::Dwm`），**不是** `Win32_UI_Shell_DwmApi` / `Win32_UI_Shell::DwmApi`。
  - `egui::Shadow` 不在 `egui` 根，需用 `eframe::egui::epaint::Shadow`；`Shadow.offset` 是 `Vec2`（用 `egui::vec2(0.0,6.0)`），`blur`/`spread` 为 `f32`。
  - `Margin::same` 参数为 `f32`（写 `16.0`）。
- **状态**：`cargo build`（dev）通过，0 warning。毛玻璃观感与 DWM 模糊、截图排除需在真实 Windows + 游戏环境验证。

## 2026-09-26（续3）— 交互逻辑重构：解绑 F8 / F9 即时响应 / 双按钮双态
- **解绑 F8**：`开始观察` 改为**仅鼠标点击触发**，不再注册 F8 快捷键（移除了 `f8_id` 与 F8 注册）。
- **F9 即时响应修复**：原 F9 因 eframe 0.27 默认 reactive 重绘，在窗口失焦 + 鼠标穿透后停止主动重绘，导致全局热键事件积压、界面"只在内部切换状态"。修复：`update()` 末尾 `ctx.request_repaint_after(30ms)` 维持约 30fps 持续重绘，命中热键后再 `request_repaint()`；`stop()` 同步发送恢复指令（`Decorations(true)`/`MousePassthrough(false)`/`WDA_NONE`/关闭模糊/焦点归还），按下 F9 即立刻消失并回到开始面板。注：eframe 0.27 `NativeOptions` 无 `run_mode` 字段，故用 `request_repaint_after` 而非 RunMode。
- **双按钮 + 双激活态**：`Mode` 改为 `Idle` / `Active(CaptureKind)`，其中 `CaptureKind::{Observing, Screenshot}`。
  - 开始面板「开始观察」→ `start(Observing)` → 激活游戏窗口，毛玻璃显示「结束观察」。
  - 开始面板「开始截图」（文案标注 `(F10)`）→ `start(Screenshot)`，也可按 **F10** 触发 → 毛玻璃显示「截图」。
  - 两个激活态共用 `FloatingPanel`，按 `CaptureKind` 显示不同状态文字与 F9 提示文案。
- **截图自动排除稳定**：进入任一激活态即 `set_exclude_from_capture(hwnd, true)`（`WDA_EXCLUDEFROMCAPTURE`）+ DWM 模糊；结束时恢复 `WDA_NONE` 并关闭模糊，面板不会进入任何基于窗口的游戏截图。
- **状态**：`cargo build`（dev）通过，0 warning（8.29s）。运行期行为需真实 Windows + 游戏环境验证（F9 即时性、F10/按钮触发、毛玻璃观感、截图排除）。

## 2026-09-26（续4）— 截图快捷键规则澄清
- **规则确立**：「开始截图」**按钮不绑定任何快捷键**（仅鼠标点击）；**F10** 为截图快捷键（= 激活三角洲窗口并进入截图状态）；**F9** 维持原行为（结束截图状态，立即恢复主面板）。
- **代码微调**：去掉按钮上误导性的 `(F10)` 文案（避免让人误以为按钮绑定了快捷键），改为纯文字「▶ 开始截图」，并在面板下方新增一行提示「开始截图快捷键：F10（开始截图按钮不绑定快捷键）」。`poll_hotkeys` 中 F10→`start(Screenshot)`、F9→`stop()` 行为不变；F8 仍不注册。
- **注意**：编译时若 `target/debug/...exe` 被上一轮运行实例锁定，linker 会报 `LNK1104: cannot open file`。需先结束运行中的 `delta-trade-observer.exe`（如 `taskkill /PID <pid> /F`）再编译。
- **状态**：`cargo build`（dev）通过（6.61s，仅一条无害的 incremental lock 文件删除警告）。

## 2026-09-26（续5）— 实现窗口截图功能 + 保存目录选择/持久化 + 控制面板分组优化
- **实现窗口截图**：新增 `win::capture_window_to_png(hwnd, dir)`，用 `PrintWindow(PW_RENDERFULLCONTENT=0x2)` + `GetDIBits` 抓窗口像素（BGRA→RGBA），经 `image::save_buffer` 存为 PNG（时间戳文件名 `shot_YYYYMMDD_HHMMSS.png`）。新增依赖 `image=0.25`、`rfd=0.15`、`windows` 特性 `Win32_Storage_Xps`/`Win32_System_SystemInformation`。
- **F10 双角色**：`poll_hotkeys` 中 F10 在 `Idle` 态→`start(Screenshot)`（激活窗口+进入截图态）；在「截图」激活态→`capture_screenshot()`（保存 PNG 到保存目录）。F9 结束行为不变。`capture_window_to_png` 内 `create_dir_all` 确保目录存在。
- **保存目录选择 + 持久化**：控制面板「截图模式」卡内新增「保存目录」`TextEdit` + 「浏览…」按钮（调用 `rfd::FileDialog::pick_folder()` 打开系统文件夹选择器）；默认 `D:\pic`；所选路径写入可执行文件同目录 `save_path.cfg`（`load_save_path`/`persist_save_path`），下次启动自动沿用。回主面板时通过 `flash` 字段显示「已保存截图：xxx」/失败信息。
- **控制面板 UI 分组优化**：`ControlPanel` 用两张带描边/圆角/阴影的卡片（`card()` 辅助函数）清晰区分「观察模式」（蓝，按钮「开始观察」，仅点击、无快捷键）与「截图模式」（青绿，含保存目录控件 + 按钮「开始截图」）；按钮改为填充强调色 + 白字 + 圆角，解决原两按钮过于突兀的问题。Idle 窗口尺寸调为 480×480。
- **悬浮面板提示增强**：截图态下 `FloatingPanel` 在原有「按 F9 结束截图」基础上新增「按 F10 截图」提示。
- **windows-rs 0.52 API 踩坑（已修，避免再踩）**：`PrintWindow`/`PRINT_WINDOW_FLAGS` 在 `Win32::Storage::Xps`（需加特性 `Win32_Storage_Xps`）；`GetWindowRect`/`BitBlt`/`DeleteObject`/`DeleteDC` 返回 `Result<()>`（`is_err()` / `let _=`），`PrintWindow` 返回 `BOOL`（`.as_bool()`）；`GetDC(hwnd)`（非 `Some(hwnd)`）；`GetLocalTime()` 直接返回 `SYSTEMTIME`（无出参）；`BITMAPINFOHEADER.biCompression` 是 `u32`（用 `0` 即 BI_RGB）。
- **状态**：`cargo build`（dev）通过，0 warning（12.8s，含 image/rfd 首次编译）。运行期行为（真实截图、文件夹选择、持久化、F10 连拍、毛玻璃观感）需真实 Windows + 游戏环境验证。

## 2026-09-26（续6）— 截图优化：剔除标题栏 / 激活 100% 可靠 / 成功反馈
- **剔除标题栏（仅截内容区）**：`capture_window_to_png` 改用 `GetClientRect` 取客户区尺寸 + `ClientToScreen` 求客户区左上角屏幕坐标，从整窗 `PrintWindow` 抓图中裁出内容区域，再 BGRA→RGBA 存 PNG。无边框/全屏游戏时偏移为 0，等同整窗。新增 `windows` 特性 `Win32_Graphics_Gdi`（`ClientToScreen` 已在该模块）、`Win32_System_Threading`、`Win32_Media_Audio`。
- **激活 100% 可靠（修复小概率失败）**：重写 `focus_window`，根因是 Windows「前台锁超时」——调用方非前台进程时 `SetForegroundWindow` 会被拒绝（仅任务栏闪一下）。修复：先用 `SetWindowPos(HWND_TOP, SWP_NOMOVE|SWP_NOSIZE|SWP_SHOWWINDOW)` 提 Z 序；再用 `AttachThreadInput(本线程, 前台线程, TRUE)` 临时绑定输入队列后 `SetForegroundWindow` 再解绑；最后 `GetForegroundWindow()` 二次校验 + 4 次 40ms 重试兜底。返回 `bool` 表示是否成功。截图前也会再 `focus_window` 一次，保证 DX 游戏被前台渲染。
- **成功反馈（毛玻璃轻量通知 + 提示音，二选一我做了双保险）**：
  - 毛玻璃通知：新增 `toast: Option<(String,bool,f64)>`，成功/失败时顶部居中弹出半透明圆角卡片（绿✓/红✗），带 0.3s 进场 + 0.4s 退场淡入淡出，1.8s/2.2s 后自动消失。`egui::Area` 覆盖层绘制，位于本程序窗口内，自动被 `WDA_EXCLUDEFROMCAPTURE` 排除，**不会污染游戏截图**。
  - 提示音：`win::play_capture_chime()` 运行时合成一段双音上行 WAV（880Hz→1318.5Hz，PCM16 单声道，6ms 淡入淡出防爆音），用 `PlaySoundW(PCWSTR::from_raw(...), SND_MEMORY|SND_NODEFAULT)` **同步**播放（不含 `SND_ASYNC`），函数返回即播完，缓冲区随之释放，无悬垂/无泄漏，零外部资源文件。
- **状态**：`cargo build`（dev）通过，0 warning（12.0s，含 `windows` 新增特性重编）。运行期（真实游戏下剔除标题栏、激活可靠性、毛玻璃通知、提示音）需真实 Windows + 游戏环境验证。

## 2026-09-26（续7）— 修复 F10 一次按键触发两次截图（重复保存根因修复）
- **根因（已定位并修复）**：`global-hotkey` 0.5.5 在 Windows 上，一次物理按键会派发 **两个事件**——`WM_HOTKEY` 时发送 `HotKeyState::Pressed`，随后后台线程轮询 `GetAsyncKeyState` 为 0 时再发送 `HotKeyState::Released`。原 `poll_hotkeys` 用 `while let Ok(event) = ...try_recv()` **无条件消费所有事件、完全未判断 `event.state`**，于是 F10 的 Pressed 与 Released 都进入 `capture_screenshot` → 一次按键保存两份 PNG（文件名秒级精度下覆盖同一文件，故磁盘上可能只见一个，但截屏/编码/写盘/提示音各跑两次；跨秒则出现两个文件）。
- **为何 F9 不暴露该缺陷**：F9 分支调用 `stop()`，其首行 `if self.mode == Mode::Idle { return; }` 把第二次（Released）调用吞成空操作，掩盖了"双事件"事实，使排查时易被误导。
- **修复（最小、根治，未废弃技术选型）**：在 `poll_hotkeys` 的 `while` 循环体首行新增 `if event.state != HotKeyState::Pressed { continue; }`，仅响应按下、忽略松开。新增导入 `use global_hotkey::HotKeyState;`（注意：`HotKeyState` 在 `global_hotkey` 顶层，**不在** `global_hotkey::hotkey`，编译曾因此报 E0432，已纠正）。
- **附带修正的行为**：从 `Idle` 按 F10 时，原 Pressed 进入截图态、Released 立即又截一次（一键启动+截图）；修复后 F10 从 Idle 仅进入截图态，再次按 F10 才截一张，符合「F10=激活窗口+进入截图 / 截图态再按 F10=抓图」的双角色设计，且无重复。
- **状态**：`cargo build`（dev）通过，0 warning（24.3s）。运行期（真实 Windows + 游戏）按 F10 应仅生成一份截图、播一次提示音；建议本机走一遍验证。

## 2026-09-26（续8）— 标签页拆分（开始观察 / 截图）+ 口径区域归档
- **需求**：将功能拆为「开始观察」「截图」两个标签页；截图标签页默认全屏截图；下拉列出 info.json 全部口径 key；选中口径后按各子弹 name/price 区域坐标裁剪，按「口径/子弹/」层级归档。
- **改动（src/main.rs）**：
  - 新增字段 `tab: usize`（0=观察 / 1=截图）、`info: Option<Value>`、`caliber_keys: Vec<String>`、`selected: usize`；`new()` 内 `load_info()` 读 info.json（搜索顺序：程序同目录 → 桌面 `C:\Users\Administrator\Desktop\info.json` → 当前目录），提取口径 key 列表。
  - 主面板新增标签页切换（`selectable_label`），拆为 `observe_tab` / `capture_tab` 两个独立渲染分支（仍共用 `ControlPanel` 容器，但与 `FloatingPanel` 悬浮面板绝不共用组件）。
  - 截图标签页：保存目录控件 + 口径 `ComboBox`（首项「全屏截图（默认）」，其后为全部口径）+ 「开始截图」按钮 + 即时提示文案。
  - `capture_screenshot` 重写：selected==0 → 全屏 `capture_window_to_png`；否则按 `info[caliber]` 遍历每个子弹，用 `rect_from_json` 取 name/price 的 `[x,y,w,h]`，从整窗缓冲 `crop_region` 裁剪，归档至 `保存目录/<口径>/<子弹>/name.png`、`price.png`。
  - 新增模块级 helper：`sanitize`（文件名清洗，替换 `\/:*?"<>|` 为 `_`）、`rect_from_json`（解析 [x,y,w,h]）、`load_info`（读 JSON）。
  - `FloatingPanel` 提示随 selected 变化（全屏→「按 F10 截取全屏」/ 口径→「按 F10 归档区域」）。
- **改动（src/win.rs）**：
  - 新增 `capture_window_rgba(hwnd) -> (Vec<u8>, w, h)`：整窗抓一次、返回 RGBA 缓冲（坐标基准 = 窗口左上角），供裁剪复用，避免重复抓屏。
  - 新增 `crop_region(rgba, w, h, [x,y,w,h]) -> Option<Vec<u8>>`：从整窗缓冲裁剪区域（越界夹紧）。
  - 新增 `save_rgba_png(dir, fname, rgba, w, h)`：写 PNG 并 `create_dir_all`。
  - `capture_window_to_png` 重构为调用以上三者（默认全屏入口）。
  - 移除不再使用的 `GetClientRect`/`ClientToScreen`/`POINT` 导入（统一为窗口左上角坐标基准）。
- **坐标约定**：info.json 的 `[x,y,w,h]` 视为相对「游戏窗口左上角」的设备像素；整窗抓图以此为准，保证区域裁剪落点正确。注意：默认全屏现在保存**整个窗口（含标题栏）**，与此前「剔除标题栏」行为不同——为与口径区域坐标基准一致而调整（区域坐标前提是窗口左上角原点）。
- **新增依赖**：`serde_json = "1"`。
- **编译坑（已修）**：format! 中字面 `{}` 须转义为 `{{}}`（已规避）；`GetLocalTime()` 为 unsafe，重构出原 `unsafe` 块后须 `unsafe { GetLocalTime() }` 包裹。
- **状态**：`cargo build`（dev）通过，0 warning（15.3s，含 `serde_json` 首次编译）。运行期（标签页、口径区域归档、按层级建目录）需真实 Windows + 游戏环境验证。

## 2026-09-26（续9）— 修复「未加载到 info.json」：配置文件项目化 + 加载逻辑重写
- **根因（双重）**：① 原 `load_info` 只在「exe 同目录 / 桌面固定路径 / 当前目录」找文件，info.json 并未作为项目受控文件放置，运行环境不在桌面目录即加载失败；② **原 `info.json` 本身含 17 处尾随逗号（非法 JSON）**，`serde_json` 严格拒绝解析 → 下拉恒显示「未加载到 info.json」。
- **改动（放置）**：将 `info.json` 作为项目受控配置文件复制到 `config/info.json`（项目根 `config/` 目录），不再依赖桌面/特定路径。
- **改动（清理）**：用正则 `,(\s*[}\]])` → `\1` 剔除原文件全部 17 处尾随逗号（不改变任何有效数据），经严格 JSON 解析校验通过（19 个口径顶层 key）。
- **改动（src/main.rs — 加载逻辑重写）**：
  - 新增常量 `CONFIG_NAMES`（候选文件名 `info.json`/`bullets.json`/`config.json`）、`CONFIG_DIR="config"`。
  - 新增 `config_candidates()`：从 `current_exe()` 所在目录**向上逐层追溯（最多 6 层）**，每层优先检查 `config/<name>`、其次同目录 `<name>`；并覆盖「当前工作目录」及其 `config/` 子目录。无论 `cargo run`（exe 在 `target/debug`）还是把 exe 放在项目下运行，都能稳定定位 `config/info.json`。
  - `load_info` 优先读取环境变量 `DELTA_OBSERVER_CONFIG` 指定的任意路径/文件名；否则遍历 `config_candidates()`，解析首个非空 JSON 对象。
  - 下拉未加载时的提示改为：「未找到口径配置：请确认项目 config/info.json 存在，或通过环境变量 DELTA_OBSERVER_CONFIG 指定配置文件路径。」（不再提桌面路径）。
- **验证（无需 GUI）**：Python 模拟 `config_candidates` 算法确认 exe 位于 `target/debug` 时解析到 `D:\workspace\owl\delta-trade-observer\config\info.json` 且 JSON 合法；`cargo build`（dev）通过，0 warning（20.1s）。
- **坑位记录**：手写/AI 生成的 JSON 极易带尾随逗号，`serde_json` 会静默失败导致配置加载报错；提交前务必用严格解析校验（如 `python -c "import json; json.load(open(f, encoding='utf-8'))"`）。
- **状态**：`cargo build`（dev）通过，0 warning。运行期下拉应正常列出全部口径（不再显示未加载错误）；本机运行 `target/debug/delta-trade-observer.exe` 即可确认。

## 2026-09-26（续10）— 修复「默认全屏截图又出现标题栏」
- **现象**：用户在窗口化运行的游戏下，按 F10 默认全屏截图（selected==0）保存的 PNG 又带上了窗口标题栏。
- **根因（非回归，是此前的有意为之）**：续8 为让口径区域坐标统一以「窗口左上角」为基准，把默认全屏保存改成写**整窗缓冲（含标题栏）**。当时约定「无边框/全屏游戏标题栏本就不存在」，但窗口化运行时标题栏会真实出现，被用户发现。
- **修复（最小、不破坏区域归档）**：在 `src/win.rs` 新增 `client_crop_rect(hwnd)` —— `GetClientRect` 取客户区尺寸，`ClientToScreen(0,0)` 求客户区左上角屏幕坐标，减去 `GetWindowRect` 窗口左上角屏幕坐标，得到客户区相对窗口左上角的 `[x,y,w,h]`；在 `capture_window_to_png` 保存前先用 `crop_region` 裁掉非客户区（标题栏）。底层 `capture_window_rgba` 整窗缓冲与原点不变，**口径区域归档（用整窗缓冲 + 窗口左上角坐标）逻辑完全不受影响**。
- **编译坑（已修）**：`ClientToScreen` 属 `Win32::Graphics::Gdi`（非 `WindowsAndMessaging`，首次放错位置报 E0432），已移至 Gdi 导入；并补回 `POINT` 导入。
- **DPI 说明**：程序 DPI-unaware，`GetClientRect`/`GetWindowRect`/`ClientToScreen` 均为逻辑像素，与 `PrintWindow` 缓冲尺寸一致，裁剪落点正确（常规缩放下）。
- **状态**：`cargo build`（dev）通过（仅一条无害的增量锁文件 warning，运行实例占用 exe 被 `taskkill` 释放后重编成功）。运行期建议本机窗口化游戏下按 F10 确认默认全屏不再含标题栏，同时用某口径做一次区域归档确认 name/price 落点仍正确。

## 2026-09-26（续11）— 新增「整窗标注图」复选框：口径截图额外保存带区域框的整窗截图
- **需求**：截图界面新增复选框。勾选且当前非默认截图（即选了具体口径、进行了自定义区域裁剪）时，额外把游戏窗口整窗截图保存进「子弹口径」目录（`保存目录/<口径>/<子弹>/`），并在该图上用矩形框标出本次裁剪选取的 name / price 区域范围。
- **改动（src/main.rs）**：
  - 新增字段 `annotate_full: bool`（默认 `false`），`new()` 初始化为 `false`。
  - 截图标签页下拉之后新增 `ui.checkbox(&mut app.annotate_full, "整窗标注图：口径截图时额外保存整窗截图，并框出 name / price 区域")`；`selected==0`（默认全屏）时附一行小字提示「该选项仅在选择具体口径时生效」。
  - `capture_screenshot` 口径分支：取 `let annotate = self.annotate_full;`，遍历每个子弹时，若 `annotate` 为真，则 `rgba.clone()` 整窗缓冲、`draw_rect_outline` 框出该子弹的 name（绿）/price（红）区域，存 `bdir/annotated.png`。区域归档（name/price 裁剪）逻辑不变，`count` 仅统计裁剪张数，标注图作为额外文件不入计数。
- **改动（src/win.rs）**：新增 `draw_rect_outline(rgba, w, h, rect, color, thick)`，在 RGBA 缓冲上以实心边框（厚度 `thick` 像素）画矩形，坐标 `[x,y,w,h]` 相对窗口左上角（与 info.json 一致，故作用在整窗缓冲上框选与口径坐标对齐），越界自动夹紧。
- **坐标一致性**：标注图用 `capture_window_rgba` 的原始整窗缓冲（**含标题栏**）绘制，因 info.json 区域坐标基准是「窗口左上角」，故框线落点正确（与默认全屏 `shot_*.png` 已剔除标题栏的行为不同，此处刻意保留标题栏以体现整窗上下文）。
- **状态**：`cargo build`（dev）通过（仅无害增量锁文件 warning；重编前 `taskkill /F /IM delta-trade-observer.exe` 释放被占用 exe）。运行期建议选一个口径勾选该复选框按 F10，确认 `保存目录/<口径>/<子弹>/` 下除 `name.png`/`price.png` 外多出 `annotated.png` 且两处区域框位置正确。

## 2026-09-26（续12）— 彻底杜绝标题栏进入任何截图（统一客户区捕获架构）
- **问题**：用户反馈 `annotated.png`（整窗标注图）仍把标题栏纳入截图范围。排查发现：该图此前直接保存 `capture_window_rgba` 的整窗缓冲（含标题栏），与「标题栏绝不能进截图」的硬性要求冲突；且各截图路径的坐标基准不统一，存在窗口状态下标题栏泄漏的隐患。
- **根因分析（全面排查所有截图场景）**：
  - 默认全屏 `capture_window_to_png`：已裁到客户区（续10），无标题栏 —— 安全。
  - 口径区域 `name.png`/`price.png`：从整窗缓冲按窗口左上角坐标裁剪；内容区本就在标题栏下方，正常无标题栏，但若坐标配置异常向上越界可能带上标题栏像素 —— 不够稳健。
  - `annotated.png`：直接存整窗缓冲 —— **确含标题栏，缺陷点**。
- **架构级修复（最稳健、容错最高）**：新增统一对外入口 `capture_client_rgba(hwnd) -> (client_buf, w, h, ox, oy)`：
  - 先 `PrintWindow` 抓整窗（DX 游戏最可靠，保留 `capture_window_rgba` 作底层源，坐标基准=窗口左上角、含标题栏）；
  - 再用 `client_crop_rect`（GetClientRect + ClientToScreen(0,0) − GetWindowRect 左上角）裁掉标题栏/边框，得到**客户区缓冲**，并返回客户区相对窗口的偏移 `(ox,oy)`；
  - 所有对外输出（默认全屏 + 口径区域 + 标注图）**全部经此入口**，调用方把 info.json 的 `[x,y,w,h]` 减去 `(ox,oy)` 后再裁剪/标注 → 标题栏在任何窗口状态下都绝不会进入截图；越界由 `crop_region`/`draw_rect_outline` 内部夹紧兜底。
- **配套改动（src/win.rs）**：
  - `crop_region` 返回值改为 `Option<(Vec<u8>, i32, i32)>`（返回实际裁剪宽高），消除「越界夹紧后尺寸≠原始 [w,h]」导致 PNG 错位/损坏的隐患（更稳健）。
  - `capture_window_to_png` 改用 `capture_client_rgba`（去掉原先内部重复裁剪）。
  - 补充各函数文档，明确「含标题栏=底层源、对外需经 capture_client_rgba 并减偏移」的契约。
- **坐标约定（最终定稿）**：info.json 区域坐标仍以「窗口左上角」为基准（便于用户量取）；运行时由 `capture_client_rgba` 的 `(ox,oy)` 重定基到客户区。无边框/全屏游戏偏移为 0，原坐标直接有效，无需重新量取。
- **窗口状态覆盖**：最大化（客户区=去边框内容）、最小化（focus_window 已 SW_RESTORE 还原）、全屏/独占全屏（无标题栏，偏移 0）、多显示器（用相对偏移，不依赖绝对屏幕坐标）、不同 Windows 版本（`GetClientRect`/`ClientToScreen`/`GetWindowRect` 跨版本稳定）—— 均通过裁客户区保证标题栏不进截图。
- **已知边界（非标题栏问题，记录备查）**：程序为 DPI-unaware，多显示器不同 DPI 时 GDI 坐标按进程虚拟 DPI 计算、与 PrintWindow 渲染尺寸一致，故裁剪仍对齐；真正的 per-monitor DPI 精确适配需另加 `SetProcessDpiAwareness` + 缩放换算，属更大改动，本次不涉及。client_crop_rect 在极端异常（GetClientRect 失败）时退回整窗，属理论兜底。
- **状态**：`cargo build`（dev）通过（仅无害增量锁文件 warning；重编前 `taskkill` 释放被占用 exe）。运行期建议：①窗口化游戏下按 F10 默认全屏，确认 `shot_*.png` 无标题栏；②选口径勾选「整窗标注图」按 F10，确认 `annotated.png` 也无标题栏且 name/price 框位置正确；③比较 `name.png`/`price.png` 与上一版坐标一致（重定基后应无偏移，除非之前就处于窗口化量取）。

## 2026-09-26（续13）— 修正第1点：坐标基准契约定为「客户区左上角」，删除错误的偏移重定基
- **问题（用户复盘第1点）**：裁剪结果与预期不符。排查确认：`capture_screenshot` 中存在 `rebase = |r|[r[0]-ox, r[1]-oy, …]`，把 `info.json` 当作「窗口左上角」基线后又减一次 `(ox,oy)`；而 `capture_client_rgba` 返回的已是客户区缓冲（原点＝客户区左上角），且用户确认坐标契约＝「无标题栏截图（客户区）左上角」。两者本应 1:1，多减一次偏移使窗口化（标题栏存在，`oy>0`）下每个区域整体上移 `oy`、左移 `ox`；全屏/`oy=0` 时等价，故 bug 在满屏下不显。
- **修复（src/main.rs + src/win.rs）**：
  - 删除 `capture_screenshot` 中的 `rebase` 闭包与所有 `rebase(...)` 调用，直接以 `info.json` 坐标 `crop_region` / `draw_rect_outline`（客户区缓冲）。
  - `capture_client_rgba` 返回值由 `(Vec<u8>,i32,i32,i32,i32)` 简化为 `(Vec<u8>,i32,i32)`，不再返回 `(ox,oy)`；`capture_window_to_png` 同步去掉 `_,_` 解构。
  - 同步更正文档：`rect_from_json`、各函数 doc、`PROJECT.md` 坑位 16/20/21/22 统一声明「客户区左上角」为唯一坐标系契约。
- **本质（专业视角）**：三类像素空间（窗口相对/客户区相对/原始位图）此前全用裸 `[i32;4]` 传递、基准只靠注释约定，导致静默错配可被编译通过地写出。根治方向：引入 `CoordinateSpace` 枚举 + 具名 `Rect/Offset/CapturedImage` 类型，使错配在类型层不可表达；坐标变换收敛到唯一纯函数 `convert` 并单测。
- **编译**：`cargo build`（dev）通过（仅无害增量锁文件 warning；重编前 `taskkill` 释放被占用 exe）。运行期建议：窗口化游戏下选口径按 F10，核对 `name.png`/`price.png`/`annotated.png` 落点正确（此前窗口化下会整体偏移 `oy`）。

- **续14 · 批量裁剪性能优化（当前）**：排查「同时裁剪多张图片缓慢」根因——原 `capture_screenshot` 在 egui **主线程串行**执行 PNG 编码（DEFLATE 压缩，CPU 密集），且勾选「整窗标注图」时每个子弹额外编码一张完整客户区 PNG（如 1920×1080≈8MB 原始像素），N 个子弹即 N 张全屏 PNG **顺序**压缩，并对整窗缓冲 N 次 `clone`；整段同步执行导致 UI 冻结数秒。无并行、无阻塞操作、无重叠，纯串行 CPU 瓶颈。
  - 优化方案（最低成本 / 最快）：① 捕获（`PrintWindow` 只能单次，无法并行）保留单次；② 把所有待保存图像收集为持有独立 `Vec<u8>` 的 Job；③ 按 `std::thread::available_parallelism()` 核心数**分批起线程并行编码+落盘**（纯 `std::thread` + `image::save_buffer`，**无新依赖**）；④ 整个「捕获+并行保存」移入**后台线程**，经 `mpsc::channel` 回传 `CaptureMsg`，主线程 `poll_capture_result` 每帧轮询写入 flash/toast 并播放提示音 → **UI 零卡顿**；⑤ 任务进行中忽略新的 F10 触发，避免并发写同目录。
  - 新增模块级纯函数 `run_capture`（捕获→收集 Job→分批并行编码保存→返回 `CaptureMsg`，不接触 egui）与字段 `capture_rx`；`capture_screenshot` 退化为「调度 + 立即返回」。
  - **正确性不变**：坐标契约（客户区左上角）、标题栏剔除、口径归档、`annotated.png` 框线位置逻辑均未改动。
  - **编译**：`cargo build`（dev）通过，0 warning。建议本机窗口化游戏下选口径（含标注图）按 F10，确认 `保存目录/<口径>/<子弹>/` 下 `name.png`/`price.png`/`annotated.png` 全部正确，且耗时显著下降、UI 不冻结。

- **续15 · 性能再优化：快速编码器 + 写时复制（当前）**：对续14 方案做进一步诊断，确认它**尚未是最优**——编码仍用 `image::save_buffer` 的默认 **zlib level 6**（高压缩、慢），且标注路径在每个子弹上 `buf.clone()` 整屏 N 次并持有全部副本（串行拷贝 + 峰值内存 ≈ N×整屏）。两项仍有显著优化空间。
  - **改进1（编码速度，影响最大）**：移除 `image` 直接依赖，改用 `png` 0.17 原生编码器，`encoder.set_compression(png::Compression::Fast)`（≈ level 1-3）。批量截图编码耗时通常下降数倍，文件体积仅温和增大——对本地截图归档完全可接受。`Cargo.toml` 中 `image = "0.25"` → `png = "0.17"`。`save_rgba_png` 签名与调用方不变。
  - **改进2（内存 + 拷贝）**：`run_capture` 改为「仅收集元数据」——新增 `Job`/`JobSpec`（`Crop{rect}` / `Annotate{name,price}`），大块像素缓冲由 `Arc<[u8]>` 共享持有（`Arc::from(buf.into_boxed_slice())`）；标注路径在每个工作线程内 `buf.to_vec()` **写时复制**仅克隆本张，峰值内存由 N×整屏降为约 cores×整屏；并预建目录避免每图重复 `create_dir_all`。
  - **维度对比（续14 vs 续15）**：时间复杂度编码步由「N 张 level-6 全屏 PNG / cores」降为「N 张 Fast 全屏 PNG / cores」（每像素编码常数更小）；空间由「N×整屏 + 小裁剪」降为「1×共享 + cores×整屏(仅标注并发) + 小裁剪」；吞吐量（编码）近线性提升；响应延迟（UI）本就 0 卡顿，现任务完成延迟进一步缩短。
  - **替代思路评估**：① `Compression::Level(0)`（不压缩）更快但文件极大、磁盘 IO 成新瓶颈，未采用；② 改用 `BMP` 免压缩——极快但体积失控，且破坏既有 PNG 约定，未采用；③ GPU/PNG 硬件编码——Rust 生态无成熟易用方案，性价比低；④ `rayon` 线程池——仅简化写法，对纯 std 分批已足够，未引入；⑤ 标注图「编码基类一次 + 每子弹叠加矩形」——PNG 不支持图层，仍须 N 次全屏重编码，无实质收益。**结论**：当前 `Fast` 压缩 + 写时复制 + 后台并行即为该工作负载下的最优现实方案。
  - **编译**：`cargo build`（dev）通过，0 warning（重编前 `taskkill` 释放被占用 exe）。注意 `png` 0.17 `Compression` **无 `BestSpeed` 变体**（误用报 E0599），最快档用 `Fast`；模块注释已记入 `PROJECT.md` 坑位 24。

- **续16 · info.json 全局去重重构（配置 schema 结构性变更）**：用户确认采用「全局去重」方案。经脚本分析确认：19 个口径全部共用同一套 3×3 网格（共 9 个唯一 `name/price` 槽位），且每个口径的子弹均按 slot 前缀 [0..n-1] 占用——故可安全抽取为单一数据源。
  - **新 schema**：顶层 `shared` = 按网格顺序（12 Gauge 的 slot0..8）排列的 9 个槽位 `{name:[x,y,w,h], price:[x,y,w,h]}`；每个口径 = `{ "bullets": [子弹名0, 子弹名1, …] }`，第 i 个子弹复用 `shared[i]`（`bullets[i]` ↔ `shared[i]`，按 JSON 排列顺序一一对应）。统一修改屏幕坐标只需改 `shared` 一处。
  - **Rust 配套改动**：`run_capture` 口径分支改为读 `shared[i]` + `bullets[i]`（新增格式），并保留「口径下每子弹直带 name/price」的旧格式兜底分支；`ObserverApp::new` 构建 `caliber_keys` 时**过滤掉顶层 `shared`** 键（否则下拉框会多出一个伪口径 "shared"）。`load_info` 注释同步更新为新 schema。
  - **附带修复**：重构前原 `info.json` 第 144 行存在**非法尾随逗号**（`serde_json` 会整体解析失败、程序加载不到配置），已借本次重写一并清洗（先用正则 `,(\s*[}\]])`→`\1` 容错解析再做结构化重写）。
  - **校验**：脚本从新结构反推每个口径 `bullets[i]` 的区域，与原数据逐项比对——19 口径 / 9 槽位 / 83 子弹，**0 处不等价**。原文件备份 `config/info.json.bak`。
  - **编译**：`cargo build`（dev）通过，0 warning。

- **续17 · 配置代码化：纯 JSON → `src/config.rs`（带逻辑）**：用户要求「忽略续16 的 JSON 改动」，把原来的纯 JSON 配置文件改为一个**带逻辑的代码文件**。据此撤销 `config/info.json` 的全局去重 schema，改为 Rust 模块 `src/config.rs`。
  - **核心设计**：以 `12 Gauge`（9 种子弹，覆盖完整 3×3 网格）作为**公共基准对象**——`pub const GAUGE12: [BulletRects; 9]`，持有 9 个网格槽位各自的 `name`/`price` 区域坐标；其余每个口径只需按「文件中书写的顺序」列出子弹名称（`insert_caliber(caliber, &[子弹名…])`），第 `i` 枚子弹**直接复用 `GAUGE12[i]` 的 name/price**，不再各自重复定义。统一改坐标只动 `GAUGE12` 一处。
  - **类型**：新增 `Rect = [i32; 4]`、`struct BulletRects { name, price }`、`type Config = HashMap<String, HashMap<String, BulletRects>>`、`fn build_config() -> Config`（带顺序复用的逻辑）。
  - **Rust 配套改动（src/main.rs）**：删除 `load_info` / `config_candidates` / `CONFIG_NAMES` / `CONFIG_DIR` / `rect_from_json` 等全部 JSON 读取逻辑；`ObserverApp.info` 由 `Option<Value>` 改为 `Arc<Config>`（构造时 `Arc::new(config::build_config())`，传入后台线程零成本克隆）；`run_capture` 直接 `info.get(&caliber)` 迭代 `(&子弹名, &BulletRects)`，不再有 `shared`/旧格式双分支；下拉口径由 `info.keys()` 派生；移除 `serde_json` 依赖（Cargo.toml）。`win.rs` 文档里对 `info.json` 的引用同步改为「代码配置（src/config.rs）」。
  - **副作用**：配置变为编译期常量，改坐标需重新编译（用户已知晓并接受）；运行时不再依赖外部 JSON 文件，`config/` 目录已删除。
  - **编译**：`cargo build`（dev）通过，0 warning。

- **续18 · 「开始观察」精确匹配游戏窗口 + 未命中启动 WeGame**：用户要求点击「开始观察」按钮后，先精确判定游戏窗口——窗口标题**或**进程名**完全等于「三角洲行动」**（必须全名匹配，不支持模糊/包含），命中则激活并置于前台；未命中则通过命令行启动 WeGame。
  - **win.rs 新增**：
    - `find_game_window(target: &str)` + 回调 `enum_find_exact` + 上下文 `ExactSearchCtx`：枚举可见窗口，逐个做 `title == target` 或 `进程主名(去目录/去 .exe) == target` 的**完全相等**判定（与既有 `find_window_by_keyword` 的「包含」语义区分开）。
    - `process_name(hwnd)`：`GetWindowThreadProcessId` → `OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)` → `QueryFullProcessImageNameW` 取完整镜像路径，再 `Path::file_stem()` 得主名。
    - `launch_wegame()`：依次尝试 `ProgramData/ProgramFiles/ProgramFiles(x86)/LOCALAPPDATA/APPDATA` 下的 `Tencent\WeGame\wegame.exe`，命中即 `spawn`；全部未命中回退 `cmd /C start wegame:`（系统协议）。
  - **main.rs 改动**：`start()` 按 `kind` 分支——`Observing` 走 `find_game_window`（精确，未命中即 `launch_wegame()` 并提示、不进激活态）；`Screenshot` 维持原 `find_window_by_keyword`（包含匹配）行为不变。仅按钮「开始观察」受本次精确匹配影响，截图按钮逻辑不变。
  - **新增导入**：`windows::core::PWSTR`；Foundation 的 `CloseHandle`；Threading 的 `OpenProcess/PROCESS_NAME_WIN32/PROCESS_QUERY_INFORMATION/PROCESS_VM_READ/QueryFullProcessImageNameW`。
  - **编译**：`cargo build`（dev）通过，0 warning。

- **续19 · 删除 WeGame 逻辑 + 观察态接入 OCR 抓取与结构化持久化**：用户要求删除全部 WeGame 相关逻辑，并把「开始观察」改为直接聚焦《三角洲行动》窗口；随后循环检索各口径、模拟人类输入触发游戏列出子弹、OCR 识别名称/价格并结构化落盘。
  - **删除 WeGame**：`win.rs` 移除 `find_game_window`/`enum_find_exact`/`ExactSearchCtx`/`process_name`/`launch_wegame` 及对应导入（`PWSTR`/`CloseHandle`/`OpenProcess`/`PROCESS_NAME_WIN32`/`PROCESS_QUERY_INFORMATION`/`PROCESS_VM_READ`/`QueryFullProcessImageNameW`）；`main.rs` `start()` 的 `Observing` 分支改为直接 `find_window_by_keyword(GAME_KEYWORD)` + `focus_window`，不再有「未命中启动 WeGame」分支。
  - **新增 `src/ocr.rs`（WinRT OCR，零外部依赖）**：`recognize_words(rgba,w,h)` 调 `OcrEngine::TryCreateFromLanguage`——优先 `zh-CN`（中文语言包），失败回退 `en-US`（数字/拉丁仍可用，价格识别不受影响）；RGBA→BGRA 后写 `SoftwareBitmap` 后备缓冲，遍历 `Lines→Words` 取词与 `BoundingRect`。
  - **新增 `src/observe.rs`（观察主流程）**：`run_observation(save_path, info, stop, tx)` 在后台线程——① 定位+聚焦游戏窗口；② 对每个口径调用 `win::type_into_search` 清空检索栏并输入口径名、回车；③ 700ms 后 `capture_client_rgba` 整屏 OCR；④ 按 `config.rs` 每子弹 `name`/`price` 矩形用「词中心落格」过滤分别取名称/价格，价格经 `extract_price` 抽取最长 `0-9., ` 段；⑤ 写 `save_path/prices/records.jsonl`（每行 `{"caliber","bullet","ocr_name","price","ts_ms","timestamp"}`）+ 每子弹 `name.png`/`price.png` 调试裁剪图与 `ocr.txt`；`stop` 支持 F9 优雅中止，进度/完成经 `mpsc` 回传主线程。
  - **win.rs 新增模拟输入三函数**（`Win32_UI_Input_KeyboardAndMouse`）：`client_to_screen`（客户区坐标→屏幕坐标）、`mouse_click_screen`（SetCursorPos + SendInput 左键下/上，带 ±2px 抖动）、`type_into_search`（点击检索栏中心 → Ctrl+A → Backspace 清空 → 逐字符 Unicode 输入口径名 → Enter）。检索栏坐标 `observe.rs::SEARCH_BAR=[100,160,170,20]` 为相对客户区左上角。
  - **main.rs 接线**：`mod observe/ocr`；`ObserverApp` 增 `observe_rx/observe_stop/observe_progress`；`start(Observing)` 进入激活态后调 `start_observation`（spawn 线程）；`poll_observe` 收进度更新悬浮面板、收完成写提示并复位；F9 在观察态先 `observe_stop.store(true)` 再 `stop()`；悬浮面板显示「当前口径（i/N） / 已识别条数」。`GAME_KEYWORD` 上提为 `config.rs` 公共常量（main/observe 共用）。
  - **Cargo.toml 补特性**：`Foundation`、`Foundation_Collections`、`Graphics_Imaging`、`Globalization`、`Media_Ocr`、`Win32_System_WinRT`、`Win32_UI_Input_KeyboardAndMouse`（保留既有基础特性）。
  - **坐标契约（关键，待校准）**：`SEARCH_BAR` 与 `config.rs` 网格坐标均为「客户区/无标题栏截图」左上角基准（与 `capture_client_rgba` 一致）。二者由用户凭记忆给出，**未经真实游戏截图校准**——检索栏实际位置、各口径子弹是否落在 GAUGE12 对应槽位均需在真实环境用截图核验（用户已主动提出可提供游戏截图作参考）。
  - **windows-rs 0.52 踩坑（避免再踩）**：① `SendInput(pinputs: &[INPUT], cbsize: i32)` 收切片（非 3 参）；② `SetCursorPos` 在 `Win32::UI::WindowsAndMessaging`（非 KeyboardAndMouse），返回 `Result<()>`；③ 字段是 newtype：`KEYBDINPUT.wVk: VIRTUAL_KEY`、`dwFlags: KEYBD_EVENT_FLAGS`（`0` 不能直赋，用 `KEYBD_EVENT_FLAGS(0)`）；④ `cast` 在 `ComInterface` trait（非 `Interface`）；⑤ WinRT 方法按特性门控：`RecognizeAsync`/`CreateReference` 需 `Foundation`+`Graphics_Imaging`，`Lines`/`Words` 需 `Foundation_Collections`，`TryCreateFromLanguage` 需 `Globalization`——缺特性会报「method not found」而非 import error。
  - **编译**：`cargo build`（dev）通过，**0 warning**（链接期曾因上一轮运行的 exe 锁文件报 `拒绝访问`/`LNK1104`，`PowerShell Stop-Process` 杀掉后重编通过）。
  - **验证**：沙箱无显示器/无游戏，仅完成编译验证；运行期行为（聚焦、模拟输入、OCR 精度、坐标对齐、持久化）需真实 Windows + 已启动游戏环境核验。

- **续20 · 截图实测标定 + 逐单元格双引擎 OCR + 多轮循环（间隔可调）+ 真人化键鼠**：用户提供 12 Gauge 检索结果实测截图（shot_20260927_115923，1080×608 客户区，与 capture_client_rgba 同基准），据此完成坐标标定、OCR 方案定型与循环观察。
  - **坐标标定（config.rs）**：GAUGE12 全部 9 格 name/price 按实测重标——网格列起点 x≈249/513/776、名称行 y≈87/179/270、价格行 y≈154/245/336、价格右对齐至列起点+246；新增 `CALIB_REF_W=1080`（标定参考宽度，运行时按实际客户区宽度**等比缩放** `scale_rect`，适配任意分辨率/DPI）；`SEARCH_BAR=[44,85,176,28]`（口径过滤输入框）由 observe.rs 上提至 config.rs 统一管理。
  - **OCR 方案定型（关键发现）**：实测价格形如 `440`/`1.920`——`.` 是**千分位分隔符**而非小数点，`extract_price` 改为取最长 `[0-9., ]` 段后**剔除全部分隔符仅留数字**；价格文字为白字深底 + 左侧金黄硬币图标 → `ocr.rs` 新增预处理管线「**近白二值化**（min(RGB)>150 且 max-min<70 → 黑字白底，天然排除深色背景与硬币图标）→ **4× 最近邻放大**（UPSCALE=4，12px 级小字放大后识别率显著提升）」；改为**逐单元格裁剪分别 OCR**（不再整屏识别）：名称格 `zh-CN` 引擎、价格格 `en-US` 引擎，主引擎缺失自动回退；调试落盘新增 `*.price.bin.png`（二值化放大图）供阈值/裁剪框校准。
  - **多轮循环（observe.rs）**：`run_observation` 增 `interval_min` 参数——每轮遍历全部口径识别完成后等待 N 分钟再开始下一轮（每秒检查 stop 并回传 `ObserveMsg::Waiting{cycle,remain_sec}`）；`Progress` 增 `cycle` 字段；连续 3 次截图失败自动终止防死循环。
  - **控制面板（main.rs）**：「开始观察」标签页新增「循环间隔（分钟）」DragValue 输入框（默认 30，0~1440，clamp_range）；悬浮面板显示「第 N 轮 · 口径 X（i/total）」「已识别 n 条」「第 N 轮完成，M:SS 后开始下一轮」；FLOAT_SIZE 168→208。egui 0.27 踩坑：DragValue 用 `.clamp_range`（`.range` 是 0.28+ 才有）。
  - **真人化键鼠（win.rs）**：新增 xorshift64* 伪随机源（`RNG`/`rnd_range`/`rnd_sleep_ms`）；新增 `move_mouse_human`——smoothstep 缓动骨架 + 每步「平滑随机游走」偏移（相邻步相关，非完美贝塞尔/白噪声，幅度随进度收敛）+ 步间 4~14ms 随机延时 + 末端 ±2px 落点抖动；`mouse_click_screen` 改为先人类化移动、点击前 40~120ms 随机延时、按下保持 25~70ms 随机；`type_into_search` 键入节奏随机化（45~150ms 普通间隔 + 8% 概率 150~320ms「犹豫停顿」），Ctrl+A/Backspace/Enter 各步骤延时全部随机化；`send_unicode_char` 按下→抬起间隔 6~18ms 随机。
  - **编译**：`cargo build`（dev）通过，0 warning（再次遇到运行中 exe 锁文件致链接失败，用 Stop-Process 结束进程后重编通过）。
  - **验证**：编译级验证通过；标定坐标在 1080 宽客户区下应像素级吻合（截图即该分辨率），其他分辨率靠等比缩放适配；OCR 实际识别率、检索输入→列表刷新联动需真实环境跑一轮后用落盘的 `*.price.bin.png`/`ocr.txt` 核验调参。

- **续21 · 独立「行情看板」可视化 Tab（产品经理视角设计）**：新增 `src/viz.rs`，主面板新增第三个标签页「行情看板」（窗口自适应放大至 960×680），解析 `prices/records.jsonl` 持久化记录做数据可视化。
  - **数据加载**：`viz::load` 极简解析 JSONL（不引入 serde，按固定键名抽取 caliber/bullet/price/ts_ms），按子弹 key 构建时间升序序列、latest/prev、口径顺序；`ObserverApp::viz_ensure` 按文件 mtime 懒加载（变更才重解析），「↻ 刷新」按钮置 `viz_reload_requested` 强制重载。
  - **视图一 · 全局概览卡**：跟踪子弹数、上涨/下跌/持平(新)分布、最近更新时间——给用户「全盘面涨跌态势」一句话结论。
  - **视图二 · 最新价格概览**：按口径分组卡片网格（响应式列数 1~4），每卡含大字当前价 + ▲/▼ 差值与幅度（**涨红跌绿**，A 股习惯）+ 近 30 点迷你火花线 + 更新时间；顶部下拉筛选「全部口径 / 指定口径」——核心指标：单品当前价与环比涨跌。
  - **视图三 · 价格波动趋势**：口径 + 子弹双下拉 + 「一天/一周/一个月」分段切换（窗口固定 [now-span, now]）；手动用 `Painter` 绘制——Y 轴价格刻度 4 段、X 轴时间刻度 5 段（一天/周显 HH:MM，一月显 MM-DD）、**每段连线按方向着色（涨红/跌绿）**、虚线标注区间最高(红)/最低(绿) + 最新点圆点 + 右端价签；上方统计卡含当前/区间最高/最低/平均/区间涨跌幅——核心指标：跨时间维度的价格趋势、极值与拐点。
  - **交互/层级**：头部标题 + 刷新；自上而下「全局→分组→单品→趋势」信息收敛；图例说明着色语义。
  - **踩坑（egui 0.27）**：`allocate_painter` 返回 `(Response, Painter)`，`Response.rect` 是**字段**非方法；`fmt_price` 返回 `String` 传 `&str` 参数需 `&`；`from_rgba_unmultiplied` 不可用于 `const`（改函数）；`show` 内刷新按钮闭包原捕获 `app` 与 `data/view` 借用冲突，改用局部 `reload_req` 标志末尾回写。
  - **编译**：`cargo build`（dev）通过，0 warning。
  - **验证**：编译级验证通过；图表渲染与交互需真实数据（多轮观察后的 records.jsonl）跑一遍核验布局与着色。

- **续22 · 新增「测试观察」按钮 + 透明可穿透叠层实时标注**：在「开始观察」旁新增「测试观察」按钮，点击后聚焦《三角洲行动》窗口、**不检索口径**，仅读取当前屏幕 3×3 网格的子弹名称与价格，在主界面实时展示，并在游戏窗口上方叠加透明、可鼠标穿透的浮层实时标注价格。
  - **新增 `src/overlay.rs`（透明穿透叠层）**：独立 Win32 分层窗口（`WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW`）。`ensure()` 进程级单例（`OnceLock` + 消息泵线程保活窗口）；`set_labels(labels, cx, cy, cw, ch)` 用 GDI 在 32 位 DIB 上以透明底写文字、再 `UpdateLayeredWindow` 提交 —— 文字像素 alpha=255、背景 alpha=0，天然实现「仅标签可见、其余全透明」；`hide()` 隐藏。`OverlayLabel{x,y,text,big}` 坐标直接取已缩放的 GAUGE12 矩形，由本模块定位到客户区屏幕坐标（`client_to_screen(0,0)`）。
  - **新增 `src/test_observe.rs`（测试观察逻辑）**：`run_test_observation(stop, tx)` 聚焦游戏后**不输入、不持久化、不循环多轮**，直接循环抓取屏幕 3×3 网格（GAUGE12 共享网格坐标，按实际客户区宽度等比缩放 `scale_rect`），逐格 OCR 名称(zh-CN)/价格(en-US)、`extract_price` 抽数字；约每 0.8s 回传「名称+价格」给主线程并驱动叠层实时标注；识别不出的格既不在主界面也不在叠层出现（即「识别不出就不显示」）。`observe.rs` 的 `scale_rect`/`ocr_crop`/`extract_price` 提为 `pub` 复用。
  - **main.rs 接线**：「开始观察」旁新增「测试观察 / 停止测试」切换按钮（仅鼠标点击，无快捷键）；observe_tab 新增 3×3 实时网格结果区（第 1~9 格，名称+价格，未识别显示「—」）；`ObserverApp` 增 `test_rx/test_stop/test_running/test_result`，`poll_test` 收消息更新界面、`start_test`/`stop_test` 启停后台线程；主面板包 `ScrollArea` 并放大至 600×700 容纳结果区。
  - **叠层视觉语义**：价格大字亮黄、名称小字浅蓝，均带半透明深底框保证在任意游戏背景可读；窗口透明且 `WS_EX_TRANSPARENT` → 鼠标点击穿透到下方游戏，不拦截操作。
  - **踩坑（windows-rs 0.52，避免再踩）**：① `TextOutW`/`GetTextExtentPoint32W` 收 `&[u16]`（非 PCWSTR+长度）；② `CreateDIBSection` 返回 `Result<HBITMAP>`；③ `WNDPROC` 是类型别名（直接 `Some(wndproc)` 赋值，非 `WNDPROC(...)` 构造）；④ `UpdateLayeredWindow`/`ULW_ALPHA` 在 `Win32_UI_WindowsAndMessaging`（非 Gdi）；⑤ `HMENU` 在 `WindowsAndMessaging`（非 Foundation）；⑥ 句柄 newtype 细节：`HWND` 内部为 `isize`、`DEFAULT_CHARSET` 等常量内层为 `u8`（转 `u32`）、`AC_SRC_OVER`/`AC_SRC_ALPHA` 为 `u32`（转 `u8`）、`ULW_ALPHA` 为 `UPDATE_LAYERED_WINDOW_FLAGS`（不可 `as`）；⑦ `CreateFontW` 参数顺序为 cHeight,cWidth,cEscapement,cOrientation,cWeight,**bItalic(u32),bUnderline(u32),bStrikeOut(u32)**,iCharSet,iOutPrecision,iClipPrecision,iQuality,iPitchAndFamily,pszFaceName；⑧ `CreateWindowExW` lpParam 收 `Option<*const c_void>`（传 `None`），`WNDCLASSW.style` 类型为 `WNDCLASS_STYLES`。
  - **编译**：`cargo build`（dev）通过，0 warning（LNK1104 exe 锁文件仍用 `Stop-Process` 杀进程后重编）。
  - **验证**：编译级通过；叠层定位与实时标注、OCR 准确率需真实 Windows + 已启动游戏环境跑一遍核验（已知风险：DPI>100% 时本进程若 DPI 上下文与游戏不同，叠层坐标可能存在偏移）。

## 2026-09-27（续23）— 测试观察九格结果迁移至毛玻璃浮层 + 移除透明叠层
- **需求变更**：① 把「测试观察」结果从主控制面板（ControlPanel）移除；② 改为在毛玻璃悬浮面板（FloatingPanel）上以九格布局实时展示；③ 既然结果已在毛玻璃上，原先盖在游戏窗口上方的透明可穿透独立叠层（overlay）不再需要，一并移除，恢复游戏窗口的正常层叠显示。
- **新增 `CaptureKind::Testing`**：「测试观察」按钮（位于「开始观察」旁，仅点击触发）改为 `app.start(CaptureKind::Testing)` 进入激活态（与观察/截图一致走毛玻璃浮层 + DWM 毛玻璃 + 鼠标穿透 + 截图排除），按 F9 结束。`start()` 按 kind 选择窗口尺寸：Testing 用更大的 `TEST_FLOAT_SIZE = (404,392)` 容纳九格网格；并据此启动测试后台线程（`start_test`）。
- **FloatingPanel 分支渲染**：`show` 按 `app.mode` 分派——`Active(Testing)` 走新增 `test_grid`（标题「测试观察（实时）」+「● 运行中」+ 错误行 + 3×3 网格，每格含「第 N 格」小标签、名称小字、价格 18px 蓝字，未识别显示「—」）；其余态走 `status_body`（原观察/截图状态文案）。九格卡片用半透明白底圆角子卡，保证在毛玻璃模糊背景上清晰可读。
- **移除 `src/overlay.rs` 与全部 overlay 引用**：删除 `mod overlay`；`test_observe.rs` 去掉 `ensure()/OverlayLabel/set_labels/hide` 及 `client_to_screen/client_crop_rect` 对叠层的依赖（坐标缩放直接复用 `capture_client_rgba` 返回的客户端宽度 `w`，与原 `cw` 等价）；`redraw_and_wait` 改为纯 `wait_or_stop(stop, ms)`（仅阶段性检查中止）。删除 `stop_test` 方法，`stop()` 内统一对 `test_stop` 置位中止（与 `observe_stop` 并列），避免悬挂后台线程。
- **主面板简化**：`observe_tab` 移除结果区与 `ControlPanel::test_result_grid`；说明文案改为「测试观察……在毛玻璃浮层上以九格布局实时展示（按 F9 结束）」。`test_result`/`test_rx`/`test_stop`/`test_running` 字段保留，仅供浮层实时渲染与通道轮询。
- **编译**：`cargo build`（dev）通过，0 error / 0 warning。
- **验证**：编译级通过；九格在毛玻璃上的布局清晰度、OCR 实时准确率、F9 结束是否干净复位，需真实 Windows + 已启动游戏环境跑一遍核验。移除 overlay 后游戏窗口层叠恢复正常（不再有独立分层窗口压在游戏上方）。

## 2026-09-27（续24）— 测试观察面板扩容（≥3×宽）+ 可拖动 + 数字识别链路落地

- **面板扩容**：`TEST_FLOAT_SIZE` 扩至 `(1240, 360)`，宽度 ≥ 原 3 倍（1240 ≥ 404×3≈1212），3×3 九格同屏更宽松。
- **可拖动**：
  - `start()` 的 `MousePassthrough` 改为按态下发：仅 Testing 态关闭穿透（面板可接收鼠标、可拖动），Observing/Screenshot 保持穿透不遮挡游戏操作。
  - `FloatingPanel::test_grid` 标题行接入 OS 级拖动：`Response::interact(Sense::drag())` + `drag_started()` → `ViewportCommand::StartDrag`（egui 0.27 / egui-winit 0.27.2 原生支持，Windows 走 ReleaseCapture + WM_NCLBUTTONDOWN 拖动循环）；标题行右侧加「⠿ 按住标题栏可拖动」提示，悬停光标 Move。
  - 移除无实际使用的 `float_pos` 字段（原为手动 `OuterPosition` 增量拖动预留；egui 指针坐标是窗口相对系，窗口一动指针增量即被抵消，手动拖动存在反馈死锁——OS 级 StartDrag 才是正解）。
- **数字识别链路（digit 模块）落地**：
  - `src/digit.rs` 精简为单一价格入口 `digit::recognize_price(rgba,w,h,rect)` = 裁剪 → `ocr::recognize_price`（颜色感知二值化：保留任意颜色明亮文字 + 排除金币图标 → 去斑点 → 6× 放大 → en-US 引擎 → `extract_price` 纯数字）。
  - `test_observe.rs` 与 `observe.rs` 的价格格统一改走该链路。数字识别「极度不准确」的根因：原通用 `ocr_crop(en-US)` 走 near-white 二值化，彩色价格文字（亮黄/金/浅蓝）不满足「近白」判据，被整片抹成背景 → OCR 看到空白。
  - **回滚半成品 `dl` feature**：`ort = "2"` 无法解析（crates.io 仅有 2.0.0-rc.x 预发布版），阻塞所有构建；按「先优化现有 OCR，仍不达标再上深度学习」策略，移除 `[features] dl` 与 ort/ndarray 依赖及 digit.rs 内 cfg 门控的 ONNX 后端，保留可插拔接缝（后续需要时以 `ort = "2.0.0-rc.x"` 显式预发布版本接入）。
- **踩坑**：
  - `ort` 2.x 未正式发布，`version = "2"` 在 sparse index 直接解析失败 → 预发布依赖必须显式写 `2.0.0-rc.x`。
  - egui 0.27 `Response::on_hover_cursor` 按值消费 `self` → 必须放在 `drag_started()` 判断之后调用，否则 E0382。
  - LNK1104 复现（旧 exe 运行中锁输出文件）→ `Stop-Process` 终止后重编。
- **编译**：0 error / 0 warning。
- **验证**：需真实环境核验：① 标题行按住拖动手感；② 九格数字识别准确率（对比旧版通用链路）；③ Testing 态面板覆盖区域会拦截游戏点击（穿透关闭的设计使然，拖开即可）。

## 2026-09-30 — 截图模块独立化重构（src/screenshot.rs）
- **需求**：将散落在 `win.rs` 的截图逻辑抽为独立模块，提升可读性与可维护性，修复潜在问题，保留区域 / 窗口 / 全屏截图核心能力，与 `observe` / `digit` 等调用方兼容。
- **改动**：
  - 新建 `src/screenshot.rs`：承接 `win.rs` 全部截图函数 —— `capture_window_rgba`（整窗抓屏）、`capture_client_rgba`（客户区统一入口，裁标题栏）、`crop_region`（区域裁剪）、`save_rgba_png`（PNG 保存）、`draw_rect_outline`/`draw_digit`（标注）、`client_crop_rect`（客户区偏移）；并新增类型化 `CapturedImage`（`crop`/`draw_rect_outline`/`draw_digit`/`save_png`/`raw`/`dimensions` 方法）作为现代接口面。
  - `win.rs` 删除上述截图函数，仅保留窗口查找 / 聚焦 / 截图排除 / 毛玻璃 / 模拟键鼠 / 提示音；import 精简（移除仅截图用的 `windows` 符号与 `Storage::Xps`）。
  - `observe.rs`/`digit.rs` 截图调用前缀 `win::` → `screenshot::`；`digit.rs` 的 `use crate::win` 改为 `use crate::screenshot`。`main.rs` 增加 `mod screenshot;`。
  - 文档注释同步（`config.rs`/`ocr.rs`/`template_match.rs` 中 `win::crop_region`/`win::capture_client_rgba` 引用改为 `screenshot::`）。
- **修复的潜在问题**：
  1. **底层抓屏隐式抢焦点**：原 `capture_window_rgba` 内部调用 `focus_window(hwnd)`，导致观察主循环每帧截图都抢占前台焦点、干扰用户操作。改为不再隐式激活，聚焦由调用方（`observe`）显式完成。
  2. **`save_rgba_png` 注释与实现不符**：原注释写 `Compression::BestSpeed`（zlib level 1），实际用 `Compression::Fast`（`png` 0.17 无 `BestSpeed` 变体）。修正注释，明确最快档即 `Fast`。
  3. **坐标类型化**：裸 `[i32;4]` 截图坐标改用模块内 `Rect` 类型别名（`pub type Rect = [i32;4]`，与 `config::Rect` 同源），从类型层降低错配风险（旧实现曾因裸数组静默错配导致裁剪偏移）。
  4. **裁剪夹紧对称化**：`crop_region`/`draw_rect_outline` 的起点/终点夹紧改为对称 `.min(尺寸)`，消除原 `x0` 用 `.min(w-1)` 的不对称写法，行为等价、更易读。
- **编译**：`cargo build` 通过，**代码相关 0 warning**（仅一条无害的增量编译锁文件删除提示 `os error 5`，系上轮残留进程持有，与代码无关）。运行期行为（截图 / 裁剪 / 标注 / 观察识别）需真实 Windows + 已启动游戏环境核验。

## 2026-09-30（续）— 截图几何修复：DPI 感知 + DWM 扩展边界剔除阴影
- **需求**：解决缩放 ≠ 100% 下的截图尺寸错位 / 黑边（典型表现为 1934×1117 而非干净的 1920×1080，多出的 14×37 主要来自 DWM 窗口阴影与标题栏）。
- **根因（按排查顺序）**：
  1. **DPI 未感知**：进程原为 DPI-unaware，`GetWindowRect`/`GetClientRect`/`ClientToScreen` 返回虚拟逻辑坐标，与 `PrintWindow` 实际渲染尺寸在缩放下不一致 → 尺寸错位。
  2. **`GetWindowRect` 含 DWM 阴影**：`capture_window_rgba` 用 `GetWindowRect` 定尺寸，把左右各 ~7px 阴影、标题栏算进来 → +14×+37 主因。
  3. **抓屏 DC 原点错位**：原用 `GetDC(hwnd)`（客户区 DC，原点在客户区左上）建整窗尺寸位图并 `PrintWindow` 渲染整窗，坐标原点与尺寸不匹配，进一步放大错位。
- **改动**：
  - `main.rs` 最早调用 `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`（需在创建任何窗口前），`Cargo.toml` 补 `Win32_UI_HiDpi`。使全部坐标 API 与 `PrintWindow` 一致以设备像素取舍。
  - 新增 `window_visible_rect(hwnd)`：优先 `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` 取**可见窗口**（剔除 DWM 阴影），失败回退 `GetWindowRect`。
  - `capture_window_rgba` 改用 `window_visible_rect` 定尺寸 + `GetWindowDC`（整窗 DC，原点=窗口左上角），整窗从 (0,0) 渲染、与缓冲严格对齐。
  - `client_crop_rect` 基准改为同一 `window_visible_rect`（不再用 `GetWindowRect`），客户区偏移与抓屏缓冲坐标系一致，裁剪落点正确。
- **排除的误判**：目标游戏若为无边框全屏，`window_visible_rect` 与客户区重合（偏移 0），此修复不影响既有全屏路径；仅在有阴影/标题栏的窗口态下消除黑边。
- **编译**：`cargo build` 通过，**0 warning**。运行期需真实 Windows（缩放 125%/150% 多显示器）截一张图核验是否得到干净 1920×1080。

## 2026-10-01（续）— OCR 测试 Tab + 识别逻辑共享模块
- **需求**：新增「OCR 测试」Tab——选本地交易行截图 + 选口径 → 复用「开始观察」的识别逻辑识别子弹名称与价格并汇总预览；同时把子弹识别提取为共享模块，UI 与业务逻辑分离、各 Tab 独立。
- **新增 `src/recognition.rs`（核心共享模块）**：
  - `BulletRecognition`：持有 `ocr::NameOcr`（名称 OCR，引擎缓存复用）+ `digit::PriceRecognizer`（价格逐位模板匹配），构造一次整周期复用；`recognize_bullet(frame, rects)` 内部按帧宽推导 `Scale` 并识别单枚子弹，返回 `BulletRec{name, price}`。
  - `recognize_caliber(frame, caliber, cfg)`：遍历该口径全部子弹槽位返回 `Vec<(子弹名, 标定坐标, BulletRec)>`，供 OCR 测试 Tab 直接调用；「开始观察」逐单元格逻辑也复用 `recognize_bullet`。**零重复实现**。
- **`screenshot.rs`**：
  - 新增 `CapturedImage::from_file(path)`：`image::open` 解码本地 PNG/JPG → RGBA，使测试 Tab 复用同一套坐标与识别。
  - 把标注颜色常量（`DIGIT_BOX_COLORS`/`NAME_BOX_COLOR`/`DIGIT_BOX_THICKNESS`/`DIGIT_LABEL_LIFT_PX`）提升为 `pub`，`observe.rs` 与 OCR 测试标注预览共用（消除重复字面量）。
- **`observe.rs`**：移除散装的 `name_ocr`/`price_recognizer` 与手写每子弹识别块，改调 `BulletRecognition::recognize_bullet`；调试帧颜色引用改为 `screenshot::*` 共享常量。
- **`main.rs`**：Idle 态加顶部 Tab 栏（`MainTab::{Observe, OcrTest}`）；新增 `OcrTestPanel`（独立组件，仅采集输入 + 展示：图片路径输入框 / 拖拽 / 口径 ComboBox / 识别按钮 / 标注预览图 + 汇总表），识别逻辑全部委托 `crate::recognition::recognize_caliber`，不触碰识别细节。
- **编译**：`cargo build` 通过，**0 error / 0 warning**（仅一条无害增量编译锁文件删除提示 `os error 5`，系上轮残留进程持有，与代码无关）。运行期需真实 Windows 环境：选一张交易行截图 + 口径点「识别」核验预览标注与价格汇总。

## 2026-10-01（续二）— OCR 测试 Tab：价格浏览滑块 + 逐位数字模板匹配可视化
- **需求**：OCR 测试 Tab 增加「价格滚动条」浏览不同子弹价格；切换时展示每个数字模板的匹配图片与匹配结果（分数 / 识别出的数字），直观查看模板匹配效果。
- **`template_match.rs`**：
  - 新增 `DigitMatchDetail { scores:[f32;10], best_digit:Option<u8>, best_score:f32 }` 与 `match_digit_detail(rgba,w,h)`：返回与 0~9 十个模板各自的 NCC 分数（不做事后阈值截断），供可视化。
  - 删除被取代的 `recognize_digit_image`（已无调用方），模块/函数文档同步改指 `match_digit_detail`。
- **`digit.rs`**：
  - 新增公开 `DigitDetail`（裁剪图 + 各模板分数 + 识别字符 + 最高分），并抽出唯一识别入口 `match_one`（裁剪→模板匹配→阈值判定，同时产出字符与详情）。
  - `recognize_digits`/`retry_missing` 改为复用 `match_one`；新增 `recognize_detail` 返回 `(PriceRec, Vec<Option<DigitDetail>>)`（千/百/十/个），二轮回填放宽阈值下重算遗漏位的详情使 `recognized` 与最终价格一致。
- **`recognition.rs`**：新增 `recognize_bullet_detail` 与 `recognize_caliber_detail`（每个子弹附带价格各数位 `DigitDetail`）；删除不再使用的 `recognize_caliber`（OCR 测试已切到 detail 版）。`DigitDetail` 经 `pub use` 对外暴露。
- **`main.rs`（OcrTestPanel）**：
  - `OcrSlot` 增 `digits: Vec<Option<DigitDetail>>`；`run()` 改调 `recognize_caliber_detail` 并填充，识别后 `ocr_test_seq += 1` / 选中重置 0。
  - 新增「价格浏览」滑块（`egui::Slider` 选子弹序号）+ 逐位展示：每个数位给出**裁剪匹配图**（实际数字截图）、识别字符、最高分，以及 0~9 十个模板的**分数条**（`draw_score_bars` 用 Painter 绘制，最佳模板绿色高亮）。
  - 纹理重建与选中提交放在 `res` 不可变借用作用域之外，渲染块只读，规避借用冲突；数字纹理仅在结果更新或选中变化时重建。
  - 新增 app 字段 `ocr_test_selected/ocr_test_seq/ocr_test_digit_seq/ocr_test_digit_sel/ocr_test_digit_tex`。
- **编译**：`cargo build` 通过，**0 error / 0 warning**（增量锁警告 `os error 5` 为被杀进程残留，删 target/debug/incremental 后消失）。需在真实 Windows 环境用一张交易行截图 + 口径点「识别」→ 拖动滑块核验逐位匹配图与分数条。
