//! SHA-256 checksums of migration scripts.
//!
//! RURQE ships its own dependency-free SHA-256 implementation: digesting a
//! migration script must stay deterministic and available in every build,
//! including fully offline ones.
//!
//! ```
//! use rurqe_migrate::Checksum;
//!
//! let checksum = Checksum::of(b"CREATE TABLE users ();");
//! assert_eq!(checksum.to_hex().len(), 64);
//! assert_eq!(Checksum::from_hex(&checksum.to_hex()), Ok(checksum));
//! ```

use core::fmt;

use crate::error::Error;

/// Size of a SHA-256 digest in bytes.
const CHECKSUM_LEN: usize = 32;

/// Round constants of the SHA-256 compression function.
const ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1, 0x923f_82a4,
    0xab1c_5ed5, 0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3, 0x72be_5d74, 0x80de_b1fe,
    0x9bdc_06a7, 0xc19b_f174, 0xe49b_69c1, 0xefbe_4786, 0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f,
    0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da, 0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7,
    0xc6e0_0bf3, 0xd5a7_9147, 0x06ca_6351, 0x1429_2967, 0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc,
    0x5338_0d13, 0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85, 0xa2bf_e8a1, 0xa81a_664b,
    0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070, 0x19a4_c116,
    0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a, 0x5b9c_ca4f, 0x682e_6ff3,
    0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208, 0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7,
    0xc671_78f2,
];

/// Initial hash state of SHA-256.
const INITIAL_STATE: [u32; 8] = [
    0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c, 0x1f83_d9ab,
    0x5be0_cd19,
];

/// The SHA-256 digest of a migration script.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Checksum([u8; CHECKSUM_LEN]);

impl Checksum {
    /// Computes the SHA-256 digest of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha256(bytes))
    }

    /// Decodes a hexadecimal digest produced by [`Checksum::to_hex`].
    pub fn from_hex(hex: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidChecksum {
            checksum: hex.to_owned(),
        };

        if hex.len() != CHECKSUM_LEN * 2 {
            return Err(invalid());
        }

        let mut digest = [0u8; CHECKSUM_LEN];
        for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            let pair = core::str::from_utf8(pair).map_err(|_| invalid())?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| invalid())?;
        }

        Ok(Self(digest))
    }

    /// The raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; CHECKSUM_LEN] {
        &self.0
    }

    /// The digest encoded as 64 lowercase hexadecimal characters.
    pub fn to_hex(&self) -> String {
        const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

        let mut hex = String::with_capacity(CHECKSUM_LEN * 2);
        for byte in self.0 {
            hex.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
            hex.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
        }
        hex
    }
}

impl fmt::Display for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Checksum({})", self.to_hex())
    }
}

/// Computes the SHA-256 digest of `data`.
pub fn sha256(data: &[u8]) -> [u8; CHECKSUM_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize()
}

/// Incremental SHA-256 hasher over 64-byte blocks.
struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    block_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: INITIAL_STATE,
            block: [0; 64],
            block_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);

        if self.block_len > 0 {
            let take = (64 - self.block_len).min(data.len());
            self.block[self.block_len..self.block_len + take].copy_from_slice(&data[..take]);
            self.block_len += take;
            data = &data[take..];

            if self.block_len == 64 {
                let block = self.block;
                self.compress(&block);
                self.block_len = 0;
            }
        }

        while data.len() >= 64 {
            let (head, tail) = data.split_at(64);
            let mut block = [0u8; 64];
            block.copy_from_slice(head);
            self.compress(&block);
            data = tail;
        }

        if !data.is_empty() {
            self.block[..data.len()].copy_from_slice(data);
            self.block_len = data.len();
        }
    }

    fn finalize(mut self) -> [u8; CHECKSUM_LEN] {
        let bit_len = self.total_len.wrapping_mul(8);

        self.block[self.block_len] = 0x80;
        self.block_len += 1;

        if self.block_len > 56 {
            self.block[self.block_len..].fill(0);
            let block = self.block;
            self.compress(&block);
            self.block_len = 0;
        }

        self.block[self.block_len..56].fill(0);
        self.block[56..].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.block;
        self.compress(&block);

        let mut digest = [0u8; CHECKSUM_LEN];
        for (chunk, word) in digest.chunks_exact_mut(4).zip(self.state.iter()) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut schedule = [0u32; 64];
        for (word, chunk) in schedule.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }

        for index in 16..64 {
            let x = schedule[index - 15];
            let y = schedule[index - 2];
            let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let s1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;

        for (&constant, &word) in ROUND_CONSTANTS.iter().zip(schedule.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(constant)
                .wrapping_add(word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digests_match_the_reference_vectors() {
        assert_eq!(
            Checksum::of(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Checksum::of(b"abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            Checksum::of(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").to_hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            Checksum::of(b"The quick brown fox jumps over the lazy dog").to_hex(),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn incremental_updates_match_a_single_pass() {
        let data = b"The quick brown fox jumps over the lazy dog";

        let mut hasher = Sha256::new();
        for chunk in data.chunks(7) {
            hasher.update(chunk);
        }

        assert_eq!(hasher.finalize(), sha256(data));
    }

    #[test]
    fn hex_encoding_round_trips() {
        let checksum = Checksum::of(b"CREATE TABLE users ();");
        let hex = checksum.to_hex();

        assert_eq!(hex.len(), 64);
        assert_eq!(Checksum::from_hex(&hex), Ok(checksum));
        assert_eq!(checksum.to_string(), hex);
        assert_eq!(format!("{checksum:?}"), format!("Checksum({hex})"));
        assert_eq!(checksum.as_bytes().len(), 32);
    }

    #[test]
    fn malformed_hex_is_rejected() {
        assert_eq!(
            Checksum::from_hex("abc"),
            Err(Error::InvalidChecksum {
                checksum: "abc".to_owned()
            })
        );

        let not_hex = "z".repeat(64);
        let rejected = Checksum::from_hex(&not_hex);
        assert_eq!(rejected, Err(Error::InvalidChecksum { checksum: not_hex }));
    }
}
