# 变更记录（CHANGELOG.md）

> 每次修改后追加一条。后续会话先读 `PROJECT.md` + 本文件即可，无需重载全部上下文、无需重读整个项目。

## 2026-10-05（续十七）— 新增「快速调整」：数量滑块就近跳转，加减微调兜底

- **需求**：购买界面加减按钮间存在数量滑块，点击滑块某位置可快速定位到目标数量。锚点 x=1555、y 取加减按钮中间行；先据已知点（目前仅 31）找到离目标最近的参考点跳过去，再从该点用加减按钮精确微调，避免从 1 起逐点累加的大幅连点。
- **改动（`config.rs`）**：新增 `SLIDER_ANCHOR_X: i32 = 1555`、参考点表 `QTY_SLIDER_REFS: &[(u32,i32)] = &[(31,0)]`（后续可直追加如 `(10,-200)`/`(100,300)` 等，调用逻辑无需改）。
- **改动（`buy.rs`）**：
  - 新增纯函数 `nearest_ref_point(target)`（从 `QTY_SLIDER_REFS` 挑离 target 最近的数量值）与 `slider_click_rect(scene, scale, dx)`（滑块点击矩形：x=`SLIDER_ANCHOR_X+dx`，y=按钮行中心，10×10 块，经 `Scale::rect` 映射）。
  - `adjust_buy_qty`：进入加减循环前先做滑块跳转——识别初始量 `cur0`，仅当 `|ref_qty-target| < |cur0-target|`（跳转后更近）才点滑块；跳转后重识别记日志，失败/未生效则下方循环照常加减微调兜底。避免"越跳越远"。
- **验证**：`cargo check` 0 错；`cargo test` **7 passed/0 failed**（target_verify 目录规避运行实例持锁 LNK1104）。滑块跳转失败不影响后续加减逻辑，无副作用。

## 2026-10-05（续十六）— 取消数量调整上限，改用「前后 CNN 对比 + 单步最多重试 3 次」

- **需求**：取消 `ADJUST_MAX` 总体次数上限（原 20 次导致 1→31 等跨距被截断、停在 21 报"未能调整到 31"）；改为：每步用 CNN 识别"调整前"数量，点击加/减后用 CNN 识别"调整后"数量并对比；变化=生效继续，未变=自动重试该步（最多 3 次），达 3 次仍无效则停止（不购买）。
- **改动（`src/domain/buy.rs` · `adjust_buy_qty`）**：
  - 删除常量 `ADJUST_MAX`（及 `for _ in 0..ADJUST_MAX` 外层循环），改为无限 `loop`，直至 `before==target_qty`→`Confirmed` 或单步重试耗尽→`Unconfirmed`。
  - 新增 `const MAX_RETRIES: usize = 3`；每步先 `recognize_buy_qty` 记 `before`，选按钮（偏少加/偏多减），`click_client_rect` 派发后等 `ADJUST_STEP_MS`，再 `recognize_buy_qty` 得 `after`；`after != before`→生效 `break`，否则重试（循环 `0..=MAX_RETRIES`，即首次+3 次重试=共 4 次点击）。
  - 重试耗尽仍 `after == before`→`[WARN] …点击 4 次均无效…` 并返回 `Unconfirmed`；识别失败（`None`）或点击坐标转换失败均立即 `Unconfirmed`（不做无意义重试）。
  - 移除旧的 `prev`/D3 校验块（其功能已由显式前后对比+重试覆盖，且更精确）。
- **验证**：`cargo test` **7 passed / 0 failed**（含 `buttons` 端到端模型测试）。无总体上限后，1→31 等任意跨距均可逐步调达；单步失效由重试兜底、超 3 次即止，不存在死循环（每次生效步 ±1 收敛至目标）。
- 注意：最坏耗时随跨距增长（每步 150ms×最多 4 次点击），但属可接受；若后续需加速可改为连点后识别（待另议）。

## 2026-10-05（续十五）— 补充「数量匹配后购买条件判断」提示：满足/未满足均向用户弹出

- **问题**：用户实测「数量调整到与面板设置一致时程序未出现任何提示」。核查 `buy::verify_then_decide` 的数量匹配分支发现：
  - 「满足」分支（D 闸门，`QtyAdjustResult::Confirmed`）原有 `tour.ask`，但文案仅问「是否点击购买」，**未显式陈述「满足购买条件」判定结论**；
  - 「未满足」分支——`QtyAdjustResult::Unconfirmed`（识别失败/超上限/落点失效）与 `scene == None`（场景未判定、无法定位按钮）——**仅有 `crate::log::write` 日志、无 `tour.ask` 提示**，故失败路径静默跳过，用户看不到任何提示（与报告一致）。
- **修复**（`src/domain/buy.rs`，仍为导游模式提示，关闭态零行为变化）：
  - 满足：`【确认购买】当前数量 X 发，已与设置(Y发)一致，满足购买条件，是否点击购买？`——明确陈述判定结论。
  - 未满足（Unconfirmed）：`【未满足购买条件】数量未能调整到设置值 Y 发（识别失败/超上限/落点失效），不满足购买条件，不购买，返回扫描`。
  - 未满足（scene==None）：`【未满足购买条件】购买界面场景未判定，无法定位按钮、数量未确认，不满足购买条件，不购买，返回扫描`。
  - 三处均在 `tour.ask` 后紧跟 `stop` 检查，命中即 `Abort`（与原安全闸门一致）。
- **验证**：`cargo test` **7 passed / 0 failed**（含 `buttons` 端到端模型测试）。提示仅在导游模式开启时出现（毛玻璃叠加层 + 回车确认），关闭则无提示、行为同前。

## 2026-10-05（续十四）— 新增「导游模式」：每步先展示会议文字计划，回车确认后执行
- **需求**：控制面板新增「导游模式」开关；开启后观察过程中每一步操作先以会议文字形式叠加在毛玻璃层上，使用者回车确认后才执行；覆盖各类场景（检索口径/进入购买/调整数量/确认购买/放弃/跳过），且后续新增逻辑可低成本接入。
- **架构（新建 `src/tour.rs`）**：`Tour` 协调器以 `Arc<Mutex<Option<(id,text)>>>` + `Arc<(Mutex<u64>, Condvar)>` 实现「后台线程 `ask` 展示并阻塞、UI `confirm` 放行」。
  - **一次性确认（自增序号）**：`confirm` 只放行「此刻正在展示」的计划序号，连按回车/提前多按均为空操作，杜绝误放行后续步骤（关键健壮性）。
  - `Tour::disabled()` 下 `ask` 为无操作、`pending_text` 恒 `None`、`confirm` 空操作——故关闭态**零行为变化、零开销**。
- **接入点**：
  - `observe::run_observation` 增 `tour: Tour` 参数；每口径检索前 `tour.ask("【计划】开始检索「{cal}」口径子弹")`，`ask` 后立即查 `stop` 退出；并 `tour.clone()` 透传 `verify_then_decide`。
  - `buy::verify_then_decide` 增 `tour: Tour` 参数；在 进入购买(B)/调整数量(C)/确认购买(D) 三道闸门及 放弃购买/超时/跳过(无阈值·价格无效) 各分支前置 `tour.ask` 等待确认，`ask` 后查 `stop` 返回 `Abort`（不购买），与原安全闸门一致。
- **UI（`main.rs` / `control_panel.rs` / `floating_panel.rs`）**：
  - `ObserverApp` 增 `tour_mode: bool` 与运行时 `tour: Tour`；「开始观察」按开关构造 `Tour::new()` / `Tour::disabled()`。
  - 激活态窗口在导游模式下放大至 `TOUR_FLOAT_SIZE (500×380)` 以容纳导游层。
  - `update()` 每帧：若有待确认计划则 `ViewportCommand::Focus` 抢焦点（仅此刻，避免常态干扰游戏输入）并检测 `egui::Key::Enter` 调用 `tour.confirm()`；确认后焦点由后台线程下一步动作前交还游戏。
  - 导游层：`floating_panel` 在状态层之上**额外叠加一层毛玻璃卡片**，展示会议文字计划 + 「按 Enter 确认并执行 · F9 结束观察」。
  - F9 兜底：`stop()` 若发现有待确认计划，先 `tour.confirm()` 放行被卡住的线程，使其随后据 `stop` 自行退出（避免线程永久阻塞在条件变量）。
