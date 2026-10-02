//! 用户界面层：三个 Tab 的面板（开始观察 / 毛玻璃悬浮 / OCR 测试 / 价格可视化）、
//! 自绘图表与公共视觉样式。仅负责采集输入与展示结果，识别等重逻辑统一走
//! `crate::domain` 模块。

pub mod theme;
pub mod control_panel;
pub mod floating_panel;
pub mod ocr_test_panel;
pub mod price_viz_panel;
