//! Offline config inspector: prints the MERGED view of a config chain
//! (includes resolved, by-id array merging applied) exactly as the bot
//! would load it — no server needed.
//!
//! Usage:
//!   cargo run --example cfg_dump -- --config profiles/本地服_100000003.json
//!   cargo run --example cfg_dump -- --config a.json --config b.json
use openstory_bot::runtime_config::{set_config_paths, RuntimeConfig};

fn main() {
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--config" {
            match args.next() {
                Some(p) => paths.push(std::path::PathBuf::from(p)),
                None => eprintln!("--config needs a path"),
            }
        }
    }
    if paths.is_empty() {
        paths.push(std::path::PathBuf::from("config.json"));
    }
    set_config_paths(paths);
    let cfg = RuntimeConfig::load();

    println!("== merged view ==");
    println!(
        "tick_ms={} potion_cooldown={} reconnect={}/{}/{} hunt.damage={}",
        cfg.tick_ms,
        cfg.potion_cooldown,
        cfg.reconnect,
        cfg.reconnect_delay,
        cfg.reconnect_max,
        cfg.hunt.damage
    );
    println!("potion: {} rule(s)", cfg.potion.len());
    for p in &cfg.potion {
        println!("  {:?} <{}% itemids={:?}", p.stat, p.threshold_pct, p.itemids);
    }
    println!("login: {}@{}", cfg.login.account, cfg.login.ip);
    println!("\nrules ({}):", cfg.rules.len());
    for r in &cfg.rules {
        println!(
            "  {} enabled={} cooldown={} when={} then={:?}",
            r.id, r.enabled, r.cooldown, r.when, r.then
        );
    }
    println!("\ntasks ({}):", cfg.tasks.len());
    for t in &cfg.tasks {
        println!("  {} priority={} steps={}", t.id, t.priority, t.steps.len());
        for (i, s) in t.steps.iter().enumerate() {
            println!(
                "    {}. cmd={:?} wait={:?} timeout={:?}",
                i + 1,
                s.cmd,
                s.wait,
                s.timeout
            );
        }
    }
    println!("\ngroups ({}):", cfg.groups.len());
    for g in &cfg.groups {
        println!("  {} enabled={} exclusive={} rules={}", g.id, g.enabled, g.exclusive, g.rules.len());
        for r in &g.rules {
            println!("    {} cooldown={} when={}", r.id, r.cooldown, r.when);
        }
    }
}
