//! Packet cursor reader/writer (little-endian field reads, length-prefixed
//! strings, GBK text).

/// A cursor over received bytes (little-endian field reads). Reads beyond the
/// buffer set the `failed` flag instead of panicking, so one malformed packet
/// cannot crash the bot (the handler checks `failed()` and skips the packet).
pub struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    failed: bool,
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            failed: false,
        }
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    pub fn available(&self) -> bool {
        !self.failed && self.remaining() > 0
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// Skip `count` bytes. On underflow the cursor is marked failed and pinned
    /// to the end (mirrors the C++ PacketError underflow check without panicking).
    pub fn skip(&mut self, count: usize) {
        if self.failed {
            return;
        }
        if count > self.remaining() {
            self.failed = true;
            self.pos = self.bytes.len();
            return;
        }
        self.pos += count;
    }

    /// Returns true (and marks the cursor failed) when out of bytes.
    fn oob(&mut self) -> bool {
        if self.failed {
            return true;
        }
        if self.pos >= self.bytes.len() {
            self.failed = true;
            return true;
        }
        false
    }

    pub fn read_u8(&mut self) -> u8 {
        if self.oob() {
            return 0;
        }
        let v = self.bytes[self.pos];
        self.pos += 1;
        v
    }

    pub fn read_bool(&mut self) -> bool {
        self.read_u8() == 1
    }

    pub fn read_i8(&mut self) -> i8 {
        self.read_u8() as i8
    }

    pub fn read_u16(&mut self) -> u16 {
        if self.failed {
            return 0;
        }
        let mut v = 0u16;
        for i in 0..2 {
            if self.oob() {
                return 0;
            }
            v |= (self.bytes[self.pos] as u16) << (8 * i);
            self.pos += 1;
        }
        v
    }

    pub fn read_i16(&mut self) -> i16 {
        self.read_u16() as i16
    }

    pub fn read_u32(&mut self) -> u32 {
        if self.failed {
            return 0;
        }
        let mut v = 0u32;
        for i in 0..4 {
            if self.oob() {
                return 0;
            }
            v |= (self.bytes[self.pos] as u32) << (8 * i);
            self.pos += 1;
        }
        v
    }

    pub fn read_i32(&mut self) -> i32 {
        self.read_u32() as i32
    }

    pub fn read_i64(&mut self) -> i64 {
        if self.failed {
            return 0;
        }
        let mut v = 0i64;
        for i in 0..8 {
            if self.oob() {
                return 0;
            }
            v |= (self.bytes[self.pos] as i64) << (8 * i);
            self.pos += 1;
        }
        v
    }

    pub fn read_point(&mut self) -> (i16, i16) {
        let x = self.read_i16();
        let y = self.read_i16();
        (x, y)
    }

    /// Reads a length-prefixed string (u16 length, then that many bytes,
    /// NULs stripped — mirrors read_padded_string).
    pub fn read_string(&mut self) -> String {
        let len = self.read_u16() as usize;
        self.read_padded_string(len)
    }

    /// Reads exactly `len` bytes, dropping NUL bytes (matches the C++).
    pub fn read_padded_string(&mut self, len: usize) -> String {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            let b = self.read_u8();
            if b != 0 {
                out.push(b);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Reads a length-prefixed string (u16 length, then that many bytes) and
    /// decodes it. The server writes map strings with `MapleType.中国`'s ANSI
    /// charset = GBK (see writeMapleAsciiString), so raw GBK bytes decode via
    /// UTF-8 lossy when the message is ASCII-compatible and via GBK otherwise.
    pub fn read_string_gb(&mut self) -> String {
        let len = self.read_u16() as usize;
        self.read_padded_string_gb(len)
    }

    /// Same as `read_string_gb` but for a fixed byte length.
    pub fn read_padded_string_gb(&mut self, len: usize) -> String {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            let b = self.read_u8();
            if b != 0 {
                out.push(b);
            }
        }
        if out.is_empty() {
            return String::new();
        }
        match std::str::from_utf8(&out) {
            Ok(s) => s.to_string(),
            Err(_) => {
                let (decoded, _, _) = encoding_rs::GBK.decode(&out);
                decoded.into_owned()
            }
        }
    }

    // ---- skip helpers ----
    pub fn skip_byte(&mut self) {
        self.skip(1);
    }
    pub fn skip_short(&mut self) {
        self.skip(2);
    }
    pub fn skip_int(&mut self) {
        self.skip(4);
    }
    pub fn skip_long(&mut self) {
        self.skip(8);
    }
    pub fn skip_point(&mut self) {
        self.skip(4);
    }
    pub fn skip_string(&mut self) {
        let len = self.read_u16() as usize;
        self.skip(len);
    }
}

/// Process start time baseline (used by write_time; must be initialized once at startup).
static TIME_BASELINE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Call at process start to pin the write_time baseline (both main entries do).
/// If never called, write_time falls back to the first call time (still
/// monotonic and non-negative).
pub fn init_time_baseline() {
    let _ = TIME_BASELINE.get_or_init(std::time::Instant::now);
}

/// Milliseconds since process start (monotonic; i32 cannot overflow within 24.8 days).
fn time_elapsed_ms() -> i32 {
    let base = *TIME_BASELINE.get_or_init(std::time::Instant::now);
    base.elapsed().as_millis() as i32
}

/// A little-endian packet builder. The opcode (u16) is written first,
/// matching `OutPacket::OutPacket`.
pub struct Packet {
    bytes: Vec<u8>,
}

impl Packet {
    pub fn new(opcode: u16) -> Self {
        let mut bytes = Vec::with_capacity(64);
        bytes.push((opcode & 0xFF) as u8);
        bytes.push((opcode >> 8) as u8);
        Self { bytes }
    }

    pub fn skip(&mut self, count: usize) {
        self.bytes.resize(self.bytes.len() + count, 0);
    }

    pub fn write_u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    pub fn write_i8(&mut self, v: i8) {
        self.bytes.push(v as u8);
    }

    pub fn write_u16(&mut self, v: u16) {
        self.bytes.push((v & 0xFF) as u8);
        self.bytes.push((v >> 8) as u8);
    }

    pub fn write_i16(&mut self, v: i16) {
        self.write_u16(v as u16);
    }

    pub fn write_u32(&mut self, v: u32) {
        for i in 0..4 {
            self.bytes.push(((v >> (8 * i)) & 0xFF) as u8);
        }
    }

    pub fn write_i32(&mut self, v: i32) {
        self.write_u32(v as u32);
    }

    pub fn write_i64(&mut self, v: i64) {
        for i in 0..8 {
            self.bytes.push(((v >> (8 * i)) & 0xFF) as u8);
        }
    }

    pub fn write_point(&mut self, x: i16, y: i16) {
        self.write_i16(x);
        self.write_i16(y);
    }

    /// Length-prefixed string (u16 length then raw UTF-8 bytes).
    pub fn write_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.write_u16(bytes.len() as u16);
        self.bytes.extend_from_slice(bytes);
    }

    /// Length-prefixed string encoded with GBK (the charset the server's
    /// `MapleType.中国` writes and reads map strings with). ASCII bytes are
    /// passed through untouched.
    pub fn write_string_gb(&mut self, s: &str) {
        if s.is_ascii() {
            self.write_string(s);
            return;
        }
        let (encoded, _, _) = encoding_rs::GBK.encode(s);
        let bytes: &[u8] = &encoded;
        self.write_u16(bytes.len() as u16);
        self.bytes.extend_from_slice(bytes);
    }

    /// Raw bytes appended verbatim.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Current time in ms (monotonic since process start).
    ///
    /// Attack packet tick = ms since process start. The old implementation
    /// lazy-initialized the baseline with LazyLock<Instant>, so the **first**
    /// write_time call in a process was always 0 — an attack packet with
    /// tick=0 could be rejected by the server. The baseline must be
    /// initialized once at startup (`init_time_baseline`, called by both main entries).
    pub fn write_time(&mut self) {
        let ms = time_elapsed_ms();
        self.write_i32(ms);
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_reads_le_fields() {
        let data = [
            0x34u8, 0x12, 0x78, 0x56, 0x34, 0x12, // u16=0x1234, i32=0x12345678
            0x03, 0x00, b'A', b'B', b'C', // string "ABC" (len 3)
        ];
        let mut c = Cursor::new(&data);
        assert_eq!(c.read_u16(), 0x1234);
        assert_eq!(c.read_i32(), 0x12345678);
        assert_eq!(c.read_string(), "ABC");
        assert!(!c.available());
    }

    #[test]
    fn cursor_strips_nuls() {
        let data = [0x04u8, 0x00, b'a', 0x00, b'b', b'c'];
        let mut c = Cursor::new(&data);
        assert_eq!(c.read_string(), "abc");
    }

    #[test]
    fn packet_writes_le_fields() {
        let mut p = Packet::new(0x0102);
        p.write_u16(0xABCD);
        p.write_i32(-2);
        p.write_string("hi");

        let b = p.into_bytes();
        assert_eq!(&b[0..2], &[0x02, 0x01]); // opcode LE
        assert_eq!(&b[2..4], &[0xCD, 0xAB]);
        assert_eq!(&b[4..8], &[0xFE, 0xFF, 0xFF, 0xFF]); // -2 LE
        assert_eq!(&b[8..12], &[2, 0, b'h', b'i']); // "hi"
    }

    #[test]
    fn read_string_after_write_roundtrip() {
        let mut p = Packet::new(0);
        p.write_string("你好, world");
        let bytes = p.into_bytes();
        let mut c = Cursor::new(&bytes[2..]); // skip opcode
        assert_eq!(c.read_string(), "你好, world");
    }

    #[test]
    fn gbk_string_from_third_party_capture() {
        // NPC_TALK text: "		#fEffect/...#e#r 本地服079-V..."
        // The Chinese part is GBK bytes (B1 BE B5 D8 B7 FE); read_string_gb should decode "本地服".
        let bytes: &[u8] = &[
            0x09, 0x09, 0x23, 0x66, 0x45, 0x66, 0x66, 0x65, 0x63, 0x74, 0x2F, 0x49,
            0x74, 0x65, 0x6D, 0x45, 0x66, 0x66, 0x2F, 0x31, 0x30, 0x37, 0x31, 0x30,
            0x38, 0x35, 0x2F, 0x65, 0x66, 0x66, 0x65, 0x63, 0x74, 0x2F, 0x77, 0x61,
            0x6C, 0x6B, 0x31, 0x2F, 0x32, 0x23, 0x20, 0x20, 0x23, 0x65, 0x23, 0x72,
            0x20, 0xB1, 0xBE, 0xB5, 0xD8, 0xB7, 0xFE, 0x30, 0x37, 0x39, 0x2D, 0x56,
        ];
        let mut c = Cursor::new(bytes);
        let s = c.read_padded_string_gb(bytes.len());
        assert!(s.contains("本地服"), "got: {s}");
        assert!(s.contains("079-V"));
    }
}
