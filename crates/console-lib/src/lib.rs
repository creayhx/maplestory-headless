//! openstory-console-lib — 控制台前端的共享层。
//!
//! 这一层承载**与前端形态无关**的展示与交互逻辑, 供两个前端复用:
//! - 本地多开控制台 (`crates/console` = `openstory-console`, ratatui 终端界面)
//! - 未来的服务器版前端 (不依赖 ratatui, 直接用核心 + `emit` JSON + `Snapshot`)
//!
//! # 分层
//!
//! ```text
//! openstory-bot (核心: 协议/会话/状态/命令解析/事件)   ← 零 UI 依赖
//!        ↑
//! openstory-console-lib (本 crate: 文本/配色/中文/日志/命令规格/补全/快照)
//!        ↑
//! openstory-console (本地终端控制台; 远期: 浏览器版前端)
//! ```
//!
//! # 为什么单独成 crate
//!
//! 驱动因素是**降低每次迭代的验证面积**: 后端每新增一次功能 (新指令、
//! 新状态字段、新中文渲染规则), 多一个前端就多一套回归测试。把"指令怎么用、
//! 参数是什么、事件怎么显示"这些唯一实现放在共享层, 前端只负责排版,
//! 新增功能的适配成本才可能为 0。

pub mod command_spec;
pub mod completion;
pub mod enrich;
pub mod eventq;
pub mod logbuf;
pub mod snapshot;
pub mod text;
pub mod theme;
pub mod zh;

/// 配置档案目录名 (相对工作目录)。
///
/// 放在共享层而不是各前端里: 补全 (`profiles add <路径>` 的候选) 与将来
/// 服务器版前端的档案浏览都要用它, 而两处各写一个字面量迟早会漂移。
pub const fn profiles_dir() -> &'static str {
    "profiles"
}
