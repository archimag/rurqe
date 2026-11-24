//! Destructive reset of the selected PostgreSQL schemas.

use rurqe_migrate::error::Error;
use rurqe_migrate::history::SCHEMA_HISTORY_TABLE;
use rurqe_migrate::introspect::SchemaSelection;

use super::backend_error;

/// Resolve the unqualified history relation through the session's search path.
pub(super) const HISTORY_SCHEMA: &str = "SELECT n.nspname FROM pg_class c \
    JOIN pg_namespace n ON n.oid = c.relnamespace \
    WHERE c.oid = to_regclass($1)";

/// The reset targets and statements shared by all PostgreSQL adapters.
pub(super) struct CleanPlan {
    schemas: Vec<String>,
    history_schema: Option<String>,
}

impl CleanPlan {
    pub(super) fn new(
        selection: &SchemaSelection,
        history_schema: Option<String>,
    ) -> Result<Self, Error> {
        let mut schemas = Vec::new();
        for name in selection.all() {
            if name.is_empty()
                || name.to_ascii_lowercase().starts_with("pg_")
                || name.eq_ignore_ascii_case("information_schema")
            {
                return Err(Error::Backend(format!(
                    "cannot clean reserved or empty PostgreSQL schema `{name}`"
                )));
            }
            if !schemas.contains(&name) {
                schemas.push(name);
            }
        }
        Ok(Self {
            schemas,
            history_schema,
        })
    }

    fn external_history(&self) -> Option<&str> {
        self.history_schema
            .as_deref()
            .filter(|schema| !self.schemas.iter().any(|selected| selected == schema))
    }

    pub(super) fn manifest(&self) -> Vec<String> {
        let mut names = self.schemas.clone();
        if let Some(schema) = self.external_history() {
            names.push(format!(
                "{}.{}",
                quote_ident(schema),
                quote_ident(SCHEMA_HISTORY_TABLE)
            ));
        }
        names
    }

    pub(super) fn statements(&self) -> Vec<String> {
        let mut statements = Vec::new();
        if let Some(schema) = self.external_history() {
            statements.push(format!(
                "DROP TABLE {}.{}",
                quote_ident(schema),
                quote_ident(SCHEMA_HISTORY_TABLE)
            ));
        }
        for schema in &self.schemas {
            statements.push(format!(
                "DROP SCHEMA IF EXISTS {} CASCADE",
                quote_ident(schema)
            ));
            statements.push(format!("CREATE SCHEMA {}", quote_ident(schema)));
        }
        statements
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub(super) fn from_rows(
    selection: &SchemaSelection,
    rows: &[postgres::Row],
) -> Result<CleanPlan, Error> {
    let schema = rows
        .first()
        .map(|row| row.try_get(0).map_err(backend_error))
        .transpose()?;
    CleanPlan::new(selection, schema)
}

pub(super) fn inspect<C: postgres::GenericClient>(
    client: &mut C,
    selection: &SchemaSelection,
) -> Result<CleanPlan, Error> {
    CleanPlan::new(selection, None)?;
    let rows = client
        .query(HISTORY_SCHEMA, &[&SCHEMA_HISTORY_TABLE])
        .map_err(backend_error)?;
    from_rows(selection, &rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_quotes_identifiers_and_deduplicates_schemas() {
        let selection = SchemaSelection::new("one\";drop", vec!["one\";drop".into(), "two".into()]);
        let plan = CleanPlan::new(&selection, Some("other".into())).unwrap();
        assert_eq!(
            plan.manifest(),
            vec!["one\";drop", "two", "\"other\".\"__rurqe_schema_history\""]
        );
        assert_eq!(
            plan.statements(),
            vec![
                "DROP TABLE \"other\".\"__rurqe_schema_history\"",
                "DROP SCHEMA IF EXISTS \"one\"\";drop\" CASCADE",
                "CREATE SCHEMA \"one\"\";drop\"",
                "DROP SCHEMA IF EXISTS \"two\" CASCADE",
                "CREATE SCHEMA \"two\"",
            ]
        );
    }

    #[test]
    fn selected_history_needs_no_separate_drop() {
        let plan = CleanPlan::new(&SchemaSelection::single("app"), Some("app".into())).unwrap();
        assert_eq!(plan.manifest(), vec!["app"]);
        assert_eq!(plan.statements().len(), 2);
    }

    #[test]
    fn reserved_schemas_are_refused() {
        for name in [
            "",
            "pg_catalog",
            "PG_temp_3",
            "pg_toast",
            "information_schema",
        ] {
            assert!(CleanPlan::new(&SchemaSelection::single(name), None).is_err());
        }
    }
}
