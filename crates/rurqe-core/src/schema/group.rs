//! Schema groups: the `schema!` macro and the accessor it declares.

/// Declares one database schema together with the tables it contains.
///
/// The macro is the level above [`table!`](crate::table): it states the SQL
/// name of the schema once, emits one module holding every table of that
/// schema, and adds a zero-sized accessor addressing the tables through the
/// schema they belong to.
///
/// ```
/// use rurqe_core::schema::{Column, Table};
///
/// rurqe_core::schema! {
///     /// The `auth` schema.
///     Auth => "auth" as auth {
///         Sessions => "sessions" {
///             SessionsId => "id": i64,
///         }
///     }
/// }
///
/// // The module carries the type namespace of the schema.
/// assert_eq!(<auth::Sessions as Table>::SCHEMA, Some("auth"));
/// assert_eq!(<auth::SessionsId as Column>::NAME, "id");
///
/// // The accessor addresses the very same markers as values.
/// assert_eq!(Auth.Sessions, auth::Sessions);
/// assert_eq!(Auth.Sessions.SessionsId, auth::SessionsId);
/// ```
///
/// The module name is written after `as` instead of being derived from the
/// schema marker, because a declarative macro cannot change the case of an
/// identifier: the marker stays in `PascalCase` (`Auth`) while the module
/// follows the naming convention of modules (`auth`).
///
/// Every table is forwarded to [`table!`](crate::table) with the schema name
/// attached, so its columns keep the shape a plain declaration produces while
/// the relation renders `"auth"."sessions"` in `FROM` and `JOIN` clauses.
///
/// Items that belong next to the tables — an enumeration, a record, an
/// import, or a table alias — are declared in a trailing `@items` block,
/// which is written into the generated module verbatim:
///
/// ```
/// rurqe_core::schema! {
///     /// The `audit` schema.
///     Audit => "audit" as audit {
///         Events => "events" {
///             EventId => "id": i64,
///         }
///
///         @items {
///             use rurqe_core::table_alias;
///
///             table_alias! {
///                 /// The `e` alias.
///                 EventAlias => Events as "e"
///             }
///         }
///     }
/// }
///
/// use rurqe_core::schema::TableAlias;
///
/// assert_eq!(<audit::EventAlias as TableAlias>::NAME, "e");
/// ```
#[macro_export]
macro_rules! schema {
    (
        $(#[$schema_meta:meta])*
        $schema:ident => $schema_name:literal as $module:ident {
            $(
                $(#[$table_meta:meta])*
                $table:ident => $table_name:literal { $($columns:tt)* }
            )*
            $(@items { $($items:tt)* })?
        }
    ) => {
        $(#[$schema_meta])*
        pub mod $module {
            $(
                $crate::table! {
                    $(#[$table_meta])*
                    $table => $table_name in $schema_name {
                        $($columns)*
                    }
                }
            )*

            $($($items)*)?
        }

        $(#[$schema_meta])*
        #[allow(dead_code, non_snake_case)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $schema {
            $(
                $(#[$table_meta])*
                #[allow(non_snake_case)]
                pub $table: $module::$table,
            )*
        }

        #[allow(dead_code, non_upper_case_globals)]
        pub const $schema: $schema = $schema {
            $($table: $module::$table,)*
        };
    };
}
