//! A nested row reports the missing JSON capability in a build without the feature.

use super::*;
use crate::row::RowDecodeError;

#[test]
fn a_nested_row_reports_the_missing_json_feature() {
    let error = match json_object(&SqlVal::Text("{}".to_owned())) {
        Ok(_) => panic!("a build without `serde_json` decodes no nested row"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        RowDecodeError::InvalidValue { detail, .. } if detail.contains("serde_json")
    ));

    assert!(matches!(
        multiset_object(&[("id", SqlVal::I64(1))]),
        SqlVal::EncodingError(_)
    ));
    assert!(nested_field::<i64>(&RowObject, "id").is_err());
}
