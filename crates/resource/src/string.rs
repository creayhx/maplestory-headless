use std::{collections::HashMap, error::Error, path::Path, sync::Mutex};

use rayon::prelude::*;
use wz_reader::{
    property::resolve_string_from_node, util::walk_node, WzNode, WzNodeArc, WzNodeCast,
};

fn normalize_key(key: &str) -> String {
    key.trim_start_matches('0').to_string()
}

/// Load all item string tables (Item, Cash, Consume, Eqp, Etc, Ins, Pet) into
/// a flat `HashMap<id_string, name_string>`. Skips IDs < 100000 (Hair/Face).
pub fn load_string_item(data_path: &Path) -> Result<HashMap<String, String>, Box<dyn Error>> {
    let mut nodes = vec![];
    let string_path = data_path.join("String");
    for name in &[
        "Item.img",
        "Cash.img",
        "Consume.img",
        "Eqp.img",
        "Etc.img",
        "Ins.img",
        "Pet.img",
    ] {
        let file_path = string_path.join(name);
        if file_path.exists() {
            let node: WzNodeArc = WzNode::from_img_file(file_path, None, None)?.into();
            nodes.push(node);
        }
    }

    let items: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());

    nodes.par_iter().for_each(|node| {
        walk_node(node, true, &|node| {
            let node_read = node.read().unwrap();
            if node_read.try_as_string().is_some() && node_read.name.contains("name") {
                if let Ok(value) = resolve_string_from_node(node) {
                    if !value.is_empty() {
                        let id = node_read
                            .parent
                            .upgrade()
                            .unwrap()
                            .read()
                            .unwrap()
                            .name
                            .to_string();
                        if let Ok(id_num) = id.parse::<u32>() {
                            if id_num < 100_000 {
                                return;
                            }
                        }
                        let key = normalize_key(&id);
                        if key.is_empty() {
                            return;
                        }
                        if let Ok(mut c) = items.lock() {
                            c.insert(key, value.trim().to_string());
                        }
                    }
                }
            }
        });
    });

    let c = items.lock().unwrap();
    Ok(c.clone())
}

/// Load a single `String/<name>.img` table (e.g. Map, Npc, Mob, Skill).
/// Extracts nodes with "name" in the field name.
pub fn load_string_by_name(
    data_path: &Path,
    name: &str,
) -> Result<HashMap<String, String>, Box<dyn Error>> {
    let file_path = data_path.join(format!("String/{}.img", name));
    if !file_path.exists() {
        return Ok(HashMap::new());
    }

    let items: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
    let node: WzNodeArc = WzNode::from_img_file(file_path, None, None)?.into();

    walk_node(&node, true, &|node| {
        let node_read = node.read().unwrap();
        if node_read.try_as_string().is_some()
            && (node_read.name.contains("name") || node_read.name.contains("mapName"))
        {
            if let Ok(value) = resolve_string_from_node(node) {
                if !value.is_empty() {
                    let id = node_read
                        .parent
                        .upgrade()
                        .unwrap()
                        .read()
                        .unwrap()
                        .name
                        .to_string();
                    let key = normalize_key(&id);
                    if key.is_empty() {
                        return;
                    }
                    if let Ok(mut c) = items.lock() {
                        c.insert(key, value.trim().to_string());
                    }
                }
            }
        }
    });

    let c = items.lock().unwrap();
    Ok(c.clone())
}