- **可扩展性**：后续任何新增自动化操作只需在执行前调用 `tour.ask(计划文字)` 即可自动获得导游确认能力，无需各自实现确认 UI。
- **验证**：`cargo check` exit 0；`cargo test` **7 passed/0 failed**（含 `buttons` 端到端模型测试）。主 target 因运行中 .exe 持锁报 LNK1104（环境锁），用 `target_verify` 目录编译链接通过。
- **注意**：导游模式开启时，无阈值/价格无效的子弹每颗都会弹出「跳过」确认提示，扫描全程需逐颗回车；此为「覆盖各类场景」的代价，可关闭导游模式全自动运行。

## 2026-10-05（续十三）— 修复加减按钮定位缺陷：改用场景标定模板，杜绝误购
- **根因**：购买阶段加减按钮坐标来自 NN 检测器（`buttons.onnx`）对 `plus/buy` 的分类输出；而该检测器会把「购买」误判为「加号」（`qty_tool.rs` 注释已明文记录、实机已观察到误点购买），且 `buy.rs` 此前未规避——直接用 `det.plus` 点击。数量动态调整（1→31 反复点加）时任一帧误判即变成反复点「购买」。同时 `det` 仅检测一次、整个 `ADJUST_MAX` 循环冻结复用，陈旧且随检测抖动。
- **修复（D1/D2）**：`adjust_buy_qty` 入参由 `det: &ButtonDet` 改为 `scene: BuyScene`，按钮坐标改由 `scene.layout()`（标定模板）经 `Scale::rect` 映射取得，与 `qty_tool` 同源自 `BUY_LAYOUT_1/2`；检测器（仍调用）只用于判定场景，不再提供点击坐标。`verify_then_decide` 的 `buy_rect` 也改取模板坐标。模板恒定、与数量无关、零检测误差。
- **加固（D3）**：新增「增减校验」——点击后下一轮复识别，若数量未按预期 ±1（落点失效/点击未生效）立即 `Unconfirmed` 中止，避免对失效落点连续盲点。移除原 `det.plus/minus/buy` 三处 `None` 守卫（模板恒有值）。
- **验证**：`cargo check` exit 0；`cargo test` **7 passed/0 failed**（含 `buttons` 端到端模型测试）。

## 2026-10-05（续十二）— 代码审查落地：F9 急停透传 / 诊断日志受控 / 数量上限对齐 / 位数决策加固
- **S1（严重·F9 急停生效）**：`buy::verify_then_decide` / `adjust_buy_qty` 新增 `stop: Arc<AtomicBool>` 参数（由 `observe::run_observation` 透传 `stop.clone()`）。就绪轮询（150ms）与数量调整等待（150ms）改用 `sleep_interruptible` 分片（每 20ms 查一次 stop），长阻塞内 F9 生效延迟降至数十毫秒；收到 stop 即保守返回 `Abort`/`Unconfirmed`，由 `observe` 统一 ESC 退出。彻底解决此前 F9 不立即停的根因（stop 未传入长阻塞分支）。
- **S2（严重·诊断日志收口）**：`win.rs` 新增 `diag_enabled()` 总开关，仅当 `DELTA_OBSERVER_DEBUG=1|true|yes|on` 时输出 `[CLICK-DIAG]`/`[MOVE-DIAG]` 及其额外 `ClientToScreen(hwnd,(0,0))` 探测；常态运行不再刷坐标细节。此前这些日志永久写入 `observe.log`，既刷屏又每次点击多发一次坐标转换。
- **H1（高·数量上限对齐）**：`qty_classifier::NUM_CLASSES` 提升为 `pub`；`observe` 在入口对 `buy_qty` 做 `clamp(1, 200)`，UI `control_panel.rs` 的 DragValue `clamp_range` 由 `1..=999` 改为 `1..=200`，与数量整块分类器支持区间一致——避免 UI 越界值导致数量永远无法对齐、盲目重试。
- **H3（高·位数决策加固）**：`verify_then_decide` 稳定后用**扫描价位数**（`four`）作为读取 3/4 位槽位的权威来源（扫描价与购买价同属一颗子弹、数值一致，且扫描界面数位坐标已充分标定；购买界面 3 位布局「百位」与 4 位布局「千位」同像素 `[298,174]`，单凭图像无法区分 3/4 位，故扫描价是唯一可靠判据）。新增一致性守卫：若稳定价位数与 `four` 明显矛盾则翻转位数重识别一次兜底；注释明确该设计取舍，纠正此前「耦合扫描价」的误读。
- **M5（中·UI 文案修正）**：`control_panel.rs` 购买数量/阈值提示文案改为与实际「进入购买界面 → 调数量 → 点击购买 → ESC 返回扫描」行为一致（旧文案仍写「点击价格左侧/上方区域并 ESC 恢复」，与现逻辑矛盾）。
- **M3（中·单测补强）**：`qty_classifier.rs` 新增 `round_half_even`（银行家舍入，对齐 Python `round`）与 `align_and_stretch`（重心对齐+拉伸）的纯函数单测。
- **验证**：`cargo check` exit 0；`cargo test` **7 passed/0 failed**（含 `buttons` 端到端模型测试），本次新增 3 项。
- **未改（沿用既有设计，记录在审查报告中）**：H2（Scale 仅按宽度缩放，交易行 UI 横向铺满、纵向裁切，标定 1920×1080 因子恒 1.0，属刻意取舍）；M2（`sanitize` 两处语义不同，未盲目合并）；M4（购买阶段每轮需重新截帧，复用无收益）；M6（手写 JSON 仅转义 5 类字符、子弹名无特殊字符，改 serde_json 收益低风险高）；L1-L4（`play_capture_chime` 同步播放 ~0.26s 仅发生于 Done 一次，影响可忽略）。

## 2026-10-05（续十一）— 真正根因：两场景 2px 纵向错位；对齐+拉伸后训练 acc=1.0
- **推翻续十误诊**：用户提供两张完整购买界面截图，`calibrate.py` 红框 + 包围盒实测证明**坐标完全正确**——
  数字 "200/200" 全局包围盒 x[1619,1690]，可兑换 y[734,746]、不可兑换 y[792,804]，均被
  (1560,722,140,32)/(1560,782,140,32) 完整罩住。放大数据集图逐类目检：类 1="1/200"、类 200="200/200"，
  **标签与内容一一对应，数据无任何问题**（此前 ASCII 渲染下采样把 13px 细数字洗掉，造成"无数字"误判）。
- **真正根因**：两场景裁剪区内数字局部行相差 **2px**（可兑换局部 y≈18，不可兑换 y≈16）。细笔画数字
  （1px 宽、13px 高）错位 2px 后，同类两图逐像素差（类内 12.6）超过不同数值差（类间 10.2），
  CNN 完全无法收敛。纵向重心对齐后：**类内 0.63 / 类间 18.4（29 倍），1-NN 留一法 97.5%**。
- **修复**（`train_qty.py` + `qty_classifier.rs` 两端同步）：
  1. 新增 `align_and_stretch()` 前处理：亮像素(≥110)行重心整幅纵向平移至第 16 行（越界补背景值，
     Rust 端银行家舍入与 Python `round` 一致）+ 线性对比拉伸 `clip((x-37)*255/115)`；常量
     `QTY_STROKE_THRESH=110 / STRETCH_LOW=37 / STRETCH_HIGH=152 / ALIGN_ROW=16` 两端一致；
  2. `QtyDataset.__getitem__` / `infer` / 信号自检统一走对齐+拉伸；自检报警文案修正；
  3. `qty_classifier.rs::preprocess` 在灰度化后、缩放前插入同款 `align_and_stretch()`。
