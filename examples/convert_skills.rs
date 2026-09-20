//! 一次性转换脚本: data/Skill.img.json (HaRepacker 导出) → data/skill.json
//! (扁平 `{"skillid": "技能名"}` 键值对, 与 map/npc/mob/item.json 一致)。
//! 注意: img 键是零填充的 ("0001000"), 须归一化为整数 id (1000) —
//! 服务端发送的技能 id 就是整数, 且 serde_json 不接受 JSON 数字前导零。
use std::collections::BTreeMap;
use std::io::Read;

fn main() {
    let mut s = String::new();
    std::fs::File::open("data/Skill.img.json")
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    let obj = v.as_object().unwrap();
    let mut out: BTreeMap<i32, String> = BTreeMap::new();
    for (id, node) in obj {
        let Ok(idn) = id.parse::<i32>() else {
            continue;
        };
        if let Some(name) = node
            .get("name")
            .and_then(|n| n.get("_value"))
            .and_then(|v| v.as_str())
        {
            if !name.is_empty() {
                out.insert(idn, name.to_string());
            }
        }
    }
    let s2 = serde_json::to_string(&out).unwrap();
    std::fs::write("data/skill.json", s2).unwrap();
    eprintln!("skills written: {}", out.len());
}
