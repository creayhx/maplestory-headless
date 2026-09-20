//! MapleStory transport cryptography.
//!
//! AES over a pre-expanded round-key table, plus the Shanda byte transform and
//! the rolling IV update; only valid for MapleStory versions below 118 (the
//! scheme was replaced upstream after that). The byte layout and the per-packet
//! behaviour were confirmed against live captures from the target server.
//!
//! The default key is the well-known v79 key first published by the HeavenMS
//! open-source server (kept by most private servers); servers that replaced it
//! can inject a custom key via `--aes-key <hex16>` (8-byte key, expanded at
//! runtime by `expand_key`) or a pre-expanded 256-byte table (`--aes-key
//! <hex512>`).

/// Length of the 4-byte packet header.
pub const HEADER_LENGTH: usize = 4;

/// Length of the server handshake received on connect.
pub const HANDSHAKE_LEN: usize = 16;

/// MapleStory version used in the packet header XOR.
/// GMS v83 = 83, CMS079 = 79.
pub const MAPLEVERSION: u16 = 79;

/// The standard pre-expanded 256-byte AES round-key table (HeavenMS standard).
/// Servers that replaced the key use a different table; expose the standard
/// one so tooling can compare / verify candidates.
pub fn standard_key() -> [u8; 256] {
    MAPLEKEY
}

/// Round constants for AES-256 key expansion (RCON[i], 1-based).
const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36];

/// Expand an 8-byte MapleStory AES key into the 256-byte pre-expanded
/// round-key table used by `Cryptography` (each key byte zero-extended to a
/// 4-byte little-endian word, standard AES-256 column-major expansion, 240
/// expanded bytes + 16 trailing zeros).
///
/// Verified 1:1 against the mp-reg reference implementation and a runtime
/// dump from a modified-key client: expanding the HeavenMS standard key
/// reproduces `MAPLEKEY`, and expanding a modified variant key
/// (`13 0A 06 B4 1B 0F 33 52`) reproduces the table asserted below.
pub fn expand_key(key8: [u8; 8]) -> [u8; 256] {
    // Word i holds key byte i zero-extended to 4 LE bytes: [k, 0, 0, 0].
    let mut w = [0u32; 60];
    for i in 0..8 {
        w[i] = key8[i] as u32;
    }
    for i in 8..60 {
        let mut temp = w[i - 1];
        if i % 8 == 0 {
            // RotWord: bytes [b0,b1,b2,b3] -> [b1,b2,b3,b0] (LE rotate).
            temp = temp.rotate_right(8);
            temp = sub_word(temp);
            // XOR the round constant into the first (least-significant) byte.
            temp ^= RCON[i / 8 - 1] as u32;
        } else if i % 8 == 4 {
            temp = sub_word(temp);
        }
        w[i] = w[i - 8] ^ temp;
    }
    let mut key = [0u8; 256];
    for i in 0..60 {
        key[i * 4] = (w[i] & 0xFF) as u8;
        key[i * 4 + 1] = ((w[i] >> 8) & 0xFF) as u8;
        key[i * 4 + 2] = ((w[i] >> 16) & 0xFF) as u8;
        key[i * 4 + 3] = ((w[i] >> 24) & 0xFF) as u8;
    }
    key
}

/// Apply the Rijndael substitution box to each byte of a word.
fn sub_word(w: u32) -> u32 {
    let b0 = SUBBOX[(w & 0xFF) as usize] as u32;
    let b1 = SUBBOX[((w >> 8) & 0xFF) as usize] as u32;
    let b2 = SUBBOX[((w >> 16) & 0xFF) as usize] as u32;
    let b3 = SUBBOX[((w >> 24) & 0xFF) as usize] as u32;
    b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
}

pub struct Cryptography {
    sendiv: [u8; 4],
    recviv: [u8; 4],
    version: u16,
    /// AES round-key table. Custom servers may replace the standard table;
    /// inject it via `--aes-key <hex16>` (8-byte key, expanded by
    /// `expand_key`) or `--aes-key <hex512>` (pre-expanded table).
    key: [u8; 256],
}

