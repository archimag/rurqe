//! Binary codec for PostgreSQL array columns.
//!
//! An array value is a header followed by the payload of every element:
//!
//! ```text
//! int32 dimensions   number of dimensions that follow
//! int32 has_nulls    1 when some element is NULL
//! int32 element      OID of the element type
//! per dimension:
//!   int32 length     number of elements along the dimension
//!   int32 lower      lower bound of the dimension
//! per element:
//!   int32 length     octets that follow, or -1 for NULL
//!   byte  payload[length]
//! ```
//!
//! A `Vec<T>` column carries one dimension, so a value of several dimensions is
//! reported instead of being flattened into a misleading single one. A `NULL`
//! element decodes into [`SqlVal::Null`], which keeps the value it describes
//! intact even though the column type itself has no optional elements.

use rurqe_core::expr::SqlVal;

use super::driver::types::Type;
use super::{decode_value, DecodeError};

/// Octets taken by one `int32` field of the representation.
const FIELD_OCTETS: usize = 4;

/// Length field marking an element that holds SQL `NULL`.
const NULL_ELEMENT: i32 = -1;

/// Decodes the binary payload of an array into the values of its elements.
pub(super) fn decode(elem_ty: &Type, bytes: &[u8]) -> Result<Vec<SqlVal>, String> {
    let mut cursor = Cursor::new(bytes);
    let dimensions = cursor.read_field()?;
    cursor.read_field()?;
    cursor.read_field()?;

    // An array of no dimension holds no element at all, which is how PostgreSQL
    // spells an empty value.
    if dimensions == 0 {
        return Ok(Vec::new());
    }
    if dimensions != 1 {
        return Err(format!(
            "an array value carries {dimensions} dimensions, where a column of a single dimension is expected"
        ));
    }

    let count = cursor.read_count()?;
    cursor.read_field()?;

    // Every element carries at least its own length field, so the declared count
    // is bounded by the payload before any memory is reserved for it.
    if count > cursor.remaining() / FIELD_OCTETS {
        return Err("an array value declares more elements than it carries".to_owned());
    }

    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        match cursor.read_element()? {
            Some(payload) => {
                values.push(decode_value(elem_ty, payload).map_err(DecodeError::describe)?);
            }
            None => values.push(SqlVal::Null),
        }
    }

    Ok(values)
}

/// Reader over the binary payload of an array value.
struct Cursor<'a> {
    /// The payload being read.
    bytes: &'a [u8],
    /// Offset of the next unread octet.
    offset: usize,
}

impl<'a> Cursor<'a> {
    /// Starts reading at the beginning of `bytes`.
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    /// Number of octets left to read.
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    /// Reads the `int32` field at the cursor.
    fn read_field(&mut self) -> Result<i32, String> {
        let field: [u8; FIELD_OCTETS] = self
            .read_octets(FIELD_OCTETS)?
            .try_into()
            .map_err(|_| truncated())?;
        Ok(i32::from_be_bytes(field))
    }

    /// Reads a field declaring a number of octets or elements.
    fn read_count(&mut self) -> Result<usize, String> {
        let count = self.read_field()?;
        usize::try_from(count)
            .map_err(|_| format!("an array value declares the negative count {count}"))
    }

    /// Reads one element, reporting a `NULL` element as `None`.
    fn read_element(&mut self) -> Result<Option<&'a [u8]>, String> {
        let length = self.read_field()?;
        if length == NULL_ELEMENT {
            return Ok(None);
        }
        let length = usize::try_from(length)
            .map_err(|_| format!("an array element declares the negative length {length}"))?;
        self.read_octets(length).map(Some)
    }

    /// Reads `length` octets from the cursor.
    fn read_octets(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self.offset.checked_add(length).ok_or_else(truncated)?;
        let octets = self.bytes.get(self.offset..end).ok_or_else(truncated)?;
        self.offset = end;
        Ok(octets)
    }
}

/// Reports a payload that ends before its declared shape does.
fn truncated() -> String {
    "an array value ended before its declared shape".to_owned()
}
