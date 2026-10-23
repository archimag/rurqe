//! Classification of a PostgreSQL column type into the representation it uses on
//! the wire.
//!
//! The code generator maps a column without a dedicated [`SqlVal`] variant onto
//! `String`, so the adapter has to reach those columns through their textual
//! form. The classification names that form for every supported type and
//! reports the rest as unsupported instead of guessing a layout the server would
//! misinterpret.

use super::driver::types::{Kind, Type};
use super::{network, numeric};

/// Representation a column type uses on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wire {
    /// The value is UTF-8 text: `text`, `citext`, enum labels, and domains over
    /// those types.
    Text,
    /// The value is PostgreSQL's binary decimal representation.
    Numeric,
    /// The value is a network address; `cidr` tells `INET` from `CIDR`.
    Network {
        /// Whether the column was declared as `CIDR`.
        cidr: bool,
    },
    /// The value is a hardware address of `octets` octets.
    MacAddr {
        /// Number of octets the declared type carries.
        octets: usize,
    },
}

impl Wire {
    /// Decodes the binary payload `bytes` into the textual SQL value.
    pub(super) fn decode(self, bytes: &[u8]) -> Result<String, String> {
        match self {
            Wire::Text => String::from_utf8(bytes.to_vec())
                .map_err(|_| "the column does not hold UTF-8 text".to_owned()),
            Wire::Numeric => numeric::decode(bytes),
            Wire::Network { .. } => network::decode_address(bytes),
            Wire::MacAddr { .. } => network::decode_hardware(bytes),
        }
    }
}

/// Classifies `ty`, resolving the domain it may be declared over.
///
/// `None` means the adapter has no textual representation for the type, so a
/// parameter bound to it and a column of it are both reported as unsupported.
pub(super) fn wire_of(ty: &Type) -> Option<Wire> {
    let ty = base_type(ty);

    let wire = if matches!(
        ty,
        &Type::TEXT | &Type::VARCHAR | &Type::BPCHAR | &Type::NAME | &Type::UNKNOWN
    ) {
        Wire::Text
    } else if matches!(ty, &Type::NUMERIC) {
        Wire::Numeric
    } else if matches!(ty, &Type::INET) {
        Wire::Network { cidr: false }
    } else if matches!(ty, &Type::CIDR) {
        Wire::Network { cidr: true }
    } else if ty.name() == "citext" {
        // `citext` is an extension type: it shares the text representation, yet
        // the driver carries no constant for its identifier.
        Wire::Text
    } else if ty.name() == "macaddr" {
        Wire::MacAddr { octets: 6 }
    } else if ty.name() == "macaddr8" {
        Wire::MacAddr { octets: 8 }
    } else if matches!(ty.kind(), Kind::Enum(_)) {
        // An enum travels as the label it was declared with.
        Wire::Text
    } else {
        return None;
    };

    Some(wire)
}

/// Resolves the type a domain is declared over.
///
/// A domain is reported as a type of its own, while its binary representation is
/// the one of the type it is declared over, so the classification always looks
/// through the whole domain chain.
fn base_type(ty: &Type) -> &Type {
    let mut base = ty;
    while let Kind::Domain(inner) = base.kind() {
        base = inner;
    }
    base
}
