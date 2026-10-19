//! Bound SQL parameter values and the primitive operator enums of expressions.

/// A SQL literal bound to a single query parameter.
///
/// Values stay in their Rust representation until the dialect renders them, so
/// the produced parameter list remains typed and allocation-free for scalars.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlVal {
    /// The SQL `NULL` value.
    Null,
    /// A 32-bit signed integer.
    I32(i32),
    /// A 64-bit signed integer.
    I64(i64),
    /// A 64-bit floating point number.
    F64(f64),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string.
    Text(String),
    /// A binary blob.
    Bytes(Vec<u8>),
}

impl From<i32> for SqlVal {
    fn from(value: i32) -> Self {
        SqlVal::I32(value)
    }
}

impl From<i64> for SqlVal {
    fn from(value: i64) -> Self {
        SqlVal::I64(value)
    }
}

impl From<f64> for SqlVal {
    fn from(value: f64) -> Self {
        SqlVal::F64(value)
    }
}

impl From<bool> for SqlVal {
    fn from(value: bool) -> Self {
        SqlVal::Bool(value)
    }
}

impl From<String> for SqlVal {
    fn from(value: String) -> Self {
        SqlVal::Text(value)
    }
}

impl From<&str> for SqlVal {
    fn from(value: &str) -> Self {
        SqlVal::Text(value.to_owned())
    }
}

impl From<&String> for SqlVal {
    fn from(value: &String) -> Self {
        SqlVal::Text(value.clone())
    }
}

impl From<Vec<u8>> for SqlVal {
    fn from(value: Vec<u8>) -> Self {
        SqlVal::Bytes(value)
    }
}

impl From<&[u8]> for SqlVal {
    fn from(value: &[u8]) -> Self {
        SqlVal::Bytes(value.to_vec())
    }
}

impl From<u32> for SqlVal {
    fn from(value: u32) -> Self {
        SqlVal::I64(value as i64)
    }
}

impl From<usize> for SqlVal {
    fn from(value: usize) -> Self {
        SqlVal::I64(value as i64)
    }
}

impl From<i16> for SqlVal {
    fn from(value: i16) -> Self {
        SqlVal::I32(value as i32)
    }
}

/// An aggregate function applied to a column or expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateFunc {
    /// `COUNT`
    Count,
    /// `SUM`
    Sum,
    /// `AVG`
    Avg,
    /// `MIN`
    Min,
    /// `MAX`
    Max,
}

impl AggregateFunc {
    /// The SQL name of the aggregate function.
    pub const fn as_str(self) -> &'static str {
        match self {
            AggregateFunc::Count => "COUNT",
            AggregateFunc::Sum => "SUM",
            AggregateFunc::Avg => "AVG",
            AggregateFunc::Min => "MIN",
            AggregateFunc::Max => "MAX",
        }
    }
}

/// The binary operator joining two sub-expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `>`
    Gt,
    /// `<`
    Lt,
    /// `AND`
    And,
    /// `OR`
    Or,
}

impl BinaryOp {
    /// The SQL text of the operator.
    pub const fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Gt => ">",
            BinaryOp::Lt => "<",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
        }
    }
}