impl Cryptography {
    /// Build from the 16-byte server handshake.
    /// Layout: [0..2] version, [2] locale, [3..7] zero, [7..11] send IV, [11..15] recv IV, [15] zero.
    pub fn from_handshake(handshake: &[u8]) -> Self {
        Self::with_version(handshake, MAPLEVERSION)
    }

    /// Build from the handshake with an explicit MapleStory version for the
    /// header XOR (83 for GMS, 79 for CMS079).
    pub fn with_version(handshake: &[u8], version: u16) -> Self {
        let mut sendiv = [0u8; 4];
        let mut recviv = [0u8; 4];
        for i in 0..4 {
            sendiv[i] = handshake[i + 7];
            recviv[i] = handshake[i + 11];
        }
        Self::with_ivs(sendiv, recviv, version)
    }

    /// Build from explicit IVs (used by the fake-server test for server-perspective crypto).
    pub fn with_ivs(sendiv: [u8; 4], recviv: [u8; 4], version: u16) -> Self {
        Self {
            sendiv,
            recviv,
            version,
            key: MAPLEKEY,
        }
    }

    /// Build from explicit IVs AND a custom pre-expanded 256-byte AES round-key
    /// table (for servers that replaced the standard MapleStory key).
    pub fn with_ivs_keyed(sendiv: [u8; 4], recviv: [u8; 4], version: u16, key: [u8; 256]) -> Self {
        Self {
            sendiv,
            recviv,
            version,
            key,
        }
    }

    /// Replace the AES round-key table (after building from a handshake).
    pub fn set_key(&mut self, key: [u8; 256]) {
        self.key = key;
    }

    /// Encrypt an outgoing packet body.
    pub fn encrypt(&mut self, bytes: &mut [u8]) {
        mapleencrypt(bytes);
        aesofb(bytes, &mut self.sendiv, &self.key);
    }

    /// Decrypt an incoming packet body.
    pub fn decrypt(&mut self, bytes: &mut [u8]) {
        aesofb(bytes, &mut self.recviv, &self.key);
        mapledecrypt(bytes);
    }

    /// Build the 4-byte header for a packet of the given length.
    pub fn create_header(&self, length: usize) -> [u8; 4] {
        let a = (((self.sendiv[3] as usize) << 8) | self.sendiv[2] as usize) ^ self.version as usize;
        let b = a ^ length;
        let mut header = [0u8; 4];
        header[0] = (a % 0x100) as u8;
        header[1] = (a / 0x100) as u8;
        header[2] = (b % 0x100) as u8;
        header[3] = (b / 0x100) as u8;
        header
    }

    /// Recover the payload length from a 4-byte header.
    pub fn check_length(bytes: &[u8]) -> usize {
        let mut mask: u32 = 0;
        for i in 0..4 {
            mask |= (bytes[i] as u32) << (8 * i);
        }
        ((mask >> 16) ^ (mask & 0xFFFF)) as usize
    }
}

/// AES-OFB-style stream cipher over the packet, updating the IV.
fn aesofb(bytes: &mut [u8], iv: &mut [u8; 4], key: &[u8; 256]) {
    let length = bytes.len();
    let mut blocklength: usize = 0x5B0;
    let mut offset: usize = 0;

    while offset < length {
        let mut miv = [0u8; 16];
        for i in 0..16 {
            miv[i] = iv[i % 4];
        }

        let mut remaining = length - offset;
        if remaining > blocklength {
            remaining = blocklength;
        }

        for x in 0..remaining {
            let relpos = x % 16;
            if relpos == 0 {
                aesencrypt(&mut miv, key);
            }
            bytes[x + offset] ^= miv[relpos];
        }

        offset += blocklength;
        blocklength = 0x5B4;
    }

    updateiv(iv);
}

// ---- MapleStory bit-shuffle ("Shanda") ------------------------------------

