# Migrations

A migration is a forward-only SQL script kept in the repository and applied once
against a database. The applied entry is recorded in a history table together
with the checksum of the script, so an edit to a script that already ran is
detected instead of being silently accepted.

Versioned scripts carry an ordered version and run in that order exactly once.
Repeatable scripts carry no version and run again whenever their contents
change, which is the shape a view or a function definition takes.

This chapter covers the script naming rules, the history table, applying and
inspecting migrations, repairing a damaged history, baselining an existing
database, and running the migrator from an application binary instead of from a
command line.