//! Client key hunter: locate the AES round-key table inside a MapleStory
//! client executable.
//!
//! The client stores three fixed 256-byte tables: the AES round-key table
//! (customized by some servers), the S-box and the IV-mixing table (usually
//! stock). Usage:
//!
//! ```text
//! cargo run --release --example keyfind -- <client.exe>
//! ```
//!
//! Output:
//! - offsets of the stock tables (S-box / IV-mix) = anchors for the crypto
//!   data section;
//! - "KEY FOUND" if the stock MapleStory key is present at all;
//! - every 16-byte-aligned 256-byte window near an anchor that looks like a
//!   key table (high entropy / mostly unique bytes), with its first 32 bytes
//!   as hex — feed any candidate to `probe` as its `<aes-key-hex512>` argument
//!   to verify it live against the server.

use std::env;
use std::fs;

const KEY_HEAD: [u8; 16] = [
    0x13, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0xB4, 0x00, 0x00, 0x00,
];
const SBOX_HEAD: [u8; 16] = [
    0x63, 0x7C, 0x77, 0x7B, 0xF2, 0x6B, 0x6F, 0xC5, 0x30, 0x01, 0x67, 0x2B, 0xFE, 0xD7, 0xAB, 0x76,
];
const IVMIX_HEAD: [u8; 16] = [
    0xEC, 0x3F, 0x77, 0xA4, 0x45, 0xD0, 0x71, 0xBF, 0xB7, 0x98, 0x20, 0xFC, 0x4B, 0xE9, 0xB3, 0xE1,
];

fn find(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    hay.windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(i, _)| i)
        .collect()
}

/// Heuristic: a pre-expanded AES round-key table looks like 256 bytes of
/// near-unique, near-nonzero data (a real table has ~250 distinct values).
fn looks_like_key_table(win: &[u8]) -> bool {
    use std::collections::HashSet;
    let mut set = HashSet::with_capacity(256);
    let mut nonzero = 0usize;
    for &b in win {
        set.insert(b);
        if b != 0 {
            nonzero += 1;
        }
    }
    set.len() >= 240 && nonzero >= 240
}

fn hex32(b: &[u8]) -> String {
    b.iter()
        .take(32)
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join("")
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let path = match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("usage: keyfind <client.exe>");
            std::process::exit(2);
        }
    };
    let data = match fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("read {path}: {e}");
            std::process::exit(2);
        }
    };
    println!("scanning {} ({} bytes)", path, data.len());

    let keys = find(&data, &KEY_HEAD);
    if !keys.is_empty() {
        println!("KEY FOUND (stock MapleStory table) at offsets: {:?}", keys);
    } else {
        println!("stock MapleStory key table NOT found — the key is customized");
    }

    let sboxes = find(&data, &SBOX_HEAD);
    let ivmixes = find(&data, &IVMIX_HEAD);
    println!("S-box (stock) at offsets: {:?}", sboxes);
    println!("IV-mix table (stock) at offsets: {:?}", ivmixes);

    // For every anchor, scan the surrounding ±64KB for 16-byte-aligned
    // windows that look like key tables.
    let mut anchors: Vec<usize> = Vec::new();
    anchors.extend_from_slice(&sboxes);
    anchors.extend_from_slice(&ivmixes);
    if anchors.is_empty() {
        println!("no stock anchors found — cannot narrow the search");
        return;
    }
    for &a in &anchors {
        let lo = a.saturating_sub(0x10000);
        let hi = (a + 0x10000).min(data.len());
        let mut found = 0;
        for off in (lo..hi).step_by(16) {
            if off + 256 > data.len() {
                break;
            }
            let win = &data[off..off + 256];
            if looks_like_key_table(win) {
                found += 1;
                if found <= 20 {
                    println!(
                        "candidate key table @ 0x{off:X} (near anchor 0x{a:X}): {}",
                        hex32(win)
                    );
                }
            }
        }
        if found > 20 {
            println!("  ... and {} more candidates near 0x{a:X}", found - 20);
        }
        if found == 0 {
            println!("no key-like tables within ±64KB of anchor 0x{a:X}");
        }
    }
}
