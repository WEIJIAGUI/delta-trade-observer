# 项目索引 / 状态缓存（PROJECT.md）

> 本文件是项目的「单一事实来源」索引。**进行任何修改前先读本文件 + CHANGELOG.md，再只读取要改的那个文件，不要重读整个项目，也不要读 `target/`。**

## 1. 项目目标与分阶段计划
《三角洲行动》交易行物价观察软件，分阶段迭代，不一步到位。
- **Phase 1（当前）**：GUI 基础框架——开始/结束观察、聚焦游戏窗口、置顶悬浮面板；并满足「中文不乱码 / 悬浮面板透明、不遮挡、不进截图」。不含价格抓取与图表。
- **Phase 2（待定）**：模拟人工操作，记录交易行商品价格 + 时间戳。
- **Phase 3（待定）**：数据持久化（CSV/SQLite）+ 图表可视化（egui plot / 导出）。

## 2. 技术栈与精确版本
| 组件 | 版本 | 作用 |
|------|------|------|
| Rust | 1.94（edition 2021） | 语言 |
| egui | 0.27 |  immediate-mode GUI |
| eframe | 0.27 | 原生窗口/事件循环（**默认 glow/OpenGL 后端**，非 wgpu） |
| windows | 0.52 | Win32 API：查找/激活游戏窗口；截图排除 `SetWindowDisplayAffinity`；毛玻璃 `DwmEnableBlurBehindWindow`；**窗口截图** `PrintWindow(PW_RENDERFULLCONTENT)`+`GetDIBits`；**可靠激活** `AttachThreadInput`+`SetWindowPos`+`GetForegroundWindow` 校验重试；**提示音** `PlaySoundW(SND_MEMORY)`（特性 `Win32_Graphics_Dwm`、`Win32_Graphics_Gdi`、`Win32_Storage_Xps`、`Win32_System_SystemInformation`、`Win32_System_Threading`、`Win32_Media_Audio`） |
| global-hotkey | 0.5 | 全局热键 F9（结束）/ F10（开始截图 或 截图态下触发截图） |
| image | 0.25 | 工具页选图解码；**模板匹配的输入解码**（`image::load_from_memory`） |
| png | 0.17 | PNG 编码保存截图（`Compression::Fast`，详见坑位 24） |
| rfd | 0.15 | 系统文件夹/文件选择器（`pick_folder` / `pick_file`） |

