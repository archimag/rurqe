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
