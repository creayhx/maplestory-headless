//! 把日志正文里的裸 id 补上中文名。
//!
//! 引擎按"英文原文 + 结构化字段"输出 (见 `openstory_bot::emit`)。有些行只在
//! 正文里带 id (`npcid=1012002`), 没有对应的语义字段, 前端就把名字补在 id 后面。
//!
//! # 为什么名字解析是参数
//!
//! 字典的加载依赖进程工作目录 (`data/*.json`), 而 `cargo test` 跑成员 crate 的
//! 测试时工作目录是**该 crate 目录** —— 单元测试里查不到字典。把解析函数作为
//! 参数注入之后, 这段逻辑 (定位 id、幂等、越界保护) 可以完全独立地测试,
//! 也顺便为"名字表按会话传递"留好了接口。

use openstory_bot::names;

/// 支持"从正文 id 补名"的键与它们的字典。
///
/// 顺序即优先级: 取**表中第一个**在正文里出现的键 —— 与逐键 `find` 的
/// 历史行为一致 (不是"正文里最靠前的键")。
type NameFn = fn(i32) -> String;

const ENRICH_KEYS: &[(&str, NameFn)] = &[
    ("mobid=", names::mob_name_text as fn(i32) -> String),
    ("npcid=", names::npc_name_text),
    ("itemid=", names::item_name_text),
];

/// 用指定的名字解析函数补名。`resolve` 返回空串表示"查不到", 此时不补。
pub fn enriched_body_with(body: &str, resolve: impl Fn(&str, i32) -> String) -> String {
    for (key, _) in ENRICH_KEYS {
        let Some(pos) = body.find(key) else {
            continue;
        };
        let start = pos + key.len();
        // id 必须从数字开始; 越界/非 ASCII 起点都不能切开字符
        let Some(rest) = body.get(start..) else {
            continue;
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        let Ok(id) = digits.parse::<i32>() else {
            continue;
        };
        if id <= 0 {
            continue;
        }
        // 已经补过名 (`id·名字`) 就不再追加, 保证幂等
        if rest[digits.len()..].starts_with('·') {
            continue;
        }
        let name = resolve(key, id);
        if name.is_empty() {
            continue;
        }
        let mut out = body.to_string();
        out.insert_str(start + digits.len(), &format!("·{name}"));
        return out;
    }
    body.to_string()
}

/// 用进程默认名字表补名 (前端渲染链用这个)。
pub fn enriched_body(body: &str) -> String {
    enriched_body_with(body, |key, id| {
        let f = ENRICH_KEYS.iter().find(|(k, _)| *k == key).map(|(_, f)| *f);
        f.map(|f| f(id)).unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定的假字典: 与 `data/*.json` 的内容无关, 因此工作目录在哪都能跑。
    fn fake(_key: &str, id: i32) -> String {
        match id {
            100100 => "绿水灵".into(),
            1012002 => "比休斯".into(),
            2000000 => "红色药水".into(),
            _ => String::new(),
        }
    }

    fn enrich(body: &str) -> String {
        enriched_body_with(body, fake)
    }

    #[test]
    fn appends_name_after_mobid() {
        assert_eq!(enrich("mobid=100100 oid=5"), "mobid=100100·绿水灵 oid=5");
    }

    #[test]
    fn appends_name_after_npcid() {
        assert_eq!(enrich("npcid=1012002"), "npcid=1012002·比休斯");
    }

    #[test]
    fn itemid_is_handled_when_alone() {
        assert_eq!(
            enrich("drop itemid=2000000 qty=1"),
            "drop itemid=2000000·红色药水 qty=1"
        );
    }

    #[test]
    fn is_idempotent() {
        let once = enrich("mobid=100100 oid=5");
        let twice = enrich(&once);
        assert_eq!(once, twice, "已补过名不应再补一次");
    }

    /// 键的优先级由 `ENRICH_KEYS` 的顺序决定, 不是正文里谁更靠前。
    #[test]
    fn first_key_in_table_order_wins() {
        // 表里 mobid 排在 npcid 之前 -> 两个都在时处理 mobid,
        // 即使 npcid 在正文里更靠前也一样
        assert_eq!(
            enrich("npcid=1012002 mobid=100100"),
            "npcid=1012002 mobid=100100·绿水灵"
        );
        // 只出现 npcid 时正常处理
        assert_eq!(
            enrich("mobid=100100 npcid=1012002"),
            "mobid=100100·绿水灵 npcid=1012002"
        );
        // itemid 在表里最后, 有 mobid/npcid 时不会被处理
        assert_eq!(
            enrich("mobid=100100 itemid=2000000"),
            "mobid=100100·绿水灵 itemid=2000000"
        );
    }

    #[test]
    fn id_already_followed_by_dot_is_not_touched() {
        // 渲染过的文本 (id·名字) 再跑一次也不该重复追加
        assert_eq!(enrich("mobid=100100·绿水灵"), "mobid=100100·绿水灵");
    }

    #[test]
    fn no_key_is_unchanged() {
        assert_eq!(enrich("plain text"), "plain text");
        assert_eq!(enrich(""), "");
        assert_eq!(enrich("mobs=3"), "mobs=3");
    }

    #[test]
    fn non_numeric_id_is_left_alone() {
        // 后面不是数字 -> 不补, 也不能 panic
        assert_eq!(enrich("mobid=abc"), "mobid=abc");
        // 后跟中文: 绝不能把切片切在字符中间 (会 panic)
        assert_eq!(enrich("mobid=中文"), "mobid=中文");
        assert_eq!(enrich("npcid=，"), "npcid=，");
    }

    #[test]
    fn zero_and_negative_ids_are_skipped() {
        assert_eq!(enrich("mobid=0"), "mobid=0");
    }

    #[test]
    fn overflow_id_does_not_panic() {
        let huge = "mobid=99999999999999999999";
        assert_eq!(enrich(huge), huge);
    }

    #[test]
    fn unknown_id_is_left_without_marker_when_resolver_is_empty() {
        // 解析不出名字时不补 (让上层决定要不要显示"未知")
        assert_eq!(enrich("npcid=123456789"), "npcid=123456789");
    }

    #[test]
    fn id_at_end_of_string() {
        assert_eq!(enrich("mobid=100100"), "mobid=100100·绿水灵");
    }

    /// 进程默认入口在小字典下工作 (不强依赖 `data/`, 只验证不 panic 且幂等)。
    #[test]
    fn default_entry_is_safe_regardless_of_cwd() {
        // 工作目录可能在 crate 目录 (查不到字典) 或仓库根 (查得到), 两种都必须安全
        let out = enriched_body("mobid=100100");
        assert!(out.starts_with("mobid=100100"), "got {out}");
        assert_eq!(enriched_body(&out), out, "默认入口也必须幂等");
    }
}