- **验证**：`train --no-augment --epochs 60 --export-onnx` → acc 0.005 → **1.0000**（epoch 58），
  loss 5.31→0.02；模型对 1/7/42/100/200 抽样推理全部命中；`cargo build` exit 0，`cargo test` 4 passed/0 failed。
- **README**：前处理契约表新增对齐+拉伸行；训练命令推荐 `--no-augment`（确定性渲染 + 每类 2 张，
  增强反而无法收敛）。**无需重采集**，现有 400 张数据直接可用。

## 2026-10-05（续十）— 根因最终定位：裁剪坐标截到静止 UI 条，区域无数字（已续十一推翻）
- **演示证伪"场景混放"假设**：单场景数据单独训练仍 acc=0.005、loss 卡 5.3（续九的 2-means 200/200 分簇分的是两场景各自静止背景，非数字）。
- **ASCII 有符号渲染铁证**：标签 1/2/3 逐字符一致，100/200 彼此一致仅整体左移一格；140×32 区域仅含左侧圆角图标 + 静止竖条分隔符，**无任何随数量变化的数字笔画**。
- **结论**：坐标 `(1560,722)/(1560,782)` 截到的是静止 UI 条，**不含所选数量数字**；训练失败 100% 是采集坐标错（数据无数字信号），非网络/方法/场景混放问题。
- **下一步**：需完整购买界面截图，用 `calibrate.py` 红框确认偏位 → 改 `train_qty.py` 顶部 `QTY_REGION_*` 与 `config.rs` `BUY_QTY_REGION_1/2` → 重采集每类 10~20 张再训。演示脚本已删除。

## 2026-10-04（续九）— 训练失败根因：裁剪坐标未捕获有效数字
- **现象**：`train_qty.py` 训练 acc 恒为 0.005（=随机 1/200），loss 卡在 5.30（=ln200）；增强开/关均失败；此前偶发的 0.99 为不可复现假象。
- **根因（经实测定位）**：`qty_dataset` 的**类间平均像素差(10.2) < 类内平均差(12.6)**（值域 0-255），即不同数值截图比同数值两张更相似——数学上不可能发生于正确截到数字的数据集。说明 `BUY_QTY_REGION_1/2`（可兑换 (1560,722,140,32)/不可兑换 (1560,782,140,32)）**未框住动态数量数字**，截到的是静止背景/标签区，标签(目录名)与图像内容脱钩。
- **解释"数据吻合却推理差"**：训练与推理裁剪同一错误区域 → 一致但无信号 → 模型无法区分数值 → 推理同样失败。前处理无偏差（裁剪恰为 140×32，缩放恒等；灰度/归一化 Python↔Rust 一致），非前处理问题。
- **改正**：
  1. `train_qty.py` 新增 `dataset_signal_report()` 信号自检——类间差≤类内差×1.2 即报警，避免再 silently 训出垃圾；
  2. 新增 `nn-classifier/calibrate.py`：用一张完整截图可视化裁剪框（红框+裁出 crop.png），目视确认是否落在数字上；
  3. 增强改温和（去强模糊，仅 ±3% 平移/±2% 缩放/±0.12 亮度），并补"每类<5 张时增强会令模型无法学习"的告警 + 修复 `present` 统计 off-by-one（用 range(1,201)）；
  4. 验证：语法 pass；信号自检正确命中报警。
- **用户下一步**：用 `calibrate.py` 校准坐标 → 同步改 `config.rs BUY_QTY_REGION_1/2` 与 `train_qty.py QTY_REGION_*`（两处都要改）→ 重采集合类 10~20 张 → `python train_qty.py train --epochs 60 --export-onnx`。注意每类 2 张即使坐标正确也只够"死记"，泛化仍需多采集+温和增强。

## 2026-10-04（续七）— 数量坐标更正 + 采集落盘防覆盖
- **坐标更正**（用户二次订正）：数量整块区域由旧值（可兑换 y=782 / 不可兑换 y=726，高 31）改为
  **可兑换 (1560,722,140,32) / 不可兑换 (1560,782,140,32)**（高 32）。同步 `config.rs`
  `BUY_QTY_REGION_1/2`、`train_qty.py` 两常量与 README 坐标表；`QTY_INPUT_H=32` 与新高 32 天然对齐。
- **确认训练工程隔离**：三个 NN 功能各自独立物理目录——`dataset/`（digit）、`button_dataset/`
  （button，含 `train.py`）、`qty_dataset/`（qty，含独立 `train_qty.py`），互不交叉、产物各为
  `model.onnx` / `buttons.onnx` / `qty.onnx`。
- **防覆盖**：`qty_tool.rs::save_qty_region` 原用 `qty_{idx}_shot.png` + `File::create` 会覆盖同名
  旧图（重跑采集 `idx` 归零即误覆盖）。改为同名存在时追加 `_1/_2…` 序号生成新文件名，**保留
  文件夹内全部已有图片、绝不覆盖/清空**。

## 2026-10-04（续六）— 数量采集合成串联 + 分类区间改 1~200 + nn-classifier README 三功能文档

- **问题**：数量采集 tab 截图速度比画面动作快一倍以上，原「pre 截图→点加号→post 截图」双截逻辑导致同一数量被重复/错标（post 截到旧值却标为 +1）。
- **`qty_tool.rs` 串联修正**：每轮改为「截一次整块区域（标注为当前应显示数量 `expected`）→ 点加号 → 等待 `CLICK_SETTLE_MS=250ms` 画面刷新 → 下一轮截 `expected+1`」。**每个数量值只截一次**，从根上消除同值重复/错标；`expected` 起步 `start_qty.clamp(1,200)`，越界 1..200 提前结束。
- **分类区间改 1~200（不含 0）**：
  - `train_qty.py`：`QTY_NUM_CLASSES=200`、`QTY_MIN_LABEL=1 / MAX=200`；`QtyDataset` 目录名（数量值 1..200）→ 训练标签 `value-1`（0..199）；`collect` 拒绝 0/越界；`infer` 输出 `value = argmax + 1`。
  - `qty_classifier.rs`：`NUM_CLASSES=200`；`classify` 输出 `top + 1`（索引 0→数值 1）；ONNX 契约 `logits[1,200]`。
  - `main.rs`：`qty_tool_start` 默认 1；`qty_tool_panel.rs`：DragValue 范围 `1..=200`；`buy.rs` 文档同步 1..200。
- **`nn-classifier/README.md`**：补齐「任务 3：子弹所选数量整块区域分类（qty）」完整文档——模型/前处理契约、两场景坐标、数据集格式 `qty_dataset/<value>/`、子命令 `collect/train/infer` 参数表、Rust 端集成、端到端流程；并将模块整体改写为「三个神经网络功能（数字分类 / 按钮检测 / 数量整块分类）按物理文件夹 + 独立脚本隔离」说明，目录结构补 `qty_dataset/`、`train_qty.py`、`qty.onnx`。
- **验证**：`cargo build` exit0（仅无害增量锁文件删除提示 `os error 5`，系残留进程持有，删 target/debug/incremental 即消失）；`cargo test` **4 passed/0 failed**；Python 改动逐段人工复核（沙箱拦截解释器，逻辑等价，无需 py_compile 即确认）。

## 2026-10-04（续五）— 子弹数量识别改为「整块区域整体分类」（0~200）