fn rollleft(data: i8, count: usize) -> i8 {
    let mask = ((data as u8) as i32) << (count % 8);
    let m = (mask & 0xFF) | (mask >> 8);
    (m & 0xFF) as u8 as i8
}

fn rollright(data: i8, count: usize) -> i8 {
    let mask = (((data as u8) as i32) << 8) >> (count % 8);
    let m = (mask & 0xFF) | (mask >> 8);
    (m & 0xFF) as u8 as i8
}

fn mapleencrypt(bytes: &mut [u8]) {
    let length = bytes.len();

    for _ in 0..3 {
        let mut remember: i8 = 0;
        let mut datalen: i8 = (length & 0xFF) as i8;

        for i in 0..length {
            let cur: i8 =
                ((rollleft(bytes[i] as i8, 3) as i32 + datalen as i32) ^ remember as i32) as u8
                    as i8;
            remember = cur;
            let cur2 = rollright(cur, (datalen as i32) as usize & 0xFF);
            bytes[i] = (!(cur2 as u8)).wrapping_add(0x48);
            datalen = datalen.wrapping_sub(1);
        }

        remember = 0;
        datalen = (length & 0xFF) as i8;

        let mut i = length;
        while i > 0 {
            i -= 1;
            let cur: i8 =
                ((rollleft(bytes[i] as i8, 4) as i32 + datalen as i32) ^ remember as i32) as u8
                    as i8;
            remember = cur;
            let t = ((cur as i32) ^ 0x13) & 0xFF;
            bytes[i] = rollright(t as u8 as i8, 3) as u8;
            datalen = datalen.wrapping_sub(1);
        }
    }
}

fn mapledecrypt(bytes: &mut [u8]) {
    let length = bytes.len();

    for _ in 0..3 {
        let mut remember: u8 = 0;
        let mut datalen: u8 = (length & 0xFF) as u8;

        let mut j = length;
        while j > 0 {
            j -= 1;
            let cur: u8 = (((rollleft(bytes[j] as i8, 3) as i32) ^ 0x13) & 0xFF) as u8;
            let t = ((cur as i32) ^ (remember as i32)) - (datalen as i32);
            bytes[j] = rollright((t & 0xFF) as u8 as i8, 4) as u8;
            remember = cur;
            datalen = datalen.wrapping_sub(1);
        }

        remember = 0;
        datalen = (length & 0xFF) as u8;

        for j in 0..length {
            let mut cur: u8 = !(bytes[j].wrapping_sub(0x48));
            cur = rollleft(cur as i8, datalen as usize) as u8;
            let t = ((cur as i32) ^ (remember as i32)) - (datalen as i32);
            bytes[j] = rollright((t & 0xFF) as u8 as i8, 3) as u8;
            remember = cur;
            datalen = datalen.wrapping_sub(1);
        }
    }
}

// ---- IV update -------------------------------------------------------------

fn updateiv(iv: &mut [u8; 4]) {
    let mut mbytes = [0xF2u8, 0x53, 0x50, 0xC6];

    for i in 0..4 {
        let ivbyte = iv[i];

        mbytes[0] = mbytes[0].wrapping_add(MAPLEBYTES[mbytes[1] as usize].wrapping_sub(ivbyte));
        mbytes[1] = mbytes[1].wrapping_sub(mbytes[2] ^ MAPLEBYTES[ivbyte as usize]);
        mbytes[2] = mbytes[2] ^ MAPLEBYTES[mbytes[3] as usize].wrapping_add(ivbyte);
        mbytes[3] = mbytes[3].wrapping_add(MAPLEBYTES[ivbyte as usize].wrapping_sub(mbytes[0]));

        let mut mask: u64 = 0;
        mask |= (mbytes[0] as u64) & 0xFF;
        mask |= ((mbytes[1] as u64) << 8) & 0xFF00;
        mask |= ((mbytes[2] as u64) << 16) & 0xFF0000;
        mask |= ((mbytes[3] as u64) << 24) & 0xFF000000;
        mask = (mask >> 0x1D) | (mask << 3);

        for j in 0..4 {
            let value = mask >> (8 * j);
            mbytes[j] = (value & 0xFF) as u8;
        }
    }

    for i in 0..4 {
        iv[i] = mbytes[i];
    }
}

