//! 事件队列: bot 的 Emitter 实现。有界容量, 满时丢最旧并计数。

use std::collections::VecDeque;
use std::sync::Mutex;

use openstory_bot::emit::{Emitter, Event};

pub struct EventQueue {
    cap: usize,
    inner: Mutex<Inner>,
}

struct Inner {
    buf: VecDeque<Event>,
    dropped: u64,
}

impl EventQueue {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            inner: Mutex::new(Inner {
                buf: VecDeque::new(),
                dropped: 0,
            }),
        }
    }

    /// Drain all pending events + the total dropped counter. The buffer is
    /// swapped out under the lock (O(1)) so the lock isn't held while the caller
    /// copies/renders the events — this keeps the bot's `emit` (which takes the
    /// same lock on the hot path) from stalling behind a large drain.
    pub fn take_all(&self) -> (Vec<Event>, u64) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let evs: Vec<Event> = std::mem::take(&mut inner.buf).into();
        let dropped = inner.dropped;
        (evs, dropped)
    }
}

impl Emitter for EventQueue {
    /// **毒化安全**: 用 `unwrap_or_else(|e| e.into_inner())` 而不是 `unwrap()`。
    ///
    /// 这条不是洁癖。事件队列是 bot 每一行日志的必经之路, 而 bot 的 future
    /// 里任何一处 panic 都会毒化这把锁 —— 于是**下一次** `emit` 跟着 panic。
    /// 在 `docs` 里那条最小复现是: 崩溃的 bot 在收尾时又发一条日志 → 二次
    /// panic → 整个进程 abort (所有账号一起死)。多开控制台里"一个号崩了
    /// 带走全部号"是最不能接受的失败模式。
    ///
    /// 内容本身在毒化时仍然完好 (是 `VecDeque` + 计数器), 所以拿回内部值
    /// 继续用是安全的。
    fn emit(&self, ev: Event) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.buf.len() >= self.cap {
            inner.buf.pop_front();
            inner.dropped += 1;
        }
        inner.buf.push_back(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openstory_bot::emit::{Category, Level};

    fn ev(text: &str) -> Event {
        Event::new(Level::Info, Category::Cmd, text.to_string())
    }

    #[test]
    fn drops_oldest_when_full() {
        let q = EventQueue::new(3);
        q.emit(ev("a"));
        q.emit(ev("b"));
        q.emit(ev("c"));
        q.emit(ev("d"));
        let (evs, dropped) = q.take_all();
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[0].text, "b");
        assert_eq!(evs[2].text, "d");
        assert_eq!(dropped, 1);
    }

    #[test]
    fn take_all_drains() {
        let q = EventQueue::new(8);
        q.emit(ev("x"));
        let (evs, _) = q.take_all();
        assert_eq!(evs.len(), 1);
        let (evs2, _) = q.take_all();
        assert!(evs2.is_empty());
    }
}
