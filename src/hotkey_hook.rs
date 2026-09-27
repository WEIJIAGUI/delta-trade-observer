//! 低级键盘钩子（WH_KEYBOARD_LL）：即使游戏独占键盘焦点，也能捕获 F9 / F10。
//!
//! 为什么需要它：`global-hotkey`（基于 `RegisterHotKey`）在「本窗口有焦点」时可靠，
//! 但进入激活态时 `start()` 会把键盘焦点交给游戏窗口（`win::focus_window`），
//! 游戏以无边框/全屏原始输入方式持有焦点时，`RegisterHotKey` 的消息**不会**投递到本进程，
//! 导致 F9/F10 失效、无法退出激活态。
//!
//! 低级键盘钩子由系统在按键分发给任何窗口**之前**于本线程消息循环回调，
//! 与焦点归属无关（射击游戏常态下依然生效），作为全局快捷键的可靠兜底。
//! 回调仅置位原子标志并 `CallNextHookEx` 透传按键（绝不吞键，不影响游戏输入）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, KBDLLHOOKSTRUCT, SetWindowsHookExW, WH_KEYBOARD_LL, HHOOK,
};

/// F9（结束）按键标志：钩子置位，主循环取走（消费式）。
static F9_DOWN: AtomicBool = AtomicBool::new(false);
/// F10（截图）按键标志。
static F10_DOWN: AtomicBool = AtomicBool::new(false);
/// 钩子句柄（进程存活期保持，不卸载）。
static HOOK: OnceLock<HHOOK> = OnceLock::new();

/// 安装低级键盘钩子。须在拥有消息循环的线程（主线程）调用一次。
/// 失败（极少见）仅影响「焦点被游戏独占时 F9/F10 的兜底」，不影响其它功能。
pub fn install() {
    // 低级钩子必须将 hmod 置 NULL（HINSTANCE(0)）、dwThreadId 置 0（钩取本桌面全部线程）。
    let r = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), HINSTANCE(0), 0) };
    if let Ok(h) = r {
        let _ = HOOK.set(h);
    }
}

/// 消费式读取 F9 标志（读到即清零）。
pub fn take_f9() -> bool {
    F9_DOWN.swap(false, Ordering::Relaxed)
}

/// 消费式读取 F10 标志。
pub fn take_f10() -> bool {
    F10_DOWN.swap(false, Ordering::Relaxed)
}

/// 低级键盘钩子回调（unsafe extern "system"，由本线程消息循环调用）。
extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // code < 0 时必须透传；仅处理按键按下。
    if code >= 0 {
        let msg = wparam.0 as u32;
        // WM_KEYDOWN = 0x0100，WM_SYSKEYDOWN = 0x0104
        if msg == 0x0100 || msg == 0x0104 {
            // lparam 为 *const KBDLLHOOKSTRUCT；vkCode 即虚拟键码。
            let ks = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
            match ks.vkCode {
                0x78 => F9_DOWN.store(true, Ordering::Relaxed),  // VK_F9
                0x79 => F10_DOWN.store(true, Ordering::Relaxed), // VK_F10
                _ => {}
            }
        }
    }
    // 关键：透传按键，绝不拦截，保证游戏正常接收输入。
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