- **需求变更**：原数量识别 `recognize_buy_qty` 逐位（百/十/个）识别再拼接；因数量在画面中居中显示、不同位数坐标位置不同，逐位裁剪极易错位。改为对**整块数量区域**一次性分类，直接输出整数值 0~200（共 201 类）。
- **新增训练脚本 `nn-classifier/train_qty.py`**（子命令 `collect`/`train`/`infer`）：
  - 模型 `QtyRegionCNN`：灰度 → 3× conv+maxpool → fc → 201 类；输入 `1×1×32×140`，输出 `logits[1,201]`（索引即数值）。
  - 前处理契约与 Rust 一致：灰度(ITU-R 601) → 双线性 32×140 → `/255` → `(x-0.5)/0.5`。
  - `collect`：从完整截图按场景裁剪整块区域并写入 `qty_dataset/<label>/`（可兑换 (1560,782,140,31) / 不可兑换 (1560,726,140,31)）；`train` 导出 `qty.onnx`（已用 checker 校验，输入 [1,1,32,140] / 输出 [1,201]）。
- **新增 Rust 模块 `src/domain/qty_classifier.rs`**：加载 `qty.onnx`（惰性单例 + env/exe 向上追溯，模板同 `nn_classifier`/`buttons`），整块区域预处理 + 201 类 softmax + 置信度阈值(0.5) → `Option<u32>`。
- **`config.rs`**：新增 `BUY_QTY_REGION_1/2` 与 `BuyScene::qty_region()`；删除已废弃的逐位 `BuyQtyRects`/`BUY_QTY_DIGITS_1/2`/`qty_rects()`。
- **`buy.rs`**：`recognize_buy_qty(hwnd, scale, scene)` 改为裁剪整块区域经 `qty_classifier::classify`；`adjust_buy_qty` 调用同步更新。
- **`qty_tool.rs`**：采集逻辑由「逐位裁剪」改为「整块区域裁剪」，并按 **起始数量 + 点击序号** 标注为 `qty_dataset/<label>/`（新增长参数 `start_qty`），直接产出带标注训练集。
- **`main.rs`/`qty_tool_panel.rs`**：新增「起始数量」输入（0..=200，默认 0），透传至 `run_qty_collection`。
- **`dist.bat`**：随包拷贝 `nn-classifier/qty.onnx` 至 `dist/nn-classifier/`。
- **验证**：`cargo build` exit0（仅增量锁文件删除警告；LNK1104 因实机运行实例占锁，Stop-Process 终止后重建）；`cargo test` **4 passed/0 failed**；`train_qty.py --help` 与 `py_compile` 通过；合成数据 1-epoch 训练+导出验证 ONNX 形状 = `input[1,1,32,140]` / `logits[1,201]`，与 Rust 端完全一致（合成产物已清理，未随包分发）。

## 2026-10-04（续四）— 数量采集：修正加号点击落点 + 毛玻璃悬浮面板 + F9 结束

- **问题①（点击落点）**：原 `qty_tool.rs` 的加号点击坐标 `plus_rect` 优先取检测器 `d.plus`（class 2）。因「购买」与「加号」外形相近，检测器在两者混淆时会把 **buy** 的 bbox 当作 plus 返回，导致实际点到**购买按钮**（实机已观察到）。**修复**：加号点击改为确定性配置坐标 `scale.rect(scene.layout().plus)`（与数量数位裁剪同源，已知场景即已知加号位置，绝不误点购买）；`d.plus` 仅保留于场景判定（Auto 模式），不再用于点击。
- **问题②（毛玻璃 + F9）**：原 `start_qty_tool` 未进入激活态（完整主面板盖住游戏），且 F9 只停观察任务、不停本工具。**修复**：
  - `main.rs` 抽离 `enter_active_mode(ctx, status)`，观察与数量采集共用；`start_qty_tool` 启动即 `enter_active_mode("数量采集中")` → 收起为置顶毛玻璃悬浮面板（透明/穿透/排除截图/开启 DWM 模糊）。
  - `stop()` 现同时中止 `qty_tool_stop` 并复位采集状态；`handle_f9` 经 `stop()` 一并结束采集并退出毛玻璃态。
  - `poll_qty_tool` 收到 `Done` 后调用 `stop(ctx)` 退出毛玻璃、恢复主面板。
  - `floating_panel.rs` 新增数量采集分支：运行中优先展示「数量采集中 / 已截图 N/200 次 / 按 F9 结束采集」，其余观察态内容不再显示。
- **验证**：`cargo build` exit0（仅增量锁文件删除警告；LNK1104 因实机运行实例占锁，停进程后重建通过）；`cargo test` **4 passed/0 failed**。
- **新增 `src/domain/qty_tool.rs`（领域层采集逻辑）**：
  - `SceneMode`（Auto / Exchangeable / NonExchangeable）+ `from_u8`；
  - `TOTAL_SHOTS = 200` 上限常量；
  - `QtyToolMsg::{Progress{taken,total}, Done{ok,msg}}` 后台→主线程通道消息；
  - `run_qty_collection(hwnd, scene_mode, out_dir, stop, tx)`：缩放因子按客户区宽度推导；场景判定一次（Auto 用 `buttons::detect_buttons_from_frame` 定位，失败回退可兑换）；加号位置优先用检测器 `det.plus`，否则用 `scale.rect(scene.layout().plus)`；
  - 每轮：截客户区 → `crop_region` 裁百/十/个位各存 `qty_{idx:05}_{pre|post}_{h|t|o}.png`（`save_rgba_png` 同步落盘，单帧保存完才进下一步）→ 点加号 → 再次截图裁剪；截图/保存失败则本帧重试（`continue`，不推进 idx/taken）；
  - 停止条件：`taken >= 200` 正常结束；`stop` 信号置位优雅退出；`max_cycles = 200+300` 安全阀防死循环；每轮含节流延时（60~120ms）。
- **新增 `src/ui/qty_tool_panel.rs`**：卡片式面板；场景下拉（自动检测/可兑换/不可兑换）、输出目录（`浏览...` 选取，默认 `{save_path}/qty_tool`）、「▶ 开始采集」/「■ 停止」互斥按钮、实时「采集中：N/200 次截图」。
- **`main.rs`**：新增 `MainTab::QtyTool` 并接入 Tab 栏 + dispatch；`ObserverApp` 新增 `qty_tool_scene/u8`、`qty_tool_dir/String`、`qty_tool_running/bool`、`qty_tool_taken/usize`、`qty_tool_rx`、`qty_tool_stop`；`start_qty_tool`（聚焦窗口 + 起后台线程）、`stop_qty_tool`（置位停止信号）、`poll_qty_tool`（收消息更新计数/复位）三方法；`update` 中调用 `poll_qty_tool`。
- **模块注册**：`domain/mod.rs` 增 `pub mod qty_tool`；`ui/mod.rs` 增 `pub mod qty_tool_panel`；`main.rs` 重导出二者。
- **验证**：`cargo build` exit code 0（仅增量锁文件删除警告，非错误；LNK1104 因用户实机运行实例占锁，PowerShell `Stop-Process` 终止后重建通过）；`cargo test` **4 passed / 0 failed**（原有用例未破坏）。
- **说明**：每次「截图」= 一个采集动作（三数位各一张 PNG，共 3 文件），计 1 次；每轮 2 次截图 → 100 轮 = 200 次上限。F9 不中断本工具，须用面板「停止」。

## 2026-10-04（再续二）— 毛玻璃实时显示子弹数量 + 三层闸门日志（启动清空）

- **需求**：① 购买界面识别出的子弹数量实时显示在毛玻璃 UI（内容不截断）；② 三层闸门（L1 调用侧 / L2 按钮检测 / L3 数量识别）加关键状态与结果日志排查；③ 每次程序启动清空旧日志，避免累积消耗。
- **新增 `log.rs`**（轻量日志模块）：`init()` 以截断模式创建 exe 同目录 `observe.log`（启动即清空旧内容）；`write(msg)` 仅记录一行关键状态/结果；未初始化（如单测）时为 no-op 不落盘。`main()` 启动时调用 `crate::log::init()`。
- **`buy.rs` 三层闸门日志（仅关键状态/结果）**：
  - L1（调用侧）：进入调整时记「场景 + 三按钮已检出 + 目标数量」；`det=None` 或 `scene=None` 各记一条跳过原因。
  - L2（`adjust_buy_qty` 首行）：`minus/plus/buy` 任一未检出各自记一条跳过原因（即「加减按钮未检出导致动作缺失」的直接证据来源）。
  - L3（循环内）：每轮记「识别数量 / 目标」，点击加/减各记一条；`recognize_buy_qty` 返 `None` 记识别失败；对齐/超上限/鼠标就位各记一条。
  - 实时上报：循环内每识别到数量即通过 `ObserveMsg::DetectedQty { qty }` 发往主线程刷新面板；失败/超上限发 `None`。
