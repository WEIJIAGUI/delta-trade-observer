//! 平台 / 系统底层：Win32 窗口操作、截图裁剪、全局键盘钩子。
//! 与业务无关、可复用的底层能力，通过 `crate::` 暴露给上层。

pub mod win;
pub mod screenshot;
pub mod hotkey_hook;
pub mod dxgi_capture;
