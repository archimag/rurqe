//! Compile-time guard for the flat crate-root export of the dialect capability markers.
//!
//! Every capability marker lives in `rurqe_core::dialect` and is re-exported from the
//! crate root, so a consumer names `rurqe_core::SupportsReturning` rather than reaching
//! into the `dialect` module. The bounds below resolve every marker through its root
//! path: removing a name from the re-export list stops this target from compiling.

#[allow(dead_code)]
fn capability_markers_are_exported_at_the_crate_root<T>() -> core::marker::PhantomData<T>
where
    T: rurqe_core::SupportsAggregateFilter
        + rurqe_core::SupportsAliasedInsertTarget
        + rurqe_core::SupportsDeleteUsing
        + rurqe_core::SupportsDistinctFrom
        + rurqe_core::SupportsDistinctOn
        + rurqe_core::SupportsFullOuterJoin
        + rurqe_core::SupportsGreatest
        + rurqe_core::SupportsJsonAggregation
        + rurqe_core::SupportsLateralJoin
        + rurqe_core::SupportsNullsOrdering
        + rurqe_core::SupportsOnConflict
        + rurqe_core::SupportsReturning
        + rurqe_core::SupportsRowLocking
        + rurqe_core::SupportsUpdateFrom
        + rurqe_core::SupportsWindowFunctions,
{
    core::marker::PhantomData
}
