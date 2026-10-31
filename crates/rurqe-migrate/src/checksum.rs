//! SHA-256 checksums of migration scripts.
//!
//! Digests are produced by the audited [`sha2`] crate: migration checksums
//! must stay deterministic and identical across every RURQE build, so the raw
//! compression function is never reimplemented locally.
//!
//! ```
//! use rurqe_migrate::Checksum;
//!
//! let checksum = Checksum::of(b"CREATE TABLE users ();");
//! assert_eq!(checksum.to_hex().len(), 64);
//! assert_eq!(Checksum::from_hex(&checksum.to_hex()), Ok(checksum));
//! ```

use core::fmt;

use sha2::{Digest, Sha256};

use crate::error::Error;

/// Size of a SHA-256 digest in bytes.
const CHECKSUM_LEN: usize = 32;

/// The SHA-256 digest of a migration script.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Checksum([u8; CHECKSUM_LEN]);

impl Checksum {
    /// Computes the SHA-256 digest of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha256(bytes))
    }

    /// Decodes a hexadecimal digest produced by [`Checksum::to_hex`].
    ///
    /// The input must be exactly 64 hexadecimal digits. Lowercase and uppercase
    /// digits decode to the same digest, while [`Checksum::to_hex`] always emits
    /// the lowercase spelling, which stays the canonical form. Every other
    /// character, including the `+` sign a numeric parser tolerates, is rejected
    /// with [`Error::InvalidChecksum`].
    pub fn from_hex(hex: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidChecksum {
            checksum: hex.to_owned(),
        };

        let digits = hex.as_bytes();
        if digits.len() != CHECKSUM_LEN * 2 {
            return Err(invalid());
        }

        let mut digest = [0u8; CHECKSUM_LEN];
        for (byte, pair) in digest.iter_mut().zip(digits.chunks_exact(2)) {
            *byte = hex_byte(pair).ok_or_else(invalid)?;
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

/// Decodes one hexadecimal digit, accepting both letter cases.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes one two-digit pair into a byte.
///
/// The digits are read from their bytes instead of through `u8::from_str_radix`,
/// whose sign handling admits `+`-prefixed pairs and lets text that is not
/// hexadecimal decode as a valid digest. A pair that is not exactly two digits
/// or that holds a non-hexadecimal byte yields `None`.
fn hex_byte(pair: &[u8]) -> Option<u8> {
    match pair {
        [high, low] => Some((hex_digit(*high)? << 4) | hex_digit(*low)?),
        _ => None,
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
    digest_of(Sha256::digest(data).as_slice())
}

/// Copies a digest into the fixed-size checksum buffer.
///
/// The copy is length-tolerant: a payload of another length leaves the rest of
/// the buffer zeroed instead of aborting the process, so the helper carries no
/// panic path even though every caller hands in a SHA-256 digest.
fn digest_of(bytes: &[u8]) -> [u8; CHECKSUM_LEN] {
    let mut digest = [0u8; CHECKSUM_LEN];
    for (slot, byte) in digest.iter_mut().zip(bytes) {
        *slot = *byte;
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Computes a digest through the incremental hasher.
    fn incremental<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> [u8; CHECKSUM_LEN] {
        let mut hasher = Sha256::new();
        for chunk in chunks {
            hasher.update(chunk);
        }
        digest_of(hasher.finalize().as_slice())
    }

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
    fn million_character_vector_matches_the_reference_digest() {
        let payload = vec![b'a'; 1_000_000];

        assert_eq!(
            Checksum::of(&payload).to_hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn incremental_updates_match_a_single_pass() {
        let data = b"The quick brown fox jumps over the lazy dog";

        let mut hasher = Sha256::new();
        for chunk in data.chunks(7) {
            hasher.update(chunk);
        }

        assert_eq!(digest_of(hasher.finalize().as_slice()), sha256(data));
    }

    #[test]
    fn block_boundary_inputs_stay_deterministic() {
        let block = [b'x'; 64];

        assert_eq!(incremental([block.as_slice()]), sha256(&block));
        assert_eq!(
            incremental([block.as_slice(), block.as_slice()]),
            sha256(&[block; 2].concat())
        );
        assert_eq!(
            incremental([block.as_slice(), b"tail".as_slice()]),
            sha256(&[block.as_slice(), b"tail".as_slice()].concat())
        );
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

    #[test]
    fn sign_bearing_pairs_are_rejected() {
        let signed = "+0+1+2+3+4+5+6+7+8+9+a+b+c+d+e+f".repeat(2);
        assert_eq!(signed.len(), 64);

        let rejected = Checksum::from_hex(&signed);
        assert_eq!(rejected, Err(Error::InvalidChecksum { checksum: signed }));

        let canonical = Checksum::of(b"sign-bearing").to_hex();
        for position in (0..canonical.len()).step_by(2) {
            let mut digits = canonical.clone().into_bytes();
            let slot = digits
                .get_mut(position)
                .expect("the position stays inside the digest");
            *slot = b'+';

            let signed = String::from_utf8(digits).expect("hexadecimal digits stay ASCII");
            let rejected = Checksum::from_hex(&signed);
            assert_eq!(rejected, Err(Error::InvalidChecksum { checksum: signed }));
        }
    }

    #[test]
    fn uppercase_digits_decode_to_the_canonical_digest() {
        let checksum = Checksum::of(b"CREATE TABLE users ();");
        let uppercase = checksum.to_hex().to_uppercase();

        assert_eq!(Checksum::from_hex(&uppercase), Ok(checksum));
        assert_eq!(checksum.to_hex(), uppercase.to_lowercase());
    }
}