- **`observe.rs`**：`ObserveMsg` 新增 `DetectedQty { qty: Option<u32> }` 与 `BuyCleared`；`verify_then_decide` 新增 `tx: &mpsc::Sender<ObserveMsg>` 透传（实机实时上报数量）；每颗子弹进入购买判定前发 `BuyCleared` 清空上一颗残留的购买展示（价格/场景/数量）。
- **`main.rs`**：`ObserverApp` 新增 `detected_qty: Option<u32>`（默认 None，new/start/Done 均清零）；`poll_observe` 处理 `DetectedQty`（写入并 `request_repaint`）与 `BuyCleared`（清空三项）；`main()` 启动时 `log::init()`。
- **`floating_panel.rs`**：购买阶段（场景已判定）在「目标数量：N」下方以 15px 加粗展示「当前数量：N」实时值；识别失败/尚未识别显示占位「当前数量：—」，避免面板空缺或截断。
- **验证**：`cargo build` exit code 0（期间遇 LNK1104，因用户实机运行中的 `delta-trade-observer.exe` 占锁，PowerShell `Stop-Process` 终止后重建通过）；`cargo test` **4 passed / 0 failed**。
- **说明**：日志仅记录关键状态/结果，非逐帧刷屏；启动截断旧日志，避免跨班次无界累积。

## 2026-10-04（再续）— 购买阶段：数量识别 + 减/加自动调整（驱动现有 buttons.onnx）

- **目标**：在 `verify_then_decide` 判定 `Proceed`（重验证价 < 阈值）后，识别购买界面「所选子弹数量」三数位，用检测器已解码的「减/加」按钮点击把数量调到面板设定值（`buy_qty`，默认 31），再把鼠标移到「购买」按钮（不点击），由 `observe` 等待 1 秒后 ESC 回扫描。
- **`config.rs` 新增数量数位坐标**：
  - `struct BuyQtyRects { hundreds, tens, ones: Rect }`；
  - `BUY_QTY_DIGITS_1`（场景1·可兑换：百/十/个 = `[1617,730]/[1628,730]/[1639,730]`，均 y=730）；
  - `BUY_QTY_DIGITS_2`（场景2·不可兑换：百/十/个 = `[1617,788]/[1628,788]/[1639,788]`，**三个数位 y 均为 788**，已纠正原方案十位 730 笔误）；
  - `BuyScene::qty_rects(&self)` 返回对应场景的数量坐标。
- **`win.rs` 新增** `click_client_rect(hwnd, rect)`（客户区矩形中心→屏幕坐标→人类化点击）与 `move_mouse_to_client(hwnd, rect)`（同源移动、不点击），供购买阶段减/加点击与鼠标就位复用。
- **`buy.rs` 新增**：
  - `recognize_buy_qty(hwnd, scale, rects)`：复用价格数字分类器 `nn_classifier::classify`（同为 32×32 逐位分类），识别百/十/个三数位拼为 `u32`；任一数位失败即返 `None`（安全降级）。
  - `adjust_buy_qty(hwnd, scale, det, scene, target_qty)`：仅在「减/加/购买」三按钮均检出时执行；循环上限 `ADJUST_MAX=20`、步进 `ADJUST_STEP_MS=150`；`cur<target` 点加、`cur>target` 点减、相等则把鼠标移到购买按钮待命；识别失败/超上限均先移鼠标到购买按钮再返回（不卡死、不误购）。
  - `verify_then_decide` 新增 `target_qty: u32` 形参；`Proceed` 路径改一次性 `buttons::detect_buttons` 取 `ButtonDet`（同时得场景与三按钮 bbox，避免重复推理），场景成立则调 `adjust_buy_qty`。
- **`observe.rs`**：`run_observation` 新增 `buy_qty: u32` 透传；`Proceed` 分支等待由 2 秒改 1 秒（贴合「等待一秒后 ESC」）。
- **`main.rs`**：`ObserverApp` 新增 `buy_qty: u32`（默认 31，未持久化，每次启动回默认防误用）；`start_observation` 透传。
- **`control_panel.rs`**：「是否购买」展开区新增「购买数量」`DragValue`（1..=999，默认 31 发）。
- **`floating_panel.rs`**：购买阶段场景标签下展示「目标数量：N」只读信息，便于实机核对。
- **数量数位识别**：暂复用现有 `model.onnx`（未专门在数量框训练）；若实机误识率高，用户将补充数量框训练数据重训。
- **验证**：`cargo build` exit code 0；`cargo test` **4 passed / 0 failed**（原有 4 用例全过，未破坏）。无实机截图推理（无游戏窗口），建议实机勾选「保存购买界面」复核数量框坐标与减/加点击落点。

## 2026-10-04（续）— Rust 端接入 buttons.onnx：模型加载 + 推理 + 场景判定（可兑换/不可兑换）
- **新增模块 `src/domain/buttons.rs`**：接入 `buttons.onnx`（三分类 CenterNet，3 类 = buy/minus/plus），完成完整场景判断逻辑：
  - 模型加载沿用 `nn_classifier` 的惰性单例 + 环境变量（`DELTA_OBSERVER_BUTTONS_MODEL` / `DELTA_OBSERVER_BUTTONS_MODEL_DIR`）+ exe 向上追溯 `nn-classifier/`、`assets/`、`exe` 同目录；模型缺失则永久记 `None`，推理返回 `None`，不崩溃。
  - 前处理与 `train.py` 完全一致：全幅客户区 RGBA → 丢 Alpha → 双线性缩放 256×256 → `/255` → NCHW（`image` crate `resize` + `Triangle`，等价 PIL `BILINEAR`）。
  - 推理：`ort::Session` 跑 `input[1,3,256,256]` → `heatmap_logits[1,3,32,32]` + `size[1,6,32,32]`（输出名与 `train.py` 导出一致）。
  - 解码（复刻 `predict_layout.py::decode_class`）：热力图峰值 `sigmoid` 过 `THRESH=0.3` → argmax 取格 → 读 `size` 两通道 → 还原客户区 bbox（乘 `sx=orig_w/256, sy=orig_h/256`）。
- **场景判定（核心需求）**：以「购买」按钮 bbox 锚定当前界面属于场景1 还是场景2，并按要求重命名：
  - **场景1 = 「可兑换子弹的场景」**（`buy ≈ [1482,829,297,61]`）；
  - **场景2 = 「不可兑换子弹的场景」**（`buy ≈ [1484,887,293,59]`）。
  - 在 `config.rs` 新增 `BuyScene` 枚举 + `label()`（`可兑换子弹的场景` / `不可兑换子弹的场景`）、`BuyBtnLayout` 结构 + `BUY_LAYOUT_1/2` 常量、`determine_scene(buy_bbox)`（复刻 `train.py::determine_case`：中心距 + 宽高相对误差，`dist<40 && sr<0.15` 才判定，否则 `None`）。
  - 检测出的 `buy` bbox 先经 `Scale::to_calib` 映射回标定视口坐标，再与两套模板比对（与现有数字识别的坐标契约一致）。