## 3. 文件地图
- `Cargo.toml` — 依赖声明（`egui`/`eframe` 0.27、`windows` 0.52、`global-hotkey` 0.5、`png` 0.17、`rfd` 0.15、`image` 0.25）。`image` 用于**工具页选图解码**与**模板匹配的输入解码**；`png`（`Compression::Fast`，见坑位 24）用于截图编码。**无 `serde_json`**——配置已代码化，`records.jsonl` 为手写格式化输出、读取为手写抽取
- `.cargo/config.toml` — 就近镜像源（rsproxy sparse，仅加速首次下载）
- `src/config.rs` — **口径 / 子弹 / 屏幕坐标配置（代码化，`config/info.json` 已废除）**：`Rect = [i32;4]`（`[x,y,w,h]`，原点＝客户区左上角，见坑位 16）；`GAUGE12: [BulletRects; 9]` 为 3×3 网格**公共基准**（12 Gauge 九格实测标定），其余 18 个口径经 `insert_caliber` **按书写顺序复用 `GAUGE12[i]`**，改坐标只需动一处；`BulletRects` 含 `name` / `price` 及派生的 `price_digits`（`split_price_5` 五等分为万/千/百/十/个）；`build_config()` 返回 `Config = HashMap<口径, HashMap<子弹名, BulletRects>>`。**副作用：改坐标需重新编译**（运行时不再依赖外部 JSON）。标定基准 `CALIB_REF_W = 1080`，运行时由 `observe::scale_rect` 按实际客户区宽度等比缩放
- `src/template_match.rs` — **单字符数字识别（模板匹配）**：只认一张单个数字图，返回 `"0"`~`"9"` 或 `""`，**不做位权 / 位数判断 / 多位拼接**（由调用方负责）。算法等价 `cv2.matchTemplate(..., TM_CCOEFF_NORMED)` 的归一化互相关（内联实现，不引入 `opencv` crate）；预处理＝灰度化 → 3×3 中值去噪 → 统一缩放到 `32×32` → `f32` 特征；对 10 个模板逐一算 NCC 取最高分，低于 `DEFAULT_MATCH_THRESHOLD = 0.65` 判为无法识别
- `assets/digits/` — **静态资源**：单字符数字模板 `0.png`~`9.png`（由使用者放入，规范见该目录 README）。目录定位＝env `DELTA_OBSERVER_DIGITS` → exe 目录向上 6 层 → cwd。**换模板需重启程序**
- `src/digit.rs` — 价格逐位链路：`recognize_price_digits` 把 5 个数位框（万/千/百/十/个）逐个交给 `template_match` 识别后**拼接**（前导空位跳过、中间空位补 `0`）；`recognize_single_digit` 只负责「裁出一张单字符图」。`DELTA_OBSERVER_DEBUG=1` 时落盘 `debug_test_digits/dN_raw.png` / `dN_bin.png` 供校准
- `src/screenshot.rs` — **截图模块（独立）**：整窗抓屏 `capture_window_rgba`（PrintWindow + GetDIBits）、客户区统一入口 `capture_client_rgba`（裁掉标题栏，保证标题栏绝不进截图）、区域裁剪 `crop_region`、PNG 保存 `save_rgba_png`（png 原生 `Compression::Fast`）、标注 `draw_rect_outline`/`draw_digit`、客户区偏移 `client_crop_rect`；类型化 `CapturedImage`（`crop`/`draw_*`/`save_png`）作现代接口面；坐标契约＝客户区左上角（与 `config` 一致）。
- `src/observe.rs` — 观察主流程：多轮循环（间隔 `interval_min` 分钟）遍历全部口径 → `type_into_search` 模拟检索 → `capture_client_rgba` 截客户区 → 名称格走 OCR（zh-CN）、价格格走逐位模板匹配 → 写 `prices/records.jsonl` + 调试图；`stop`(`AtomicBool`) 支持 F9 优雅中止，进度经 `mpsc` 回传
- `src/ocr.rs` — WinRT OCR（`Windows.Media.Ocr`，零外部依赖）：名称专用。管线＝近白二值化 → 4× 放大 → 引擎识别（`zh-CN`，缺失回退 `en-US`）
- `src/viz.rs` — 行情看板：解析 `prices/records.jsonl`，概览卡（当前价 + ▲/▼ 涨跌，**涨红跌绿**）+ 用 `Painter` 手绘的趋势图（一天/一周/一月）
- `src/hotkey_hook.rs` — 低级键盘钩子（`WH_KEYBOARD_LL`）：游戏独占焦点时仍能捕获 F9/F10 并透传按键，作为 `global-hotkey` 的兜底
- `src/main.rs` — GUI 主逻辑：`ObserverApp`（`Mode`: `Idle` / `Active(CaptureKind::{Observing,Screenshot,Testing})`；`tab` 0=开始观察/1=截图/2=行情看板/3=工具，`info`/`caliber_keys`/`selected` 为口径数据，`annotate_full` 为整窗标注图开关）；**两个独立面板组件** `ControlPanel`（Idle 主控制面板，含 4 个标签页，互不共用）/ `FloatingPanel`（毛玻璃悬浮面板，按 `CaptureKind` 分派：Observing/Screenshot 显示状态与进度），二者不共用同一组件；F9 结束（对任何激活态置位 `observe_stop` 并复位）、F10 开始截图（Idle）/ 触发截图（截图态）、开始观察/测试观察仅按钮触发；聚焦游戏窗口、ViewportCommand 交互、毛玻璃渲染；保存目录选择（rfd）+ 本地持久化（`save_path.cfg`）；info.json 加载（`load_info`，新 schema：顶层 `shared` 槽位 + 口径 `bullets` 数组）+ 口径下拉（`ComboBox`，`caliber_keys` 已排除顶层 `shared`）+ 区域归档（`run_capture` 新格式：读 `shared[i]` 与 `bullets[i]` 一一对应；旧格式兜底；`rect_from_json`/`sanitize`）；截图标签页新增「整窗标注图」复选框（仅口径模式生效）；`capture_screenshot` 改为在**后台线程**调用 `run_capture`（单次 PrintWindow 捕获 + 分批**并行编码保存**，并发度=`available_parallelism()` 核心数，纯 std 无新依赖）并经由 `capture_rx`(`mpsc`) 通道 + `poll_capture_result` 每帧轮询回传结果（UI 不卡顿）；任务进行中忽略新的 F10 触发，避免并发写同目录；`run_capture` 内部以 `Job`/`JobSpec` 收集**仅元数据**的任务，大块像素缓冲由 `Arc<[u8]>` 共享持有，标注路径在每个工作线程内「写时复制」(`buf.to_vec()`) 仅克隆所需单张，彻底消除原实现「先 `buf.clone()` N 次整屏并持有全部副本」的串行拷贝与内存放大。价格识别走 `digit.rs` 逐位（万/千/百/十/个）链路，**每一位由 `template_match.rs` 模板匹配识别单个字符**，`config.rs::split_price_5` 派生 5 个数位框；新增 `tool_tab`：图片选择(rfd)+ x/y/w/h 输入 + 预览叠加矩形定位截图范围；`training/` 目录为深度学习训练工程（见其 README/PROMPTS）
- `src/win.rs` — Win32 封装：`EnumWindows` 按标题关键字找可见窗口；`focus_window`（可靠激活：提 Z 序 + `AttachThreadInput` 绑定前台线程 + 校验重试）；`SetWindowDisplayAffinity` 截图排除；`DwmEnableBlurBehindWindow` 毛玻璃；`play_capture_chime`（内存合成双音提示音）。**截图功能已迁移至 `src/screenshot.rs`**