// ---- AES variant -----------------------------------------------------------

fn aesencrypt(bytes: &mut [u8; 16], key: &[u8; 256]) {
    let mut round: usize = 0;
    addroundkey(bytes, round, key);

    round = 1;
    while round < 14 {
        subbytes(bytes);
        shiftrows(bytes);
        mixcolumns(bytes);
        addroundkey(bytes, round, key);
        round += 1;
    }

    subbytes(bytes);
    shiftrows(bytes);
    addroundkey(bytes, round, key);
}

fn addroundkey(bytes: &mut [u8; 16], round: usize, key: &[u8; 256]) {
    let offset = round * 16;
    for i in 0..16 {
        bytes[i] ^= key[i + offset];
    }
}

fn subbytes(bytes: &mut [u8; 16]) {
    for i in 0..16 {
        bytes[i] = SUBBOX[bytes[i] as usize];
    }
}

fn shiftrows(bytes: &mut [u8; 16]) {
    let mut remember = bytes[1];
    bytes[1] = bytes[5];
    bytes[5] = bytes[9];
    bytes[9] = bytes[13];
    bytes[13] = remember;

    remember = bytes[10];
    bytes[10] = bytes[2];
    bytes[2] = remember;

    remember = bytes[3];
    bytes[3] = bytes[15];
    bytes[15] = bytes[11];
    bytes[11] = bytes[7];
    bytes[7] = remember;

    remember = bytes[14];
    bytes[14] = bytes[6];
    bytes[6] = remember;
}

fn gmul(x: u8) -> u8 {
    (x << 1) ^ (0x1B & ((x as i8) >> 7) as u8)
}

fn mixcolumns(bytes: &mut [u8; 16]) {
    for i in (0..16).step_by(4) {
        let cpy0 = bytes[i];
        let cpy1 = bytes[i + 1];
        let cpy2 = bytes[i + 2];
        let cpy3 = bytes[i + 3];

        let mul0 = gmul(bytes[i]);
        let mul1 = gmul(bytes[i + 1]);
        let mul2 = gmul(bytes[i + 2]);
        let mul3 = gmul(bytes[i + 3]);

        bytes[i] = mul0 ^ cpy3 ^ cpy2 ^ mul1 ^ cpy1;
        bytes[i + 1] = mul1 ^ cpy0 ^ cpy3 ^ mul2 ^ cpy2;
        bytes[i + 2] = mul2 ^ cpy1 ^ cpy0 ^ mul3 ^ cpy3;
        bytes[i + 3] = mul3 ^ cpy2 ^ cpy1 ^ mul0 ^ cpy0;
    }
}

// ---- Tables ----------------------------------------------------------------

