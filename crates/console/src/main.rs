//! `openstory-console` 可执行入口。
//!
//! 全部实现都在 `openstory-console` 这个 lib crate 里 (`src/lib.rs`), 本文件
//! 只做转发。`openstory-tui` (`src/bin/tui.rs`) 是同一个函数的第二个入口 ——
//! 两者共用同一份编译产物, 不会各编一遍, 也不会出现行为分叉。
//!
//! 为什么这样分: 见 `src/lib.rs` 的模块文档。

fn main() {
    openstory_console::run();
}