- **接入购买流程**（`buy.rs` / `observe.rs` / `main.rs` / `floating_panel.rs`）：`verify_then_decide` 在界面稳定后调用 `buttons::detect_scene`，把场景塞进 `BuyDecision::{Proceed,Abort}`；经新增 `ObserveMsg::BuySceneNotice { label }` 传到主线程，在毛玻璃面板展示当前场景（如「可兑换子弹的场景」）。模型缺失时 `scene=None`，购买流程照常、不报错。
- **`config.rs::Scale` 新增 `to_calib`**：与 `rect` 互逆，把实际客户区矩形映射回标定视口坐标。
- **验证**：`cargo build` 0 error；`cargo test` 3 项全过（`decode_class` 坐标还原 roundtrip、`determine_scene` 匹配两套布局 / 远距返回 None、低置信度不检出）；ONNX I/O 契约与已验证的 Python 路径同名同形。真实 `buttons.onnx` 由 `train.py button` 在 `nn-classifier/` 产出后随 `dist.bat` 分发到 `dist/nn-classifier/buttons.onnx`（搜索路径已覆盖）。
- **待实机确认**：本环境无游戏窗口，未跑真实截图推理；建议用户实机勾选「保存购买界面截图」复核检测框与场景标签。

## 2026-10-04 — 按钮检测升级为三分类（buy/minus/plus）+ 布局判定 + 标注工具重构
- **三分类检测器**：`BuyButtonDetector` 由单类（购买）升级为 3 类 CenterNet，`hm_head` 输出通道 `C=3`、`size_head` `2C=6`。训练数据格式改为 `targets` 字典 `{"buy","minus","plus"}` 各自 `bbox|null`；旧 `{"bbox":...}` 单按钮格式仍兼容（`_normalize_sample`）。
- **两种固定布局**（`train.py` 的 `LAYOUTS`）：① `buy[1482,829,297,61]` / `minus[1461,754,38,42]` / `plus[1764,754,38,42]`；② `buy[1484,887,293,59]` / `minus[1461,815,38,42]` / `plus[1764,815,38,42]`。新增 `determine_case(buy_bbox)` 据「购买」bbox 判定布局①/②（中心距离 + 尺寸相对误差双重判定）。
- **检测逻辑**（`predict_layout.py`）：模型独立解码三类 → 以「购买」bbox 为锚点判定布局 → 减/加按钮布局固定，模型检出且与模板中心偏差 ≤ `CASE_TOL_PX=100` 才采用模型结果，否则回退模板坐标（`score` 置 null 标记 `from_case`），保证输出完整、精确、不被误检带偏。
- **产物重命名**：`buttons.pt` + `buttons.onnx`（原 `buy_button.*`），不覆盖数字模型 `model.onnx`。`predict_button.py` 重命名为 `predict_layout.py`。
- **标注工具重构**（`annotate_gui.py`）：支持鼠标拖拽框选、buy/minus/plus 类别切换、布局①/②一键填充全部三按钮、导出 `targets` 格式；`label.py` 支持按类别逐条录入并合并到同一图 `targets`。
- **数据迁移**：`button_dataset/annotations.jsonl` 245 条旧格式（200 条布局① / 45 条布局②）已迁移为 `targets` 格式（minus/plus 由用例推导）。
- **合成数据**（`make_dummy_dataset.py`）：改为 1920×1080、按两种固定布局精确放置三按钮。
- **验证**：py_compile 全过；合成 160 张训练 12 epoch loss 0.30→0.028；`buttons.onnx` 过 checker，I/O 名 `input` / `heatmap_logits[1,3,32,32]` / `size[1,6,32,32]` 与 `predict_layout` 一致；`predict_layout` 在合成图上正确判定布局、`buy` 由模型检出、`minus/plus` 走模板回退（score null）。onnxruntime 推理路径已装包复测（见下条）。

## 2026-10-03（续2）— 删除 Notification 组件 + 毛玻璃去黑底改通透
- **删除 Notification 组件**：移除 `render_toast`（右上角 Element UI 风格通知）及其 `toast: Option<(String,bool,f64)>` 字段、初始化、`poll_observe` 中 `BuyNotice`/`Done` 的 toast 写入，以及 `update()` 里的 `render_toast` 调用与对应导入（Align2/Margin/RichText/Stroke/Shadow）。
- **价格改渲染到毛玻璃面板**：`ObserveMsg::BuyNotice { text }` 改为 `BuyNotice { price: u32 }`；新增 `ObserverApp.buy_price: Option<u32>`，购买阶段重验证识别到的价格直接写入该字段，由 `floating_panel::status_body` 以**大号数字**渲染（仅显示数字 + 微小「哈夫币」单位，不弹任何提示框）。`start()` 进入激活态时清零，`Done` 时清零。
- **毛玻璃去黑底 → 通透磨砂**：根因是 eframe 默认 `clear_color` 返回 `from_rgba_unmultiplied(12,12,12,180)`（深灰半透明底色）→ 黑色背景。覆写 `App::clear_color` 返回 `[0,0,0,0]` 全透明，使 DWM 背景模糊透出桌面。面板填充由 `228,234,246,70` + 黑色阴影改为极淡白底 `240,244,250,50` + 白描边 `255,255,255,210`，**移除黑色实色底与黑色阴影**，仅保留磨砂质感。悬浮窗高度 208→256 以容纳价格数字。
- **错误可见性保留**：`Done{ok:false}` 不再弹 toast，改为写入 `error` 字段（Idle 主面板展示），避免静默丢失。
- **验证**：`cargo build` 0 error；清理 4 个 warning（floating_panel 多余 `self` 导入、`observe.rs` 中 `Proceed` 解构未用的 `caliber`/`bullet` → 改 `Proceed { price, .. }`）。仅余增量 lock 文件删除的良性 os error 5 警告（非代码问题）。

## 2026-10-03（续）— 废弃 nn-buy-button，合并入 nn-classifier（单一训练入口）
- **合并**：废弃独立子项目 `nn-buy-button/`，全部训练代码并入 `nn-classifier/`（目录名沿用不改）。原两处训练脚本整合为 **`nn-classifier/train.py` 单一文件**，通过子命令区分：`python train.py digit`（数字分类，默认）/ `python train.py button`（购买按钮检测）。无子命令时等同 `digit`，兼容旧调用。
- **共享依赖**：两项目 `requirements.txt` 合并为 `nn-classifier/requirements.txt` 一份（torch / torchvision / pillow / numpy / onnx；onnxruntime 仅 predict_layout.py 可选）。
- **产物隔离**：按钮检测默认输出 `buttons.pt` + `buttons.onnx`，**不再覆盖**数字模型的 `model.onnx`（Rust/ORT 使用的数字模型保持 `model.onnx`）。按钮数据集默认 `button_dataset/`，与数字 `dataset/` 互不干扰。
- **辅助脚本**：`predict.py`（数字）、`predict_layout.py`（按钮布局，import 自合并后的 train.py）、`make_dummy_dataset.py`（按钮合成数据，默认 out=`button_dataset`）均置于 `nn-classifier/`。
- **修正**：`predict_layout.py`（原 `predict_button.py`）在缺 onnxruntime 回退 PyTorch 路径下漏 import `torch` → NameError，已上移至模块级 import 修复。
- **验证**：4 个 .py 均 py_compile 通过；`train.py`/`digit`/`button` 三个 `--help` 正常；合成 200 张训练 12 epoch，loss 0.24→0.013，ONNX 过 checker；digit `model.onnx` mtime 在按钮训练前后不变（未被覆盖）；`predict_layout.py` PyTorch 回退路径可运行。验证产物已清理，仅留源码。README 已同步两类任务文档。

