//! 导游模式协调器（领域无关，跨模块复用）。
//!
//! 后台观察线程在「计划执行某一步」前调用 [`Tour::ask`] 展示会议文字形式的计划并**阻塞**，
//! 直到主线程 UI 检测到使用者按下回车、调用 [`Tour::confirm`] 才放行。导游模式关闭时
//! [`Tour::ask`] 为无操作，任何步骤照常执行、不展示、不阻塞。
//!
//! 设计为可复用单元：任何后续新增的自动化操作（如新类型的购买 / 交互）只需在「执行前」调用
//! `ask(计划文字)`，即可自动获得「先展示计划、回车确认再执行」的导游能力，无需各自实现确认 UI。
//!
//! 确认采用「自增序号 + 一次性确认」模型，避免使用者连按回车导致后续步骤被误放行：
//! `confirm` 只确认「此刻正在展示」的那一条计划，过期 / 提前多按的回车均为空操作。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// 导游模式协调器。内部状态以 `Arc` 持有，[`Clone`] 仅增加引用计数，
/// 后台线程与 UI 主线程共享同一份「待确认计划」与「确认信号」。
pub struct Tour {
    enabled: bool,
    /// 当前正在展示、等待确认的计划：`(自增序号, 会议文字)`；无则为 `None`。
    pending: Arc<Mutex<Option<(u64, String)>>>,
    /// `(已确认序号, 条件变量)`：线程在条件变量上等待，直到 `已确认序号 == 当前计划序号`。
    ack: Arc<(Mutex<u64>, Condvar)>,
}

/// 计划序号发生器：每次 `ask` 取一个全局唯一、单调递增的值，作为一次性确认的凭据。
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

impl Tour {
    /// 关闭态：所有 `ask` 立即放行、不产生任何提示；UI 也不会渲染导游层。
    pub fn disabled() -> Self {
        Self::make(false)
    }

    /// 开启态：构造全新的提示 / 确认通道。
    pub fn new() -> Self {
        Self::make(true)
    }

    fn make(enabled: bool) -> Self {
        Self {
            enabled,
            pending: Arc::new(Mutex::new(None)),
            ack: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    /// 当前是否开启导游模式。
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 后台逻辑调用：展示计划文字（会议文字形式）并阻塞，直到使用者在 UI 上回车确认。
    ///
    /// 非导游模式（[`Tour::disabled`]）下立即返回，不阻塞、不展示。
    ///
    /// 阻塞期间若观察被 `stop` 中止，调用方应在 `ask` 返回后**立即检查 `stop` 并退出**，
    /// 避免「被 F9 唤醒后仍执行了本应取消的动作」。UI 侧在 F9 时会主动 [`confirm`](Self::confirm)
    /// 放行卡住的线程；但放行后线程仍需自行依据 `stop` 决定放弃执行。
    pub fn ask(&self, text: &str) {
        if !self.enabled {
            return;
        }
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut p) = self.pending.lock() {
            *p = Some((id, text.to_string()));
        }
        let (lock, cvar) = &*self.ack;
        let mut ack_id = lock.lock().unwrap();
        while *ack_id != id {
            ack_id = cvar.wait(ack_id).unwrap();
        }
        // 已确认：清除提示，避免 UI 残留。
        if let Ok(mut p) = self.pending.lock() {
            *p = None;
        }
    }

    /// 当前是否在等待确认（用于 UI 渲染导游层）。返回提示文字副本；无则返回 `None`。
    pub fn pending_text(&self) -> Option<String> {
        self.pending
            .lock()
            .ok()
            .and_then(|p| p.as_ref().map(|(_, t)| t.clone()))
    }

    /// UI 调用：使用者按下回车后，放行**当前**等待的计划步骤。
    ///
    /// 仅确认「此刻正在展示」的那一条计划；若此刻没有待确认计划（如使用者提前多按的回车），
    /// 则为空操作，不会误放行后续步骤。
    pub fn confirm(&self) {
        let (lock, cvar) = &*self.ack;
        if let Ok(mut ack_id) = lock.lock() {
            // 仅确认当前展示中的计划序号；无展示中计划时不动 ack_id（空操作）。
            let cur = self
                .pending
                .lock()
                .ok()
                .and_then(|p| p.as_ref().map(|(id, _)| *id));
            if let Some(id) = cur {
                *ack_id = id;
                cvar.notify_one();
            }
        }
    }
}

impl Clone for Tour {
    fn clone(&self) -> Self {
        Self {
            enabled: self.enabled,
            pending: self.pending.clone(),
            ack: self.ack.clone(),
        }
    }
}
