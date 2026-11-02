//! Binary codecs for PostgreSQL's network and hardware address columns.
//!
//! `INET` and `CIDR` share one representation:
//!
//! ```text
//! int8 family   2 for IPv4, 3 for IPv6
//! int8 bits     prefix length
//! int8 is_cidr  1 for `CIDR`, 0 for `INET`
//! int8 nb       address octets that follow: 4 or 16
//! byte address[nb]
//! ```
//!
//! `MACADDR` and `MACADDR8` carry their octets verbatim.

use core::fmt::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use bytes::BytesMut;

/// Address family of an IPv4 value.
const FAMILY_IPV4: u8 = 2;

/// Address family of an IPv6 value.
const FAMILY_IPV6: u8 = 3;

/// Prefix length of a complete IPv4 address.
const BITS_IPV4: u8 = 32;

/// Prefix length of a complete IPv6 address.
const BITS_IPV6: u8 = 128;

/// Encodes the textual address `text` into the binary `INET` / `CIDR` form.
pub(super) fn encode_address(text: &str, cidr: bool, out: &mut BytesMut) -> Result<(), String> {
    let trimmed = text.trim();
    let (address, prefix) = match trimmed.split_once('/') {
        Some((address, prefix)) => (address.trim(), Some(prefix.trim())),
        None => (trimmed, None),
    };
    let parsed: IpAddr = address
        .parse()
        .map_err(|_| format!("`{address}` is not an IP address"))?;

    let complete = match parsed {
        IpAddr::V4(_) => BITS_IPV4,
        IpAddr::V6(_) => BITS_IPV6,
    };
    let bits = match prefix {
        Some(prefix) => prefix
            .parse::<u8>()
            .map_err(|_| format!("`{prefix}` is not a prefix length"))?,
        None => complete,
    };
    if bits > complete {
        return Err(format!(
            "a prefix length of {bits} exceeds the {complete} bits of `{address}`"
        ));
    }

    match parsed {
        IpAddr::V4(address) => {
            out.extend_from_slice(&[FAMILY_IPV4, bits, u8::from(cidr), 4]);
            out.extend_from_slice(&address.octets());
        }
        IpAddr::V6(address) => {
            out.extend_from_slice(&[FAMILY_IPV6, bits, u8::from(cidr), 16]);
            out.extend_from_slice(&address.octets());
        }
    }

    Ok(())
}

/// Decodes the binary `INET` / `CIDR` form into its textual address.
///
/// A host address of a complete prefix is rendered without the suffix, which is
/// how PostgreSQL spells it, while a `CIDR` value always keeps its prefix.
pub(super) fn decode_address(bytes: &[u8]) -> Result<String, String> {
    let header = bytes
        .get(..4)
        .ok_or_else(|| "a network address value was truncated".to_owned())?;
    let header: [u8; 4] = header
        .try_into()
        .map_err(|_| "a network address value was truncated".to_owned())?;
    let family = header[0];
    let bits = header[1];
    let cidr = header[2];
    let octets = header[3];

    let length = usize::from(octets);
    let address = bytes
        .get(4..4 + length)
        .ok_or_else(|| "a network address value was truncated".to_owned())?;
    if bytes.len() != 4 + length {
        return Err("a network address value carries trailing octets".to_owned());
    }

    let (text, complete) = match family {
        FAMILY_IPV4 => {
            let address: [u8; 4] = address
                .try_into()
                .map_err(|_| format!("an IPv4 address carries {length} octets"))?;
            (Ipv4Addr::from(address).to_string(), BITS_IPV4)
        }
        FAMILY_IPV6 => {
            let address: [u8; 16] = address
                .try_into()
                .map_err(|_| format!("an IPv6 address carries {length} octets"))?;
            (Ipv6Addr::from(address).to_string(), BITS_IPV6)
        }
        other => {
            return Err(format!(
                "a network address carries the unknown family {other}"
            ))
        }
    };

    if bits > complete {
        return Err(format!(
            "a network address declares a {bits}-bit prefix over {complete} bits"
        ));
    }

    if cidr == 1 || bits < complete {
        Ok(format!("{text}/{bits}"))
    } else {
        Ok(text)
    }
}