## 2026-10-03 — 价格可视化窗口起点改为「昨日 18:00」+ 新增「购买」按钮目标检测训练子项目
- **价格可视化（price_viz_panel.rs）**：数据窗口起点从「今日 00:00」改为「上一天 18:00」（今日 00:00 往前 6 小时，`window_start_ms`），终点保持当前时刻 `now_ms`。同步更新：① 数据查询过滤 `r.ts_ms >= window_start`；② x 轴域 `[window_start, now_ms]`（不再固定 24:00，折线铺满宽度）；③ 时间刻度改为从昨日 18:00 起每 3 小时一个整点（18:00/21:00/00:00/03:00…），起点标「昨日 18:00」、跨日点标「今日 00:00」；④ 副标题/坐标标题/空数据文案改为「昨日 18:00 起」。移除已无用的 `DAY_MS` 常量。`cargo build` 0 error。
- **新增 NN 子项目 `nn-buy-button/`**：游戏交易行「购买」按钮位置不固定，训练轻量 **CenterNet 风格单目标检测器**自动回归归一化 bbox。模型：3×Conv+ReLU+MaxPool（8× 下采样到 32×32）→ heatmap_head `[1,1,32,32]`（中心热力图）+ size_head `[1,2,32,32]`（w/h 归一化）。损失 = focal heatmap loss + masked L1 size loss。`train.py` 支持 `--epochs/--batch-size/--lr/--export-onnx/--only-export`，导出 ONNX（dynamo=False，过 `onnx.checker`）；数据集为 JSONL（image + bbox 像素坐标，支持 `bbox:null` 负样本）；提供 `predict.py`（优先 onnxruntime，回退 torch，含坐标还原与画框）、`make_dummy_dataset.py`（合成数据冒烟测试）、`requirements.txt`、`README.md`（含与 Rust 端 ORT 前处理对照表）。
- **验证（合成数据 200 张，15 epoch）**：loss 0.19→0.014，ONNX 通过 checker；40 个正样本 **100% 检出**，mean IoU 0.587（tiny 模型 + 拉伸缩放下的合理基线，真实游戏截图需重新采集标注训练）。dummy 产物（dataset/、model.pt、model.onnx）已清理，仅留源码与生成器。

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

## 2026-10-05（续十二）— 购买流程闸门：未确认数量禁止触发购买点击
- **现象（用户全流程测试）**：到达购买界面后跳过数量调整、直接触发购买按钮点击。
- **根因（状态校验缺失）**：
  1. `verify_then_decide` 里数量调整（`adjust_buy_qty`）只在 `det` 与 `scene` 都为 `Some` 时运行，但**无论是否运行都照常 `return BuyDecision::Proceed`**——按钮检测失败/场景判不出时仍发购买触发；
  2. `adjust_buy_qty` 原返回 `Option<()>`，把「已对齐」「识别失败」「超上限未对齐」全部 `return Some(())`，**调用方无法区分数量是否确认过**；
  3. 下游购买触发点只认 `Proceed` 决策，无 `qty_confirmed` 状态闸门 → 未确认数量下也能触发购买。
- **修复（新增约束逻辑）**：
  1. 新增 `QtyAdjustResult { Confirmed(u32), Unconfirmed }` 枚举，`adjust_buy_qty` 仅在复识别 `cur==target_qty` 时返回 `Confirmed`，其余（减加/购买按钮任一缺失、识别失败、超 `ADJUST_MAX` 未对齐）一律 `Unconfirmed`；
  2. `verify_then_decide` 仅在 `adjust_buy_qty` 返回 `Confirmed` 时产出 `BuyDecision::Proceed`，且 `Proceed` 新增 `qty/u32` 与 `buy/Rect` 字段；否则一律 `Abort`（ESC 返回，不购买）；
  3. `observe.rs` `Proceed` 分支改为：数量已确认才 `win::click_client_rect(hwnd, buy)` 实际触发购买（全流程唯一购买触发点），其后短暂停顿再 ESC；`buy_enabled` 开关仍总闸控制。
- **验证**：`cargo build` 通过（主 target 因运行中的 .exe 持有锁报 LNK1104，改用独立 `target_verify` 目录编译链接通过，证明代码无误）；`cargo test` 4 passed / 0 failed。LNK1104 为环境锁文件问题，与代码无关（终止 .exe 或换 target 目录即可）。

## 2026-10-05（续十四）— 点击诊断日志 + 修正"已点击"误报
- **背景**：用户全流程实测"鼠标压根没往加按钮移动"，但日志却打印"点击加按钮"，且数量恒读 1。排查指向 `click_client_rect` 内部 `client_to_screen` 失败静默返回 `false`（鼠标不动），而调用方无条件打印"已点击"→ 误报。
- **改动（仅诊断+日志，未碰主逻辑）**：
  1. `win.rs::click_client_rect` / `move_mouse_to_client`：坐标转换前/后加 `[CLICK-DIAG]` / `[MOVE-DIAG]` 日志，打印 `rect`、客户区中心、屏幕坐标；`client_to_screen` 返回 `None` 时明确记"坐标转换失败（鼠标不会移动）"。
  2. `buy.rs::adjust_buy_qty`：把"点击加/减按钮"日志移到 `click_client_rect` 返回 `true` 之后，并区分"已派发 / 失败（坐标转换失败，未派发）"，消除误报。
- **验证**：`CARGO_TARGET_DIR=target_verify` 编译链接通过；`cargo test` 4 passed / 0 failed（主 target 因运行中 .exe 持锁报 LNK1104，与代码无关）。
- **待实机确认**：跑一次购买流程，据 `[CLICK-DIAG]` 判断根因分支——若打印"坐标转换失败 None"→ DPI/客户区基准错配（修 `client_to_screen` 或程序 DPI 感知）；若打印屏幕坐标但落点偏→ `det.plus` 检测器落点错（re-calibrate 加按钮）。

