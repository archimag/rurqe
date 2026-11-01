//! The literal fragments of a raw SQL expression.
//!
//! A raw SQL template is split into literal text fragments interleaved with the
//! parameters it binds. The template of the `sql!` and `raw_sql!` macros is fixed
//! at expansion time, so its fragments are borrowed from the constant data of the
//! program instead of being rebuilt on every evaluation, while a template
//! assembled at run time keeps the strings `parse_raw_sql` split it into.

/// The literal text fragments a raw SQL expression interleaves with its parameters.
///
/// A fragment list holds exactly one entry more than the parameters it binds: the
/// renderer emits the fragments in order and binds one parameter between every two
/// of them. A list fixed at compile time is borrowed from the constant data of the
/// program, so evaluating the expression it belongs to copies no fragment text; a
/// list assembled at run time owns the strings it was built from. The two shapes
/// compare equal when they carry the same text, so the statement rules that match
/// one expression against another keep observing the value the expression names
/// rather than the storage it chose.
#[derive(Debug, Clone)]
pub enum RawFragments {
    /// Fragments fixed at compile time, borrowed from the program's constant data.
    Static(&'static [&'static str]),
    /// Fragments assembled at run time.
    Owned(Vec<String>),
}

impl RawFragments {
    /// The fragment at `index`, when the list is long enough.
    pub fn get(&self, index: usize) -> Option<&str> {
        match self {
            RawFragments::Static(fragments) => fragments.get(index).copied(),
            RawFragments::Owned(fragments) => fragments.get(index).map(String::as_str),
        }
    }

    /// The first fragment of the list, when it holds one.
    pub fn first(&self) -> Option<&str> {
        match self {
            RawFragments::Static(fragments) => fragments.first().copied(),
            RawFragments::Owned(fragments) => fragments.first().map(String::as_str),
        }
    }

    /// The number of fragments in the list.
    pub fn len(&self) -> usize {
        match self {
            RawFragments::Static(fragments) => fragments.len(),
            RawFragments::Owned(fragments) => fragments.len(),
        }
    }

    /// Whether the list holds no fragment.
    pub fn is_empty(&self) -> bool {
        match self {
            RawFragments::Static(fragments) => fragments.is_empty(),
            RawFragments::Owned(fragments) => fragments.is_empty(),
        }
    }

    /// Iterates over the fragments in template order.
    pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        (0..self.len()).filter_map(move |index| self.get(index))
    }
}

impl From<Vec<String>> for RawFragments {
    fn from(fragments: Vec<String>) -> Self {
        RawFragments::Owned(fragments)
    }
}

impl PartialEq for RawFragments {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }

        self.iter().zip(other.iter()).all(|(lhs, rhs)| lhs == rhs)
    }
}
