//! TCP session + MapleStory packet framing.

use crate::packet::Packet;
use crate::crypto::{Cryptography, HANDSHAKE_LEN};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Sanity cap on a single packet size (C++ MAX_PACKET_LENGTH is 0x10000).
const MAX_PACKET_LENGTH: usize = 0x20000;

pub struct Session {
    stream: TcpStream,
    crypto: Cryptography,
    crypto_version: u16,
    /// Custom AES round-key table (modified-key servers); re-applied on every
    /// reconnect so channel-server connections keep the custom key. None =
    /// standard MapleStory key.
    key: Option<[u8; 256]>,
    inbox: Vec<u8>,
    pub show_packets: bool,
    pub dump_packets: bool,
    /// Chinese name dictionaries for THIS session's `data_dir`.
    ///
    /// Per-session state, deliberately not a process global: two sessions with
    /// different `data_dir` values must never see each other's names. Shared
    /// via `Arc` with other sessions on the same directory.
    pub names: std::sync::Arc<crate::names::NameTable>,
    /// Profile config chain for THIS session (`--config` files, last = write
    /// target). Empty = the legacy `./config.json`. Per-session for the same
    /// reason as `names`: `hunt save` / `reload` must only touch its own
    /// profile.
    pub config_paths: Vec<std::path::PathBuf>,
}

impl Session {
    /// Connect with an optional raw-hex dump of received packets.
    pub async fn connect_ex(
        addr: &str,
        version: u16,
        show_packets: bool,
        dump_packets: bool,
    ) -> Result<Self, String> {
        Self::connect_ex_keyed(addr, version, show_packets, dump_packets, None).await
    }

    /// Connect with an optional custom AES round-key table (modified-key
    /// servers; `None` = standard MapleStory v79 key).
    pub async fn connect_ex_keyed(
        addr: &str,
        version: u16,
        show_packets: bool,
        dump_packets: bool,
        key: Option<[u8; 256]>,
    ) -> Result<Self, String> {
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("connect {addr}: {e}"))?;

        let mut handshake = [0u8; HANDSHAKE_LEN];
        stream
            .read_exact(&mut handshake)
            .await
            .map_err(|e| format!("handshake read: {e}"))?;

        let crypto = match key {
            Some(key) => {
                let mut c = crate::crypto::Cryptography::with_version(&handshake, version);
                c.set_key(key);
                c
            }
            None => Cryptography::with_version(&handshake, version),
        };

        Ok(Self {
            stream,
            crypto,
            crypto_version: version,
            key,
            inbox: Vec::new(),
            show_packets,
            dump_packets,
            names: crate::names::NameTable::for_dir(""),
            config_paths: Vec::new(),
        })
    }

    /// Attach this session's name table and profile config chain.
    ///
    /// Builder-style so the three `connect*` constructors stay unchanged and
    /// existing call sites keep their exact behavior (default table, empty
    /// config chain → legacy `./config.json`).
    pub fn with_profile(
        mut self,
        names: std::sync::Arc<crate::names::NameTable>,
        config_paths: Vec<std::path::PathBuf>,
    ) -> Self {
        self.names = names;
        self.config_paths = config_paths;
        self
    }

    /// Active config chain: this session's files, or the legacy default.
    pub fn config_paths(&self) -> Vec<std::path::PathBuf> {
        if self.config_paths.is_empty() {
            vec![std::path::PathBuf::from(crate::runtime_config::CONFIG_PATH)]
        } else {
            self.config_paths.clone()
        }
    }

    /// Write-back target for this session: the last file of its chain.
    pub fn write_path(&self) -> std::path::PathBuf {
        self.config_paths
            .last()
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from(crate::runtime_config::CONFIG_PATH))
    }

    /// Drop the current connection and open a new one to a channel server.
    pub async fn reconnect(&mut self, addr: &str) -> Result<(), String> {
        self.inbox.clear();
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("reconnect {addr}: {e}"))?;

        let mut handshake = [0u8; HANDSHAKE_LEN];
        stream
            .read_exact(&mut handshake)
            .await
            .map_err(|e| format!("handshake read: {e}"))?;

        let mut crypto = Cryptography::with_version(&handshake, self.crypto_version);
        if let Some(key) = self.key {
            crypto.set_key(key);
        }
        self.stream = stream;
        self.crypto = crypto;
        Ok(())
    }

    /// Send a fully-built Packet (calls into_bytes internally).
    pub async fn send_packet(&mut self, packet: Packet) -> Result<(), String> {
        self.send(&packet.into_bytes()).await
    }

    /// Send a full packet (opcode + body, no header). Encrypts + adds header.
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        debug_assert!(bytes.len() >= 2, "packet must start with a 2-byte opcode");

        if self.show_packets {
            let opcode = u16::from_le_bytes([bytes[0], bytes[1]]);
            crate::emit_f!([] => "SEND [{opcode}] {} bytes", bytes.len());
            let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
            crate::emit_f!([] => "SENDHEX: {}", hex.join(""));
        }

        let mut body = bytes.to_vec();
        let header = self.crypto.create_header(body.len());
        self.crypto.encrypt(&mut body);

        if self.show_packets {
            let mut enc = header.to_vec();
            enc.extend_from_slice(&body);
            let hex: Vec<String> = enc.iter().map(|b| format!("{b:02X}")).collect();
            crate::emit_f!([] => "SENDWIRE: {}", hex.join(""));
        }

        self.stream
            .write_all(&header)
            .await
            .map_err(|e| format!("send header: {e}"))?;
        self.stream
            .write_all(&body)
            .await
            .map_err(|e| format!("send body: {e}"))?;

        Ok(())
    }

    /// Wait for and return the next full decrypted packet body (without header),
    /// or None when the peer closes the connection.
    pub async fn recv_packet(&mut self) -> Result<Option<Vec<u8>>, String> {
        loop {
            // A complete packet is buffered: [4-byte header][payload].
            if self.inbox.len() >= 4 {
                let len = Cryptography::check_length(&self.inbox[0..4]);

                if len > MAX_PACKET_LENGTH {
                    return Err(format!("packet length {len} exceeds cap"));
                }

                if self.inbox.len() >= 4 + len {
                    let mut body: Vec<u8> = self.inbox.drain(..4 + len).collect();
                    body.drain(..4);
                    self.crypto.decrypt(&mut body);

                    if self.show_packets {
                        let opcode = u16::from_le_bytes([body[0], body[1]]);
                        crate::emit_f!([] => "RECV [{opcode}] {} bytes", body.len());
                    }

                    if self.dump_packets {
                        let hex: Vec<String> = body.iter().map(|b| format!("{b:02X}")).collect();
                        crate::emit_f!([] => "RECVHEX: {}", hex.join(" "));
                    }

                    return Ok(Some(body));
                }
            }

            let mut buf = [0u8; 8192];
            let n = self
                .stream
                .read(&mut buf)
                .await
                .map_err(|e| format!("recv: {e}"))?;

            if n == 0 {
                return Ok(None);
            }

            self.inbox.extend_from_slice(&buf[..n]);
        }
    }
}