## 4. 关键决策与坑（避免重复踩坑）

> **历史语境提示**：本节按会话轮次（续 N）累积记录，**部分条目属历史决策留档**。凡提及 `config/info.json` / `serde_json` / `load_info` 的条目（坑位 15、16、18、25 等）仅用于记住「当时踩过的坑」——**配置现已代码化到 `src/config.rs`**（见第 3 节），那些文件路径与依赖在当前代码中均已不存在，请勿照搬。
1. **置顶**用 `ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop)`，不是 `AlwaysOnTop(bool)` 变体。
2. 本程序窗口标题 `SELF_TITLE = "交易行物价观察器"`，**绝不能**包含「三角洲行动」，否则 `EnumWindows` 会先命中自身窗口。
3. `global_hotkey` 的 `manager.register(hotkey)` **按值**接收 `HotKey`：必须先 `let id = hotkey.id` 取出 id 再注册，否则事件里无法比对。
4. 聚焦**游戏窗口**用 Win32 `SetForegroundWindow`+`ShowWindow(SW_RESTORE)`；归还**自身**焦点用 `ViewportCommand::Focus`（native only）。
5. 悬浮面板定位用 `ctx.input().screen_rect()` 取屏幕右上角。
6. **编译瓶颈是 CPU，不是网络**：依赖树（egui/glow/winit/windows）首次编译慢；eframe 已用轻量 glow 后端，无更优可换。
7. **中文乱码**：egui 0.27 默认字体不含中文 → 用 `install_cjk_font()` 加载系统 CJK 字体（`msyh.ttc` 等），运行时 `ctx.set_fonts` 追加到字体族回退。沙箱无中文字体文件，本地 Windows 才有。
8. **悬浮面板透明 + 不遮挡 + 不进截图 + 毛玻璃**的落地方式：
   - 启动即 `with_transparent(true)`（分层窗口，毛玻璃与截图排除的前置条件）。
   - 截图态：`CentralPanel` 用 `fill(Color32::TRANSPARENT)` + `FloatingPanel` 画半透明圆角卡片（毛玻璃观感）；`ViewportCommand::Decorations(false)` + `MousePassthrough(true)`（鼠标穿透，游戏可交互）；结束态恢复。
   - **真正毛玻璃**：`win::set_window_blur(hwnd, true)` → Win32 `DwmEnableBlurBehindWindow(hwnd, DWM_BLURBEHIND{hRgnBlur=NULL})` 背景模糊。`windows` 需特性 `Win32_Graphics_Dwm`（模块 `Win32::Graphics::Dwm`）。
   - **不进截图**：`win::set_exclude_from_capture(hwnd, true)` → Win32 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE=0x11)`。egui 无此能力，必须用 Win32。窗口必须是分层窗口（已满足）。
   - 取自身 HWND 复用 `win::find_window_by_keyword(SELF_TITLE)`（标题唯一）。
   - 两个激活态（观察 / 截图）均不放可点击按钮（穿透下点不到），结束统一按全局 **F9**。
   - **F9 即时响应**：eframe 0.27 默认 reactive 重绘，窗口失焦 + 鼠标穿透后不再主动重绘，导致全局热键事件积压、响应迟滞。修复：`update()` 末尾调用 `ctx.request_repaint_after(30ms)` 维持约 30fps 持续重绘，并在 `poll_hotkeys` 命中后 `request_repaint()`。此机制无需 `RunMode`（0.27 无该字段）。
9. **快捷键**：
   - **开始观察**：仅鼠标点击「开始观察」按钮触发（**F8 已解绑**，不再绑定任何快捷键）。
   - **开始截图**：鼠标点击「开始截图」按钮（**按钮本身不绑定任何快捷键**），或按 **F10**（= 激活三角洲窗口并进入截图状态）。
   - **截图触发**：在「截图」激活态下按 **F10** → 截取游戏窗口为 PNG 存入保存目录（时间戳文件名 `shot_YYYYMMDD_HHMMSS.png`）。
   - **结束（观察 / 截图通用）**：按 **F9**；立即恢复主控制面板与窗口（含 `Decorations`/`MousePassthrough`/`WDA` 还原、焦点归还）。原 F5/F6 已废弃。
10. **截图保存目录**：默认 `D:\pic`；通过 `rfd::FileDialog::pick_folder()` 系统文件夹选择器设置；持久化在可执行文件同目录 `save_path.cfg`（`load_save_path`/`persist_save_path`），每次启动自动沿用上次路径；`capture_window_to_png` 内 `create_dir_all` 确保目录存在。
11. **窗口截图实现**（Win32）：`win::capture_window_to_png(hwnd, dir)` 用 `GetWindowRect` 取尺寸 → `GetDC`/`CreateCompatibleDC`/`CreateCompatibleBitmap` 建内存 DC → `PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT=0x2)`（对 DX 游戏更可靠，失败回退 `BitBlt`）→ `GetDIBits` 取 BGRA → 转 RGBA → `image::save_buffer` 写 PNG。注意 windows-rs 0.52 的 API 细节：`PrintWindow`/`PRINT_WINDOW_FLAGS` 在 `Win32::Storage::Xps`（需特性 `Win32_Storage_Xps`）；`GetWindowRect`/`BitBlt`/`DeleteObject`/`DeleteDC` 返回 `Result<()>`（`is_err()`/`let _=`），`PrintWindow` 返回 `BOOL`（`.as_bool()`），`GetDC` 接收 `HWND`（非 `Option`），`GetLocalTime()` 直接返回 `SYSTEMTIME`，`BITMAPINFOHEADER.biCompression` 为 `u32`（用 `0`=BI_RGB）。
10. **两个独立面板组件**：`ControlPanel`（Idle）与 `FloatingPanel`（Capturing）各自 `show()`，`update()` 按 `mode` 分派；不再共用同一 `CentralPanel` 内的 `match`。
11. **egui/windows 编译坑**：`Shadow` 类型在 `eframe::egui::epaint::Shadow`，`offset` 为 `Vec2`、`blur/spread` 为 `f32`；`Margin::same` 取 `f32`；DWM 特性名是 `Win32_Graphics_Dwm`（非 `Win32_UI_Shell_DwmApi`）。
12. **仅截内容区（剔除标题栏）**：`capture_client_rgba`（`screenshot.rs`）先整窗 `PrintWindow` 抓屏，再用 `client_crop_rect`（`GetClientRect`+`ClientToScreen(0,0)`−`window_visible_rect` 可见窗口左上角）裁出内容区域。`GetClientRect`/`ClientToScreen` 均返回 `Result`/`BOOL`（`ClientToScreen` 在 `Win32::Graphics::Gdi`）。无边框/全屏游戏偏移为 0，等同整窗。注意：进程已在 `main` 声明 **Per-Monitor V2 DPI 感知**，GetClientRect/GetWindowRect/ClientToScreen/PrintWindow 一致以**设备像素**取舍；抓屏矩形另由 `window_visible_rect`（`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`，剔除 DWM 阴影）取可见窗口，缩放 ≠ 100% 下亦无错位/黑边（详见 CHANGELOG 2026-09-30 续）。
13. **激活 100% 可靠**：`SetForegroundWindow` 单独调用会被 Windows「前台锁超时」拒绝（调用方非前台进程时）。`focus_window` 的标准解法：先 `SetWindowPos(HWND_TOP, SWP_NOMOVE|SWP_NOSIZE|SWP_SHOWWINDOW)` 提 Z 序；再 `AttachThreadInput(本线程, GetWindowThreadProcessId(GetForegroundWindow()), TRUE)` 绑定输入队列 → `SetForegroundWindow` → `AttachThreadInput(..., FALSE)` 解绑；最后 `GetForegroundWindow()==hwnd` 校验 + 4×40ms 重试兜底。`AttachThreadInput`/`GetCurrentThreadId` 在 `Win32::System::Threading`；`GetForegroundWindow`/`GetWindowThreadProcessId`/`SetWindowPos`/`IsIconic`/`HWND_TOP`/`SWP_*` 在 `Win32::UI::WindowsAndMessaging`。
14. **截图成功反馈**：① 毛玻璃通知 `toast: Option<(String,bool,f64)>`（文字/是否成功/过期时间戳，`ctx.input(|i| i.time)` 计时），`egui::Area` 顶部居中半透明卡片，0.3s 进场 + 0.4s 退场淡入淡出，1.8s/2.2s 自动消失；位于本程序窗口内，自动被 `WDA_EXCLUDEFROMCAPTURE` 排除，**不进游戏截图**。② 提示音 `play_capture_chime()` 运行时合成双音上行 WAV（880→1318.5Hz，PCM16 单声道，6ms 淡入淡出），`PlaySoundW(PCWSTR::from_raw(wav.as_ptr() as *const u16), HMODULE::default(), SND_MEMORY|SND_NODEFAULT)` **同步**播放（不含 `SND_ASYNC`）→ 函数返回即播完，缓冲区随之释放，无悬垂/无泄漏，零外部资源。`SND_MEMORY=4`、`SND_NODEFAULT=2` 在 `Win32::Media::Audio`；`PCWSTR::from_raw` 在 `windows::core::PCWSTR`。
15. **标签页 + 口径区域归档（续8）**：主面板拆为「开始观察 / 截图」两个标签页（`tab: usize`，`selectable_label` 切换；两标签页内容各自独立渲染，与 `FloatingPanel` 悬浮面板绝不共用组件）。截图标签页默认全屏（`selected==0` → `capture_window_to_png` 整窗）；下拉加载 info.json 全部口径（顶层 key），选中后 F10 执行区域归档：遍历该口径下每个子弹，按 `name`/`price` 的 `[x,y,w,h]` 从整窗缓冲裁剪，归档至 `保存目录/<口径>/<子弹>/name.png`、`price.png`。**info.json 现为项目受控配置文件，置于 `config/info.json`**；`load_info` 通过 `config_candidates()` 从 exe 目录向上逐层追溯 `config/`（或同目录）、并覆盖 cwd，最高优先支持环境变量 `DELTA_OBSERVER_CONFIG` 指定任意路径/文件名；不再依赖桌面固定路径。加载失败则下拉提示「未找到口径配置」（见坑位 18）。
16. **坐标系契约（唯一真相，续13 终定）**：`info.json` 的 `[x,y,w,h]` 与默认 `shot_*.png` **均以「客户区 / 无标题栏截图」左上角为原点**（用户量取坐标即在此图上）。`capture_client_rgba` 先整窗 `PrintWindow`、再用 `client_crop_rect` 裁掉标题栏/边框，返回**客户区缓冲**（原点＝客户区左上角）；调用方直接使用 info.json 坐标裁剪/标注，**无需任何偏移换算**。历史上曾误把 info.json 当作「窗口左上角」并多减一次 `(ox,oy)`，导致窗口化（有标题栏）下每个区域整体上移 `oy`、左移 `ox` 而错位；续13 已删除该重定基（见坑位22）。全屏/无边框下偏移本为 0，两种契约等价，故该 bug 在满屏时不易察觉。
17. **global-hotkey 双事件陷阱（已踩并已修）**：`global-hotkey` 0.5.5 在 Windows 上，**一次物理按键会派发两个 `GlobalHotKeyEvent`**——`WM_HOTKEY` 时发 `HotKeyState::Pressed`，随后后台线程轮询 `GetAsyncKeyState==0` 再发 `HotKeyState::Released`（事件含 `pub state: HotKeyState`）。若 `poll_hotkeys` 用 `while let Ok(event)=...try_recv()` 无条件消费，则 F10 的 Pressed 与 Released 都会触发 `capture_screenshot` → **一次按键保存两份 PNG**。修复：循环体首行 `if event.state != HotKeyState::Pressed { continue; }`，只响应按下、忽略松开。`HotKeyState` 位于 `global_hotkey` 顶层（**不是** `global_hotkey::hotkey`，编译会 E0432）。F9 因 `stop()` 首行 `if self.mode==Mode::Idle { return; }` 把第二次（Released）吞成空操作，故不易察觉该缺陷——排查「重复触发」时勿被 F9 的正常假象误导。
18. **「未加载到 info.json」根因与修复（续15）**：原 `load_info` 只在「exe 同目录 / 桌面固定路径 / 当前目录」找文件，且**原 info.json 本身含 17 处尾随逗号（非法 JSON）**——`serde_json` 严格拒绝，导致解析恒失败、下拉显示未加载。修复：① 将配置文件作为项目受控文件放入 `config/info.json`；② 重写 `load_info`：`config_candidates()` 从 exe 目录向上逐层追溯 `config/`（最多 6 层）+ cwd，`DELTA_OBSERVER_CONFIG` 环境变量可指定任意路径/文件名；③ 清理原文件尾随逗号（用正则 `,(\s*[}\]])`→`\1`，不改变有效数据）。注意：手写的 JSON/AI 生成的配置极易带尾随逗号，提交前务必用严格解析校验（如 `python -c "import json;json.load(open(f))"`），否则 `serde_json` 会静默失败。
19. **默认全屏又出现标题栏（续16）**：`capture_window_to_png`（默认全屏入口）原本直接保存整窗缓冲，会带标题栏。修复：`client_crop_rect(hwnd)`（`GetClientRect`+`ClientToScreen(0,0)`−`window_visible_rect` 可见窗口左上角，得客户区相对抓屏缓冲原点的 `[x,y,w,h]`），在 `capture_window_to_png` 保存前 `crop_region` 裁掉非客户区。**区域归档不受影响**：它走 `capture_window_rgba` 整窗缓冲 + 客户区坐标，逻辑未动。`ClientToScreen` 属 `Win32::Graphics::Gdi`（非 WindowsAndMessaging，曾 E0432）。DPI：进程已声明 Per-Monitor V2 DPI 感知（见 CHANGELOG 2026-09-30 续），GetClientRect/GetWindowRect/ClientToScreen/PrintWindow 与 `window_visible_rect` 均为一致的设备像素，与 `PrintWindow` 缓冲对齐。
20. **整窗标注图（续11，续21 改为客户区，续13 校正坐标）**：截图标签页复选框 `annotate_full`（默认 false）。勾选且为口径模式（selected!=0）时，对口径下每个子弹除归档 `name.png`/`price.png` 外，另克隆**客户区**缓冲、用 `draw_rect_outline` 框出该子弹的 name（绿）/price（红）区域（厚 3px，越界夹紧），存为 `保存目录/<口径>/<子弹>/annotated.png`。**该标注图已剔除标题栏**，其框线坐标直接取 info.json（客户区原点），与 `name.png`/`price.png` 完全对齐（旧版曾对其再减 `(ox,oy)`，续13 已去掉）。默认全屏模式不生成标注图。
21. **彻底杜绝标题栏进入截图（续21，续13 校正坐标）**：此前 `annotated.png` 误用整窗缓冲（含标题栏）。改为统一经 `capture_client_rgba`：无论最大化/最小化(已还原)/全屏/多显示器/不同 Windows 版本，均先 `PrintWindow` 抓整窗（DX 游戏最可靠）再 `client_crop_rect` 裁掉非客户区；返回 `(client_buf, w, h)`（**不再返回 (ox,oy)**），`info.json` 坐标即客户区原点，**直接裁剪/标注，不再减偏移**。同时 `crop_region` 返回值改 `Option<(Vec<u8>, i32, i32)>`（实际裁剪宽高），消除越界夹紧后尺寸不匹配导致的图像错位/损坏风险。`capture_window_to_png`（默认全屏）也改用 `capture_client_rgba`。兜底：client_crop_rect 失败时退回整窗（极端场景，正常不会触发）。

22. **第1点坐标偏差根因与修复（续13）**：用户明确坐标契约＝「无标题栏截图（客户区）左上角」。原 `capture_screenshot` 的 `rebase = |r|[r[0]-ox,r[1]-oy,…]` 把 info.json 当「窗口左上角」多减一次偏移，窗口化（标题栏存在，`oy>0`）下区域整体上移 `oy`、左移 `ox`；全屏/无边框下 `oy=0` 等价故不易察觉——这就是「裁剪结果与预期不符」的本质。修复：删除 `rebase` 及 `capture_client_rgba` 的多余 `(ox,oy)` 返回值，全程以客户区左上为唯一原点（见坑位16）。**数据提示**：现有 `info.json` 必须按「无标题栏截图」左上角量取；若曾按带标题栏整窗量取，需在该图上重测或整体下移 `oy`。

23. **批量裁剪性能优化（续14）**：瓶颈在「主线程串行 PNG 编码」，尤其勾选「整窗标注图」时每个子弹额外编码一张完整客户区 PNG（N 张全屏 PNG 顺序压缩）+ 整窗缓冲 N 次克隆 → UI 冻结。`PrintWindow` 捕获本身只能单次、不可并行，故**并行点选在「编码落盘」**：收集所有图像为独立 `Vec<u8>` 的 Job，按 `available_parallelism()` 核心数分批 `std::thread::spawn` 编码保存（纯 std，无新依赖）；整段「捕获+并行保存」移入后台线程，经 `mpsc` 通道回传 `CaptureMsg`，主线程 `poll_capture_result` 每帧 `try_recv` 取结果 → UI 不卡顿。任务进行中（`capture_rx.is_some()`）忽略新 F10，防并发写同目录。坐标契约/标题栏剔除/归档逻辑不变。

24. **编码器替换 + 写时复制（续15，性能再优化）**：原 `save_rgba_png` 用 `image::save_buffer`（底层 PNG 默认 zlib level 6，编码慢）。改为 `png` 0.17 原生编码器 `Compression::Fast`（≈ level 1-3），批量截图编码耗时下降数倍，文件体积仅温和增大；并**移除 `image` 直接依赖**（仅作 PNG 写，已被 `png` 取代）。同时 `run_capture` 改造为「仅收集元数据」：`Job`/`JobSpec` 只存路径与裁剪/标注参数，大块像素缓冲由 `Arc<[u8]>` 共享持有；标注路径（`Annotate`）在每个工作线程内 `buf.to_vec()` **写时复制**仅克隆本张，彻底消除旧版「先 `buf.clone()` N 次整屏并持有全部副本」的串行拷贝与峰值内存放大（旧版峰值 ≈ N×整屏，新版 ≈ cores×整屏）。小心：`png::Compression` 0.17 **没有 `BestSpeed` 变体**（易误用报 E0599），最快档用 `Fast`；若要绝对最快可用 `Compression::Level(1)`。

25. **info.json 全局去重 schema（续16，结构变更）**：所有口径共用同一套 3×3 网格（经脚本验证 19 口径均为同一 9 槽位网格的前缀占用）。重构为：**顶层 `shared`** = 按网格顺序排列的 9 个槽位 `{name:[x,y,w,h], price:[x,y,w,h]}`（屏幕坐标唯一数据源）；**每个口径** = `{ "bullets": [子弹名0, 子弹名1, …] }`，第 i 个子弹复用 `shared[i]` 区域（`bullets[i]` ↔ `shared[i]`，按 JSON 排列顺序对应）。好处：统一修改坐标只改 `shared` 一处。配套 Rust 改动：`run_capture` 新格式走 `shared`+`bullets`，并保留旧格式（口径下每子弹直带 name/price）兜底；`ObserverApp::new` 构建 `caliber_keys` 时**必须排除顶层 `shared`**（否则下拉框会出现一个伪口径 "shared"）。注意：本次重构前原文件第 144 行存在**非法尾随逗号**（`serde_json` 会整体解析失败、加载不到配置），已借重构一并清洗；以后手改 JSON 务必用严格解析校验（如 `python -c "import json;json.load(open(f,encoding='utf-8'))"`）。备份见 `config/info.json.bak`。

## 5. 当前状态
- Phase 1 代码已完成（`src/main.rs`、`src/win.rs`）。
- 已通过 `cargo build`（dev）编译验证；产物：`target/debug/delta-trade-observer.exe`。沙箱无显示器，未做运行期验证。
- 依赖首次编译后缓存于 `target/` 与 `~/.cargo/registry`，后续增量编辑仅重编本 crate（秒级，见 CHANGELOG 实测 4.95s）。

## 6. 降低长期积分/资源消耗的约定（重要）
- **迭代用 dev 构建**：`cargo build`（快）；只有出最终分发版才 `cargo build --release`（更慢）。
- **不要重读整个项目**：读 `PROJECT.md` + `CHANGELOG.md` + 目标文件即可。
- **不读 `target/`**：编译产物巨大且与逻辑无关（已由 `.gitignore` 排除）。
- **保持依赖精简**：新增依赖前评估其编译成本。
- **每次修改后**在 `CHANGELOG.md` 追加记录（日期 / 改了什么 / 为什么 / 涉及文件），使后续会话无需重载上下文。