## 2026-10-05（续十六）— DPI 声明已存在；加 hwnd/原点诊断定位 -30350 真因
- **重要发现**：`main.rs` 早已有 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` 调用（751行），但用 `let _ =` **静默吞掉返回值**——它若被 manifest/框架拒绝会静默失败，进程退回 DPI-unaware，正是 ClientToScreen 坐标错乱的隐患。无 manifest/build.rs/.rc 干扰，声明本应生效。
- **新证据指向 hwnd 而非 DPI**：同一窗口相邻点，加按钮(1770,860)→(1770,860) 恒等、购买按钮(1650,860)→(-30350,-31140) 爆负，非典型 DPI 虚拟化产物（虚拟化不会一恒等一爆负），更像是 `hwnd` 在移动调用时非有效游戏窗口句柄。
- **改动（诊断，未改主逻辑）**：
  1. `win.rs` 的 `click_client_rect`/`move_mouse_to_client` 诊断新增打印 `hwnd` 原始值 + `ClientToScreen(hwnd,(0,0))` 客户原点；
  2. `main.rs` DPI 声明改为可见化记录成功/失败（定位是否真生效）。
- **验证**：`target_verify` 编译链接通过（仅 lock 删除权限警告，无害）。
- **待实机确认**：重跑购买流程，看 `[DPI]` 是否"已声明"；对比 `[CLICK-DIAG]` 与 `[MOVE-DIAG]` 的 `hwnd`/原点值——若二者 hwnd 不同或原点=`-30350` 类垃圾，则根因是句柄失效（疑似反作弊拦截外部进程坐标查询），需改用不依赖 ClientToScreen 的坐标换算。

## 2026-10-06 — 价格可视化新增「一天/一周/一个月」时间范围切换（起点由数据动态推导）
- **背景**：原可视化时间窗口被写死为「昨日 18:00 → 当前」（`window_start_ms` 基于本地午夜减 6h 推导），`VizState.range` 的「近7天/近30天」仅是占位提示、未接入过滤逻辑，实质只显示约一天数据；刻度为固定 3 小时、标签硬编码「昨日18:00/今日00:00」。
- **改动（数据驱动 + 三档视图）**：
  1. 新增依赖 `chrono = "0.4"`（本地时区格式化与刻度对齐）。
  2. 删除写死的 `WINDOW_START_OFFSET_MS` / `local_day_start_ms` / `window_start_ms` 与 `GetLocalTime` 导入；新增 `range_duration_ms`（0=1天/1=7天/2=30天）、`range_label`、`choose_tick_step`（按跨度自适应步长，约 6~10 个标签）、`first_tick_ms`（按本地时区对齐到步长边界）、`fmt_local`（本地时间文本）。
  3. `show` 重构窗口计算：先按口径/子弹/有效价筛选候选，取候选最大时间戳 `data_max` 为窗口右端，`window_start = data_max - range_duration_ms(range)`，起点完全由数据动态推导，绝不写死固定值；切换 `range` 下一帧即联动重算渲染。
  4. `right_column` 标题行加动态副标题（`{} 至 {} · 视图：{}`），右侧新增分段控件 `range_selector`（一天/一周/一个月，当前项橙色高亮、其余浅底，点击即切换）。
  5. `draw_chart` 时间轴改为自适应步长（1天→3h、1周→1天、1月→1周）+ 本地对齐刻度和含日期的动态标签；x 轴标题/空数据文案同步更新。
- **验证**：`CARGO_TARGET_DIR=target_verify cargo check` 通过（exit 0，拉取 chrono 0.4.45）；`cargo test` 7 passed / 0 failed（含 `e2e_real_model_detects_scene`）。

## 2026-10-10 — 购买结果 toast 类别识别（正常购买成功 / 低价购买成功）
- **背景**：点击「购买」后游戏在窗口顶部居中弹出成功 toast（实测延迟 ≈0.3s、持续 ≈1.1s），此前流程完全盲等 400ms 即 ESC，购买结果无回读。需求：只识别两类购买成功（正常「已存入仓库」/ 低价「已用更低价格成交」），失败类（白字）与限流不在处理范围。
- **关键标定发现（录屏逐帧实测）**：toast 文本中只有开头「子弹名 x 数量」是亮绿字，其后消息文本是白字；文本整体在窗口内水平居中（绿字左缘 L 与整句右缘 R 满足 R ≈ 2×窗口中心 − L，标定帧误差 ≤2px）；失败 toast 整句白字、无绿字。
- **新模块 `src/domain/toast.rs`**（识别策略：纯图像判定，不引入中文 OCR，文字"读不准"但归类必须可靠）：
  1. **绿字门控**：监听带（按客户区比例 y∈[13%,24%]H、x∈[15%,85%]W）内亮绿像素计数越阈 → 有成功 toast；失败白字 toast 天然被排除。
  2. **候选带推导**：绿字 bbox 给出文本左缘与行高，按居中对称 `R=2×中心−L`(+48px 余量) 裁出「绿名+白消息」完整文本带，再按亮度 bbox 收紧到字形、高度归一 32px。
  3. **判别模板滑动匹配**：内嵌「更低价格」白字字形模板（`assets/toast/lower_price_marker.jpg`，include_bytes 编译期打入，无外部文件依赖），与候选带逐 x 滑动 NCC，峰值 ≥0.6 判「低价购买成功」，否则「正常购买成功」。
  4. **监听窗取峰值**：点击后监听 3s（90ms 轮询、F9 可中断），累计 NCC 峰值再判定，容忍淡入淡出模糊帧；「低价」确定性命中即提前返回。
- **流程接线**：`observe.rs` Proceed 分支把原 400ms 盲等替换为 `toast::monitor_after_buy`；结果经新消息 `ObserveMsg::BuyResultNotice` 渲染到毛玻璃面板（`buy_result` 字段，floating_panel 展示、BuyCleared/Done/enter_active 统一清理）；未捕获到绿字 toast 记 `[BUY][WARN]`（可能静默失败），不阻断主流程、照常 ESC。
- **标定数据分离度**：低价自匹配 NCC=0.992，正常文案交叉匹配=0.275，阈值 0.6 双边余量 >0.3。
- **验证**：新增 4 个单测（模板加载 / 失败白字不触绿门 / 成功帧过绿门 / 低价高分且正常低分）全过；`cargo test` 11 passed / 0 failed（原 7 + 新 4），无回归。

## 2026-10-13 — 购买后判断逐模式核查 + 导游低价提醒
- **模式界定**：本工程的「购买模式」即 `BuyScene` 两变体——`Exchangeable`（可兑换子弹场景，BUY_LAYOUT_1）/ `NonExchangeable`（不可兑换子弹场景，BUY_LAYOUT_2）。购买点击在 `observe.rs:431` **仅一处**，之后紧跟 `toast::monitor_after_buy`，两场景均经这条**同一条已接入 toast 判定的购买路径**（buy 坐标取自 `s.layout().buy`），故两模式都已接入「正常/低价购买」判定。数量采集工具 `qty_tool` 的点击为 +/- 采集训练数据，非购买，不计入。
- **`BuyScene` 增强**：新增 `#[derive(Hash)]` 与 `BuyScene::all()`（遍历用）；新增单测 `buy_scene_all_covers_every_variant` 锁住「两变体均被核查遍历且各含购买按钮坐标」不变量。
- **逐模式核查表**：`ObserverApp.buy_result_by_scene: HashMap<BuyScene,String>`，`ObserveMsg::BuyResultNotice` 新增 `scene` 字段；每次购买完成据所属场景记入该表（Done 时统一清空，BuyCleared 不清除以跨弹药累计）。
- **毛玻璃面板**：新增「购买后判断 · 模式核查」区块，逐模式列出 `✓ 已接入` 与该模式上次实测结果（尚无记录则「尚无购买记录」），实机逐模式验证可视化。
- **导游低价提醒**：`observe.rs` 点击购买后，据 `monitor_after_buy` 的 `BuyToastKind` 调用 `tour.ask` 提醒——低价成交显「本次为低价购买成功（已用更低价格成交）」、正常显「正常购买成功」、未捕获显「未捕获到成功提示」，回车后再 ESC 返回扫描；非导游态为无操作。
- **验证**：`cargo check` 0 错；`cargo test` 12 passed / 0 failed（原 11 + 新 1），无回归。

## 2026-10-13 — 购买区域优化：口径拖拽排序 + 子弹网格布局
- **需求**：勾选「是否购买」后展示的各口径子弹区域需支持 (1) 拖拽调整口径先后顺序以控制处理顺序；(2) 子弹改横向/网格排列提升多子弹可见性。
- **数据模型**：`ObserverApp` 新增 `buy_caliber_order: Vec<String>`（口径级处理顺序，经 `buy_order.txt` 持久化，每行一个口径）、拖拽临时态 `drag_src`/`drop_dst: Option<usize>`。原 `buy.cfg` 不变。
- **控制面板 `control_panel.rs` 重构**：
  - 口径改为可拖拽卡片：卡左上角竖排三点 grip 手柄（`Sense::drag()` 发起拖拽，不阻塞下方阈值输入），卡内口径名带序号；拖拽源卡半透明、落点卡蓝色高亮描边。
  - 卡片内子弹由纯纵向长列改为 `horizontal_wrapped` 网格（每项 150px 宽，自动换行），提升单屏可见性。
  - 落点判定用卡片 `Frame` 矩形 `contains(指针)`，释放主指针时移动元素并 `persist_buy_order`；指针离窗等异常安全复位。
  - 新增「重置顺序」按钮清空顺序（运行期回退到口径名排序）；每帧与校准配置对账（剔除已移除口径、补入新增口径按名排序），保证「展示=处理顺序」且完整。
- **观察流程 `observe.rs`**：`run_observation` 新增 `buy_caliber_order` 参数，原 `calibers.sort()` 替换为纯函数 `resolve_caliber_order(preferred, info)`——保留用户顺序、剔除配置已删口径、补入未列口径按名排序；空顺序等价于旧版口径名排序，向后兼容。`main.rs::start_observation` 透传。
- **验证**：新增 2 个单测锁定 `resolve_caliber_order` 不变量（保留优先序+补尾排序、剔除失效项、空序=全量名排序）；`cargo check` 0 错；`cargo test` 14 passed / 0 failed（原 12 + 新 2），无回归。
