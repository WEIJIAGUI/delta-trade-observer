//! 业务逻辑 / 领域层：标定配置、识别编排、OCR、数字识别（神经网络）、
//! 观察循环与落盘、识别记录解析。各模块之间通过 `crate::` 绝对路径
//! 互相引用（由 `main.rs` 统一 `pub use` 重导出，移动文件后路径不变）。

pub mod config;
pub mod ocr;
pub mod observe;
pub mod digit;
pub mod nn_classifier;
pub mod recognition;
pub mod records;
