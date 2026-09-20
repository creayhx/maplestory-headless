use std::{collections::HashMap, error::Error, path::Path};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use wz_reader::{
    property::resolve_string_from_node, util::node_util::parse_node, WzNode, WzNodeArc, WzNodeCast,
};

use super::utils::{get_padding_zero, map_bucket};

/// A single portal entry within a map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortalEntry {
    /// Portal name (e.g. "sp", "west00", "in00").
    pub name: String,
    /// Target map ID this portal leads to.
    pub target_map: i32,
    /// X position of the portal on the map.
    pub x: i32,
    /// Y position of the portal on the map.
    pub y: i32,
}

/// Parse a single portal child node, returning `Some(PortalEntry)` if valid.
fn parse_portal_node(node: &WzNodeArc) -> Option<PortalEntry> {
    let node_read = node.read().unwrap();
    let mut name = String::new();
    let mut target_map: i32 = 0;
    let mut x: i32 = 0;
    let mut y: i32 = 0;
    let mut has_tm = false;
    let mut has_script = false;

    for (field, child) in node_read.children.iter() {
        let child_read = child.read().unwrap();
        match field.as_str() {
            // WZ uses "pn" for portal name
            "pn" => {
                name = resolve_string_from_node(child).unwrap_or_default();
            }
            "tm" => {
                target_map = *child_read.try_as_int().unwrap_or(&0);
                has_tm = true;
            }
            "x" => {
                x = *child_read.try_as_int().unwrap_or(&0);
            }
            "y" => {
                y = *child_read.try_as_int().unwrap_or(&0);
            }
            "script" => {
                has_script = !resolve_string_from_node(child).unwrap_or_default().is_empty();
            }
            _ => {}
        }
    }

    if name.is_empty() || !has_tm {
        return None;
    }

    // Skip the invisible "spawn" portal (tm = 999999999) UNLESS:
    // - it has a script (script portals like in00 for boss entry)
    // - it is the "sp" spawn point portal (used as standing point reference)
    if target_map == 999_999_999 && !has_script && name != "sp" {
        return None;
    }

    Some(PortalEntry {
        name,
        target_map,
        x,
        y,
    })
}

/// Load all portals from a single map WZ file.
pub fn load_map_portals(data_path: &Path, map_id: u32) -> Result<Vec<PortalEntry>, Box<dyn Error>> {
    let bucket = map_bucket(map_id);
    let padded = get_padding_zero(map_id, 9);
    let map_path = data_path.join(format!("Map/Map/Map{}/{padded}.img", bucket));

    if !map_path.exists() {
        return Ok(Vec::new());
    }

    let node: WzNodeArc = WzNode::from_img_file(&map_path, None, None)?.into();
    parse_node(&node)?;

    let mut portals = Vec::new();
    let node_read = node.read().unwrap();

    if let Some(portal_node) = node_read.children.get("portal") {
        let portal_read = portal_node.read().unwrap();
        for child in portal_read.children.values() {
            if let Some(entry) = parse_portal_node(child) {
                portals.push(entry);
            }
        }
    }

    Ok(portals)
}

/// Scan all map files and extract portals for every map.
/// Returns `HashMap<map_id_string, Vec<PortalEntry>>`.
pub fn load_all_portals(
    data_path: &Path,
) -> Result<HashMap<String, Vec<PortalEntry>>, Box<dyn Error>> {
    let map_base = data_path.join("Map/Map");
    let mut result: HashMap<String, Vec<PortalEntry>> = HashMap::new();

    // Collect all .img files across Map0..Map9
    let mut img_files: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&map_base)? {
        let entry = entry?;
        let dir_name = entry.file_name().to_string_lossy().to_string();
        // Match Map0, Map1, ..., Map9
        if !dir_name.starts_with("Map") || dir_name == "MapHelper.img" || dir_name == "Map" {
            continue;
        }
        let dir_path = entry.path();
        if !dir_path.is_dir() {
            continue;
        }
        for img_entry in std::fs::read_dir(&dir_path)? {
            let img_entry = img_entry?;
            let file_name = img_entry.file_name().to_string_lossy().to_string();
            if let Some(id_str) = file_name.strip_suffix(".img") {
                if let Ok(id) = id_str.parse::<u32>() {
                    img_files.push((id, img_entry.path()));
                }
            }
        }
    }

    // Parse in parallel
    let portals: Vec<(u32, Vec<PortalEntry>)> = img_files
        .par_iter()
        .filter_map(|(id, _path)| {
            match load_map_portals(data_path, *id) {
                Ok(portals) if !portals.is_empty() => Some((*id, portals)),
                _ => None,
            }
        })
        .collect();

    for (id, portals) in portals {
        result.insert(id.to_string(), portals);
    }

    Ok(result)
}
