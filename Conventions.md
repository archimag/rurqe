# Project Conventions

## 0. Language & Tone
- All source code, identifiers, comments, documentation, and Git commit messages MUST be strictly in English.

## 1. Architectural Philosophy & Minimalism
- Strictly adhere to YAGNI (You Aren't Gonna Need It). Implement only what is required by current user goals.
- Prefer small, cohesive, deterministic modules over speculative abstractions.
- Do not invent unused configuration options, speculative features, or dead code.

## 2. Git Safety & Commits
- Formulate Git commit summaries in the imperative mood using Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`).
- Keep commits atomic and self-contained.

## 3. Verification & Quality Gates
- Ensure all changes compile cleanly and pass project build, test, and lint toolchains without warnings or regressions.

## 4. Documentation & AI Context Integrity (`llms.txt`)
- Continuously maintain `llms.txt` in an up-to-date, comprehensive, and accurate state whenever library APIs, query builders, DML, migrations, CLI tools, or driver configurations evolve.
- `llms.txt` must contain all necessary and practical information required to understand and effectively use the RURQE library: architectural patterns, fluent DSL syntax, schema definition, type safety rules, execution backends, transactions, connection pooling, and CLI commands.