/// Encodes the textual hardware address `text` into its octets.
///
/// PostgreSQL accepts the octets of a `macaddr` as colon-, dash-, or
/// dot-separated runs of hex digits, and one run may spell several octets, so
/// `08:00:2b:01:02:03`, `08-00-2b-01-02-03`, `08002b:010203`, `0800.2b01.0203`,
/// and `08002b010203` all name the same address. Every group therefore holds
/// whole octets, and the assembled total has to match the width of the column.
pub(super) fn encode_hardware(text: &str, octets: usize, out: &mut BytesMut) -> Result<(), String> {
    let trimmed = text.trim();
    let mut bytes = Vec::with_capacity(octets);

    for group in trimmed.split([':', '-', '.']) {
        if group.is_empty() {
            return Err(format!("`{trimmed}` is not a hardware address"));
        }

        for pair in group.as_bytes().chunks(2) {
            if pair.len() != 2 {
                return Err(format!("`{trimmed}` is not a hardware address"));
            }
            let pair = core::str::from_utf8(pair)
                .map_err(|_| format!("`{trimmed}` is not a hardware address"))?;
            let byte = u8::from_str_radix(pair, 16)
                .map_err(|_| format!("`{trimmed}` is not a hardware address"))?;
            bytes.push(byte);
        }
    }

    if bytes.len() != octets {
        return Err(format!(
            "`{trimmed}` carries {} octets where {octets} are expected",
            bytes.len()
        ));
    }

    out.extend_from_slice(&bytes);
    Ok(())
}

/// Decodes the octets of a hardware address into its textual form.
pub(super) fn decode_hardware(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() != 6 && bytes.len() != 8 {
        return Err(format!("a hardware address carries {} octets", bytes.len()));
    }

    let mut text = String::with_capacity(bytes.len() * 3);
    for (position, byte) in bytes.iter().enumerate() {
        if position > 0 {
            text.push(':');
        }
        let _ = write!(text, "{byte:02x}");
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use super::{decode_address, decode_hardware, encode_address, encode_hardware};

    #[test]
    fn an_ipv4_network_round_trips_through_its_octets() {
        let mut out = BytesMut::new();
        encode_address("10.1.2.0/24", true, &mut out).expect("the address is encodable");

        assert_eq!(out.to_vec(), vec![2u8, 24, 1, 4, 10, 1, 2, 0]);
        assert_eq!(decode_address(&out), Ok("10.1.2.0/24".to_owned()));
    }

    #[test]
    fn a_host_address_keeps_its_bare_textual_form() {
        let mut out = BytesMut::new();
        encode_address("192.168.1.5", false, &mut out).expect("the address is encodable");

        assert_eq!(decode_address(&out), Ok("192.168.1.5".to_owned()));
    }

    #[test]
    fn an_ipv6_network_round_trips_through_its_octets() {
        let mut out = BytesMut::new();
        encode_address("2001:db8::1/64", true, &mut out).expect("the address is encodable");

        assert_eq!(decode_address(&out), Ok("2001:db8::1/64".to_owned()));
    }

    #[test]
    fn an_unusable_address_is_rejected() {
        assert!(encode_address("not an address", false, &mut BytesMut::new()).is_err());
        assert!(encode_address("10.0.0.1/33", false, &mut BytesMut::new()).is_err());
    }

    #[test]
    fn a_hardware_address_round_trips_through_its_octets() {
        let mut out = BytesMut::new();
        encode_hardware("08:00:2B:01:02:03", 6, &mut out).expect("the address is encodable");

        assert_eq!(out.to_vec(), vec![0x08u8, 0x00, 0x2b, 0x01, 0x02, 0x03]);
        assert_eq!(decode_hardware(&out), Ok("08:00:2b:01:02:03".to_owned()));
        assert!(encode_hardware("08:00:2b:01:02:03", 8, &mut BytesMut::new()).is_err());
    }

    #[test]
    fn a_compact_hardware_address_round_trips_through_its_octets() {
        let expected = vec![0x08u8, 0x00, 0x2b, 0x01, 0x02, 0x03];

        for spelling in [
            "08:00:2b:01:02:03",
            "08-00-2b-01-02-03",
            "08002b:010203",
            "08002b-010203",
            "0800.2b01.0203",
            "08002b010203",
        ] {
            let mut out = BytesMut::new();
            encode_hardware(spelling, 6, &mut out).expect("the address is encodable");

            assert_eq!(out.to_vec(), expected, "`{spelling}` names the same address");
        }
    }

    #[test]
    fn an_unusable_hardware_address_is_rejected() {
        assert!(encode_hardware("08:00:2b:01:02", 6, &mut BytesMut::new()).is_err());
        assert!(encode_hardware("08:00:2b:01:02:0", 6, &mut BytesMut::new()).is_err());
        assert!(encode_hardware("08::2b:01:02:03", 6, &mut BytesMut::new()).is_err());
        assert!(encode_hardware("08:00:2b:01:02:zz", 6, &mut BytesMut::new()).is_err());
    }
}