/// Pre-expanded AES round key.
/// Runtime-verified from the private server's client (MapleStory.exe): the
/// AES_EncKeySchedule path expands the STOCK HeavenMS key
/// (13 08 06 B4 1B 0F 33 52 — the default defined by the first open-source
/// server, kept by most private servers), NOT the custom UserKey_0 at
/// 0xBA05B0 (13 52 2A 5B 08 02 10 60) which is unused by the transport cipher.
const MAPLEKEY: [u8; 256] = [
    0x13, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0xB4, 0x00, 0x00, 0x00,
    0x1B, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x33, 0x00, 0x00, 0x00, 0x52, 0x00, 0x00, 0x00,
    0x71, 0x63, 0x63, 0x00, 0x79, 0x63, 0x63, 0x00, 0x7F, 0x63, 0x63, 0x00, 0xCB, 0x63, 0x63, 0x00,
    0x04, 0xFB, 0xFB, 0x63, 0x0B, 0xFB, 0xFB, 0x63, 0x38, 0xFB, 0xFB, 0x63, 0x6A, 0xFB, 0xFB, 0x63,
    0x7C, 0x6C, 0x98, 0x02, 0x05, 0x0F, 0xFB, 0x02, 0x7A, 0x6C, 0x98, 0x02, 0xB1, 0x0F, 0xFB, 0x02,
    0xCC, 0x8D, 0xF4, 0x14, 0xC7, 0x76, 0x0F, 0x77, 0xFF, 0x8D, 0xF4, 0x14, 0x95, 0x76, 0x0F, 0x77,
    0x40, 0x1A, 0x6D, 0x28, 0x45, 0x15, 0x96, 0x2A, 0x3F, 0x79, 0x0E, 0x28, 0x8E, 0x76, 0xF5, 0x2A,
    0xD5, 0xB5, 0x12, 0xF1, 0x12, 0xC3, 0x1D, 0x86, 0xED, 0x4E, 0xE9, 0x92, 0x78, 0x38, 0xE6, 0xE5,
    0x4F, 0x94, 0xB4, 0x94, 0x0A, 0x81, 0x22, 0xBE, 0x35, 0xF8, 0x2C, 0x96, 0xBB, 0x8E, 0xD9, 0xBC,
    0x3F, 0xAC, 0x27, 0x94, 0x2D, 0x6F, 0x3A, 0x12, 0xC0, 0x21, 0xD3, 0x80, 0xB8, 0x19, 0x35, 0x65,
    0x8B, 0x02, 0xF9, 0xF8, 0x81, 0x83, 0xDB, 0x46, 0xB4, 0x7B, 0xF7, 0xD0, 0x0F, 0xF5, 0x2E, 0x6C,
    0x49, 0x4A, 0x16, 0xC4, 0x64, 0x25, 0x2C, 0xD6, 0xA4, 0x04, 0xFF, 0x56, 0x1C, 0x1D, 0xCA, 0x33,
    0x0F, 0x76, 0x3A, 0x64, 0x8E, 0xF5, 0xE1, 0x22, 0x3A, 0x8E, 0x16, 0xF2, 0x35, 0x7B, 0x38, 0x9E,
    0xDF, 0x6B, 0x11, 0xCF, 0xBB, 0x4E, 0x3D, 0x19, 0x1F, 0x4A, 0xC2, 0x4F, 0x03, 0x57, 0x08, 0x7C,
    0x14, 0x46, 0x2A, 0x1F, 0x9A, 0xB3, 0xCB, 0x3D, 0xA0, 0x3D, 0xDD, 0xCF, 0x95, 0x46, 0xE5, 0x51,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Rijndael substitution box.
const SUBBOX: [u8; 256] = [
    0x63, 0x7C, 0x77, 0x7B, 0xF2, 0x6B, 0x6F, 0xC5, 0x30, 0x01, 0x67, 0x2B, 0xFE, 0xD7, 0xAB, 0x76,
    0xCA, 0x82, 0xC9, 0x7D, 0xFA, 0x59, 0x47, 0xF0, 0xAD, 0xD4, 0xA2, 0xAF, 0x9C, 0xA4, 0x72, 0xC0,
    0xB7, 0xFD, 0x93, 0x26, 0x36, 0x3F, 0xF7, 0xCC, 0x34, 0xA5, 0xE5, 0xF1, 0x71, 0xD8, 0x31, 0x15,
    0x04, 0xC7, 0x23, 0xC3, 0x18, 0x96, 0x05, 0x9A, 0x07, 0x12, 0x80, 0xE2, 0xEB, 0x27, 0xB2, 0x75,
    0x09, 0x83, 0x2C, 0x1A, 0x1B, 0x6E, 0x5A, 0xA0, 0x52, 0x3B, 0xD6, 0xB3, 0x29, 0xE3, 0x2F, 0x84,
    0x53, 0xD1, 0x00, 0xED, 0x20, 0xFC, 0xB1, 0x5B, 0x6A, 0xCB, 0xBE, 0x39, 0x4A, 0x4C, 0x58, 0xCF,
    0xD0, 0xEF, 0xAA, 0xFB, 0x43, 0x4D, 0x33, 0x85, 0x45, 0xF9, 0x02, 0x7F, 0x50, 0x3C, 0x9F, 0xA8,
    0x51, 0xA3, 0x40, 0x8F, 0x92, 0x9D, 0x38, 0xF5, 0xBC, 0xB6, 0xDA, 0x21, 0x10, 0xFF, 0xF3, 0xD2,
    0xCD, 0x0C, 0x13, 0xEC, 0x5F, 0x97, 0x44, 0x17, 0xC4, 0xA7, 0x7E, 0x3D, 0x64, 0x5D, 0x19, 0x73,
    0x60, 0x81, 0x4F, 0xDC, 0x22, 0x2A, 0x90, 0x88, 0x46, 0xEE, 0xB8, 0x14, 0xDE, 0x5E, 0x0B, 0xDB,
    0xE0, 0x32, 0x3A, 0x0A, 0x49, 0x06, 0x24, 0x5C, 0xC2, 0xD3, 0xAC, 0x62, 0x91, 0x95, 0xE4, 0x79,
    0xE7, 0xC8, 0x37, 0x6D, 0x8D, 0xD5, 0x4E, 0xA9, 0x6C, 0x56, 0xF4, 0xEA, 0x65, 0x7A, 0xAE, 0x08,
    0xBA, 0x78, 0x25, 0x2E, 0x1C, 0xA6, 0xB4, 0xC6, 0xE8, 0xDD, 0x74, 0x1F, 0x4B, 0xBD, 0x8B, 0x8A,
    0x70, 0x3E, 0xB5, 0x66, 0x48, 0x03, 0xF6, 0x0E, 0x61, 0x35, 0x57, 0xB9, 0x86, 0xC1, 0x1D, 0x9E,
    0xE1, 0xF8, 0x98, 0x11, 0x69, 0xD9, 0x8E, 0x94, 0x9B, 0x1E, 0x87, 0xE9, 0xCE, 0x55, 0x28, 0xDF,
    0x8C, 0xA1, 0x89, 0x0D, 0xBF, 0xE6, 0x42, 0x68, 0x41, 0x99, 0x2D, 0x0F, 0xB0, 0x54, 0xBB, 0x16,
];

/// IV mixing table.
const MAPLEBYTES: [u8; 256] = [
    0xEC, 0x3F, 0x77, 0xA4, 0x45, 0xD0, 0x71, 0xBF, 0xB7, 0x98, 0x20, 0xFC, 0x4B, 0xE9, 0xB3, 0xE1,
    0x5C, 0x22, 0xF7, 0x0C, 0x44, 0x1B, 0x81, 0xBD, 0x63, 0x8D, 0xD4, 0xC3, 0xF2, 0x10, 0x19, 0xE0,
    0xFB, 0xA1, 0x6E, 0x66, 0xEA, 0xAE, 0xD6, 0xCE, 0x06, 0x18, 0x4E, 0xEB, 0x78, 0x95, 0xDB, 0xBA,
    0xB6, 0x42, 0x7A, 0x2A, 0x83, 0x0B, 0x54, 0x67, 0x6D, 0xE8, 0x65, 0xE7, 0x2F, 0x07, 0xF3, 0xAA,
    0x27, 0x7B, 0x85, 0xB0, 0x26, 0xFD, 0x8B, 0xA9, 0xFA, 0xBE, 0xA8, 0xD7, 0xCB, 0xCC, 0x92, 0xDA,
    0xF9, 0x93, 0x60, 0x2D, 0xDD, 0xD2, 0xA2, 0x9B, 0x39, 0x5F, 0x82, 0x21, 0x4C, 0x69, 0xF8, 0x31,
    0x87, 0xEE, 0x8E, 0xAD, 0x8C, 0x6A, 0xBC, 0xB5, 0x6B, 0x59, 0x13, 0xF1, 0x04, 0x00, 0xF6, 0x5A,
    0x35, 0x79, 0x48, 0x8F, 0x15, 0xCD, 0x97, 0x57, 0x12, 0x3E, 0x37, 0xFF, 0x9D, 0x4F, 0x51, 0xF5,
    0xA3, 0x70, 0xBB, 0x14, 0x75, 0xC2, 0xB8, 0x72, 0xC0, 0xED, 0x7D, 0x68, 0xC9, 0x2E, 0x0D, 0x62,
    0x46, 0x17, 0x11, 0x4D, 0x6C, 0xC4, 0x7E, 0x53, 0xC1, 0x25, 0xC7, 0x9A, 0x1C, 0x88, 0x58, 0x2C,
    0x89, 0xDC, 0x02, 0x64, 0x40, 0x01, 0x5D, 0x38, 0xA5, 0xE2, 0xAF, 0x55, 0xD5, 0xEF, 0x1A, 0x7C,
    0xA7, 0x5B, 0xA6, 0x6F, 0x86, 0x9F, 0x73, 0xE6, 0x0A, 0xDE, 0x2B, 0x99, 0x4A, 0x47, 0x9C, 0xDF,
    0x09, 0x76, 0x9E, 0x30, 0x0E, 0xE4, 0xB2, 0x94, 0xA0, 0x3B, 0x34, 0x1D, 0x28, 0x0F, 0x36, 0xE3,
    0x23, 0xB4, 0x03, 0xD8, 0x90, 0xC8, 0x3C, 0xFE, 0x5E, 0x32, 0x24, 0x50, 0x1F, 0x3A, 0x43, 0x8A,
    0x96, 0x41, 0x74, 0xAC, 0x52, 0x33, 0xF0, 0xD9, 0x29, 0x80, 0xB1, 0x16, 0xD3, 0xAB, 0x91, 0xB9,
    0x84, 0x7F, 0x61, 0x1E, 0xCF, 0xC5, 0xD1, 0x56, 0x3D, 0xCA, 0xF4, 0x05, 0xC6, 0xE5, 0x08, 0x49,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shanda_roundtrip() {
        // mapleencrypt / mapledecrypt are exact inverses for any data.
        for len in [1usize, 2, 6, 16, 64, 0x5B0, 0x5B4, 200] {
            let original: Vec<u8> = (0u8..len as u8).cycle().take(len).collect();
            let mut payload = original.clone();
            mapleencrypt(&mut payload);
            mapledecrypt(&mut payload);
            assert_eq!(payload, original, "len={len}");
        }
    }

    #[test]
    fn aesofb_keystream_inverse() {
        // aesofb is a pure XOR keystream: applying it twice from the same IV
        // state restores the data (and updateiv must be deterministic).
        let original: Vec<u8> = (0u8..250).chain(0u8..250).chain(0u8..250).chain(0u8..250).collect();

        let mut buf = original.clone();
        let mut iv1 = [0xA1u8, 0xB2, 0xC3, 0xD4];
        aesofb(&mut buf, &mut iv1, &MAPLEKEY);

        let mut iv2 = [0xA1u8, 0xB2, 0xC3, 0xD4];
        aesofb(&mut buf, &mut iv2, &MAPLEKEY);

        assert_eq!(buf, original);
    }

    #[test]
    fn full_client_server_roundtrip() {
        // Client encrypts with its send IV; the server decrypts with that same
        // IV stream. Mirror that by feeding both directions through sendiv.
        let handshake = [0x4Fu8, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x1A, 0x2B, 0x3C, 0x4D, 0x5E, 0x6F, 0x80, 0x91, 0x00];
        let mut client = Cryptography::from_handshake(&handshake);
        let mut server = Cryptography::from_handshake(&handshake);
        let original: Vec<u8> = (0u8..250).collect();

        let mut payload = original.clone();
        client.encrypt(&mut payload);

        // Server decrypts with the same IV the client used to encrypt (sendiv).
        aesofb(&mut payload, &mut server.sendiv, &MAPLEKEY);
        mapledecrypt(&mut payload);

        assert_eq!(payload, original);
    }

    #[test]
    fn header_roundtrip() {
        let handshake = [0x4Fu8, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x00];
        let crypto = Cryptography::from_handshake(&handshake);

        for len in [0usize, 1, 2, 6, 16, 64, 0x5B0, 0x5B4, 2000] {
            let header = crypto.create_header(len);
            assert_eq!(Cryptography::check_length(&header), len, "len={len}");
        }
    }

    #[test]
    fn handshake_iv_extraction() {
        let handshake = [0u8; 16];
        let crypto = Cryptography::from_handshake(&handshake);
        assert_eq!(crypto.sendiv, [0; 4]);
        assert_eq!(crypto.recviv, [0; 4]);
    }

    #[test]
    fn custom_key_matches_standard_table() {
        // Injecting the standard table must reproduce default behavior exactly.
        let handshake = [0x4Fu8, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x1A, 0x2B, 0x3C, 0x4D, 0x5E, 0x6F, 0x80, 0x91, 0x00];
        let mut a = Cryptography::from_handshake(&handshake);
        let sendiv = a.sendiv;
        let mut b = Cryptography::with_ivs_keyed(sendiv, a.recviv, MAPLEVERSION, MAPLEKEY);
        let data: Vec<u8> = (0u8..250).collect();
        let mut x = data.clone();
        let mut y = data.clone();
        a.encrypt(&mut x);
        b.encrypt(&mut y);
        assert_eq!(x, y);
        // Server-side decrypt (same send-IV stream + mapledecrypt, mirroring
        // full_client_server_roundtrip) restores the data.
        let mut iv = sendiv;
        aesofb(&mut y, &mut iv, &MAPLEKEY);
        mapledecrypt(&mut y);
        assert_eq!(y, data);
    }

    #[test]
    fn expand_key_standard_matches_table() {
        // Expanding the HeavenMS standard key (13 08 06 B4 1B 0F 33 52) must
        // reproduce the compile-time MAPLEKEY table byte for byte.
        let key = expand_key([0x13, 0x08, 0x06, 0xB4, 0x1B, 0x0F, 0x33, 0x52]);
        assert_eq!(&key[..], &MAPLEKEY[..]);
    }

    #[test]
    fn expand_key_modified_variant_matches_reference_table() {
        // Modified-key client: key 13 0A 06 B4 1B 0F 33 52 (one byte differs
        // from the standard key). Reference table runtime-dumped from the
        // client, cross-checked against the mp-reg reference implementation:
        // 240 expanded bytes + 16 zero pad.
        let hex = concat!(
            "130000000A00000006000000B40000001B0000000F0000003300000052000000",
            "716363007B6363007D636300C9636300C6FBFB63C9FBFB63FAFBFB63A8FBFB63",
            "7C6C98C2070FFBC27A6C98C2B30FFBC2AB8DF44662760F25988DF44630760F25",
            "401AA7C647155C043D79C4C68E763F04B2B581B4D0C38E91484E7AD7783875F2",
            "4F872E7A0892727E35EBB6B8BB9D89BC58EB26D18828A840C066D297B85EA765",
            "07DB63160F4911683AA2A7D0813F2E6C549E1781DCB6BFC11CD06D56A48ECA33",
            "3EAFA05F31E6B1370B4416E78A7B388B2ABF10BCF609AF7DEAD9C22B4E570818",
            "259F0D701479BC471F3DAAA09546922B00000000000000000000000000000000",
        );
        let mut expect = [0u8; 256];
        for i in 0..256 {
            expect[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        let key = expand_key([0x13, 0x0A, 0x06, 0xB4, 0x1B, 0x0F, 0x33, 0x52]);
        assert_eq!(&key[..], &expect[..]);
    }
}

