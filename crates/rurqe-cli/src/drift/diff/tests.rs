//! Wording of the rendered findings.
//!
//! Every category of a [`SchemaDiff`] is rendered by one shared implementation, so
//! these assertions lock the exact sentence of each state and each qualifier: a
//! category added or renamed later cannot drift silently.

use core::fmt;
use std::collections::BTreeMap;

use super::{DiffWording, SchemaDiff, TableDiff};

/// Renders a diff with the wording a verifier uses.
struct Rendered<'a>(&'a SchemaDiff, DiffWording);

impl fmt::Display for Rendered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_details(f, self.1)
    }
}

#[test]
fn top_level_categories_render_every_state() {
    let diff = SchemaDiff {
        missing_enums: vec!["retired".to_owned()],
        extra_enums: vec!["task_state".to_owned()],
        altered_enums: vec!["user_role".to_owned()],
        missing_views: vec!["active_users".to_owned()],
        extra_views: vec!["archived_users".to_owned()],
        altered_views: vec!["recent_users".to_owned()],
        missing_triggers: vec!["users_audit".to_owned()],
        extra_triggers: vec!["users_touch".to_owned()],
        altered_triggers: vec!["orders_audit".to_owned()],
        missing_tables: vec!["legacy".to_owned()],
        extra_tables: vec!["draft".to_owned()],
        ..SchemaDiff::default()
    };

    assert_eq!(
        Rendered(&diff, DiffWording::SNAPSHOT).to_string(),
        concat!(
            "  - Enum `retired` is declared in snapshot but missing from database.\n",
            "  - Enum `task_state` exists in database but is missing from snapshot.\n",
            "  - Enum `user_role` definition differs between database and snapshot.\n",
            "  - View `active_users` is declared in snapshot but missing from database.\n",
            "  - View `archived_users` exists in database but is missing from snapshot.\n",
            "  - View `recent_users` definition differs between database and snapshot.\n",
            "  - Trigger `users_audit` is declared in snapshot but missing from database.\n",
            "  - Trigger `users_touch` exists in database but is missing from snapshot.\n",
            "  - Trigger `orders_audit` definition differs between database and snapshot.\n",
            "  - Table `legacy` is declared in snapshot but missing from database.\n",
            "  - Table `draft` exists in database but is missing from snapshot.\n",
        )
    );
}

#[test]
fn table_members_are_qualified_by_the_owning_table() {
    let mut table_diffs = BTreeMap::new();
    table_diffs.insert(
        "users".to_owned(),
        TableDiff {
            missing_columns: vec!["email".to_owned()],
            extra_columns: vec!["phone".to_owned()],
            altered_columns: vec!["age".to_owned()],
            primary_key_mismatch: true,
            foreign_key_mismatch: true,
            missing_checks: vec!["users_age_check".to_owned()],
            extra_checks: vec!["users_name_check".to_owned()],
            altered_checks: vec!["users_email_check".to_owned()],
            missing_indexes: vec!["users_email_idx".to_owned()],
            extra_indexes: vec!["users_phone_idx".to_owned()],
            altered_indexes: vec!["users_age_idx".to_owned()],
        },
    );

    let diff = SchemaDiff {
        table_diffs,
        ..SchemaDiff::default()
    };

    assert_eq!(
        Rendered(&diff, DiffWording::SNAPSHOT).to_string(),
        concat!(
            "  - Table `users`: column `email` is in snapshot but missing from database.\n",
            "  - Table `users`: column `phone` exists in database but is missing from snapshot.\n",
            "  - Table `users`: column `age` definition differs between database and snapshot.\n",
            "  - Table `users`: primary key constraint differs between database and snapshot.\n",
            "  - Table `users`: foreign key constraints differ between database and snapshot.\n",
            "  - Table `users`: check `users_age_check` is in snapshot but missing from database.\n",
            "  - Table `users`: check `users_name_check` exists in database but is missing from snapshot.\n",
            "  - Table `users`: check `users_email_check` definition differs between database and snapshot.\n",
            "  - Table `users`: index `users_email_idx` is in snapshot but missing from database.\n",
            "  - Table `users`: index `users_phone_idx` exists in database but is missing from snapshot.\n",
            "  - Table `users`: index `users_age_idx` definition differs between database and snapshot.\n",
        )
    );
}

#[test]
fn golden_wording_names_the_stage_and_the_tail() {
    let mut table_diffs = BTreeMap::new();
    table_diffs.insert(
        "users".to_owned(),
        TableDiff {
            missing_columns: vec!["email".to_owned()],
            ..TableDiff::default()
        },
    );

    let diff = SchemaDiff {
        missing_tables: vec!["legacy".to_owned()],
        table_diffs,
        ..SchemaDiff::default()
    };

    assert_eq!(
        Rendered(&diff, DiffWording::GOLDEN).to_string(),
        concat!(
            "  - Table `legacy` is declared in the stage but missing from the tail.\n",
            "  - Table `users`: column `email` is in the stage but missing from the tail.\n",
        )
    );
}
