//! `openstory-tui` 可执行入口 —— 老命令行 / 快捷方式 / 文档里的名字。
//!
//! 与 `openstory-console` **同一份实现** (`openstory_console::run`), 只是可执行
//! 文件名不同: 不是转发脚本 (那会多一个进程, 且二进制被改名/挪走时会静默失效),
//! 而是**同一个函数的第二个入口**, 行为由编译器保证一致。
//!
//! 去掉它: 从 `Cargo.toml` 删掉对应的 `[[bin]]` 并删除本文件。

fn main() {
    openstory_console::run();
}
