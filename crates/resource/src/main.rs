use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use openstory_resource::map;
use openstory_resource::string;

#[derive(Parser)]
#[command(name = "openstory-resource")]
#[command(about = "MapleStory WZ data exporter for openstory-bot")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Export all string tables and portal data to JSON files
    Export {
        /// Path to WZ extracted data directory (e.g. D:\Game\client\data)
        #[arg(long)]
        data: PathBuf,

        /// Output directory for JSON files
        #[arg(long)]
        out: PathBuf,
    },

    /// List portals for a specific map
    Portals {
        /// Path to WZ extracted data directory
        #[arg(long)]
        data: PathBuf,

        /// Map ID to inspect
        #[arg(long)]
        map_id: u32,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Export { data, out } => cmd_export(&data, &out),
        Commands::Portals { data, map_id } => cmd_portals(&data, map_id),
    }
}

fn cmd_export(data: &std::path::Path, out: &std::path::Path) {
    if !data.exists() {
        eprintln!("error: data directory does not exist: {}", data.display());
        process::exit(1);
    }
    if !out.exists() {
        std::fs::create_dir_all(out).expect("failed to create output directory");
    }

    println!("exporting from: {}", data.display());
    println!("output to:      {}", out.display());

    // --- String tables ---
    // Note: Skill is excluded — the original resource-loader doesn't export it.
    // skill.json is manually maintained with server-specific names.
    let string_tables: &[(&str, &str)] = &[
        ("item", "Item"),
        ("map", "Map"),
        ("mob", "Mob"),
        ("npc", "Npc"),
        ("skill", "Skill"),
    ];

    for (file_key, wz_name) in string_tables {
        let label = format!("{file_key}.json");
        print!("  {label} ... ");
        match string::load_string_by_name(data, wz_name) {
            Ok(map) => {
                let count = map.len();
                let json = serde_json::to_string(&map).expect("serialize failed");
                let path = out.join(format!("{file_key}.json"));
                std::fs::write(&path, json).expect("write failed");
                println!("ok ({count} entries)");
            }
            Err(e) => {
                println!("FAILED: {e}");
            }
        }
    }

    // --- Item table (multi-file merge) ---
    {
        let label = "item.json (multi)";
        print!("  {label} ... ");
        match string::load_string_item(data) {
            Ok(map) => {
                let count = map.len();
                let json = serde_json::to_string(&map).expect("serialize failed");
                let path = out.join("item.json");
                std::fs::write(&path, json).expect("write failed");
                println!("ok ({count} entries)");
            }
            Err(e) => {
                println!("FAILED: {e}");
            }
        }
    }

    // --- Portals ---
    {
        let label = "portals.json";
        print!("  {label} ... ");
        match map::load_all_portals(data) {
            Ok(portals) => {
                let map_count = portals.len();
                let total_portals: usize = portals.values().map(|v| v.len()).sum();
                let json = serde_json::to_string(&portals).expect("serialize failed");
                let path = out.join("portals.json");
                std::fs::write(&path, json).expect("write failed");
                println!("ok ({map_count} maps, {total_portals} portals)");
            }
            Err(e) => {
                println!("FAILED: {e}");
            }
        }
    }

    println!("done.");
}

fn cmd_portals(data: &std::path::Path, map_id: u32) {
    if !data.exists() {
        eprintln!("error: data directory does not exist: {}", data.display());
        process::exit(1);
    }

    match map::load_map_portals(data, map_id) {
        Ok(portals) => {
            if portals.is_empty() {
                println!("no portals found for map {map_id}");
                return;
            }
            println!("map {map_id} — {} portal(s):", portals.len());
            println!("{:<4} {:<20} {:<12} {:<6} {:<6}", "#", "名称", "目标地图", "X", "Y");
            println!("{}", "-".repeat(56));
            for (i, p) in portals.iter().enumerate() {
                println!(
                    "{:<4} {:<20} {:<12} {:<6} {:<6}",
                    i + 1,
                    p.name,
                    p.target_map,
                    p.x,
                    p.y
                );
            }
        }
        Err(e) => {
            eprintln!("error loading portals for map {map_id}: {e}");
            process::exit(1);
        }
    }
}
