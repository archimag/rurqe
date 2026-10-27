# Эволюция схемы: обзор

Сопровождение схемы — отдельный жизненный цикл, не привязанный к остальной библиотеке: код запросов (крейты `rurqe-core`, `rurqe-postgres`, `rurqe-sqlite`) не знает о миграциях и не импортирует их. Реализация жизни схемы собрана в двух местах:

- `rurqe-migrate` — модель скриптов (`Migration`, `MigrationKind`, `MigrationVersion`, `Checksum`), таблица истории (`SchemaHistoryEntry`, `SCHEMA_HISTORY_TABLE`), раннер (`Migrator`, `MigrationBackend`, `MigrationTransaction`, `LockGuard`), декларативный stage (`DeclarativeStage`, `StageScript`, `StageMarker`, `cold_start`) и маркер границы (Checkpoint`).
- `rurqe-cli` — команды (`migrate`, `baseline`, `codegen`, `verify`, `stage init|verify|commit|cleanup`, `prune`) и протокол артефактов чекпоинта: сериализующая секция `checkpoint.lock`, аварийно-безопасная установка пары «маркер + снимок границы» и восстановление прерванного коммита.

Эти два слоя связаны единственным форматом — снимком каталога (`SchemaCatalog` → `schema.lock` и `checkpoint.toml`), а не общим кодом.

## Два режима одного раннера

Раннер истории один и тот же: `Migrator` читает `__rurqe_schema_history`, валидирует контрольные суммы и применяет хвост. Второй режим — не другой раннер, а другой способ создать базу, которую этот раннер затем продвигает.

| Свойство | Традиционный (`migrations/`) | Stage + Tail (`schema/` + `migrations/`) |
|---|---|---|
| Источник истины | упорядоченный набор скриптов | декларативный stage, плюс активный хвост |
| Инициализация чистой базы | прогон всех `V...` | `stage init` → `cold_start` из `schema/` |
| Продвижение живой базы | `migrate up` | `migrate up --checkpoint-path checkpoint.toml` |
| Удаление старых миграций | сквош через `baseline` | `prune --before <version>` без сквоша |
| Проверка целостности | `db-verify` (drift к `schema.lock`) | `stage verify` (Golden Equation) |
| След в истории | одна строка на скрипт | одна запись `<< Stage >>` + строки хвоста |

Обе модели могут сосуществовать в одном проекте: `cold_start` записывает маркер kind `Baseline`, поэтому все версии `V...` не выше версии маркера считаются уже применёнными (в `status` — `IgnoredBaseline`), а активный хвост выше маркера продолжает применяться обычным `migrate up`.

## Состав раздела

- [Кризис традиционных миграций в эпоху AI](problem.md) — почему линейная история скриптов перестала масштабироваться.
- [Два режима и критерии выбора](modes.md) — что общего и чем именно режимы отличаются.
- [Традиционный путь: `V...` и `R...`](traditional.md) — именование, контрольные суммы, транзакционность, `migrate`/`baseline`/`repair`.
- [Stage + Tail: декларативный stage и cold start](stage_tail.md) — `schema/*.sql`, `StageMarker`, мгновенная инициализация, коммит границы.
- [The Golden Equation](golden_equation.md) — сверка `State(schema/)` и `State(Checkpoint + tail)`.
- [Безопасный прунинг истории](pruning.md) — `prune --before`, защита от устаревшей цели.
- [Протокол чекпоинтов](checkpoint_protocol.md) — `checkpoint.toml`, `checkpoint.lock`, owner-запись, восстановление.

## Общие примитивы

| Примитив | Где объявлен | Роль |
|---|---|---|
| `Migration` | `rurqe-migrate/src/migration.rs` | разобранный скрипт: вид, версия, описание, SHA-256, признак транзакционности |
| `MigrationVersion` | `rurqe-migrate/src/version.rs` | версия как `Vec<u64>`, тотальный числовой порядок |
| `Checksum` | `rurqe-migrate/src/checksum.rs` | SHA-256 в hex, сравнение записанного и фактического |
| `SchemaHistoryEntry` | `rurqe-migrate/src/history.rs` | строка `__rurqe_schema_history` |
| `MigrationBackend` / `MigrationTransaction` | `rurqe-migrate/src/runner/backend.rs` | контракты драйвера: история, лок, транзакции |
| `LockGuard` | `rurqe-migrate/src/runner/guard.rs` | RAII-удержание run-лока |
| `DeclarativeStage` / `StageScript` | `rurqe-migrate/src/stage/script.rs` | декларативный `schema/` |
| `StageMarker` | `rurqe-migrate/src/stage/marker.rs` | синтетическая запись маркера stage |
| `Checkpoint` | `rurqe-migrate/src/checkpoint.rs` | граница прунинга и хеш stage на ней |
| `GoldenEquation` / `GoldenReport` | `rurqe-cli/src/stage/equation.rs` | сверка stage против чекпоинта и хвоста |

## Таблица истории

Обе модели пишут в одну таблицу, создаваемую `create_schema_history_table_sql()`:

```sql
CREATE TABLE IF NOT EXISTS __rurqe_schema_history (
    installed_rank BIGINT NOT NULL PRIMARY KEY,
    version VARCHAR(50),
    description VARCHAR(200) NOT NULL,
    type VARCHAR(20) NOT NULL,
    script VARCHAR(1000) NOT NULL,
    checksum VARCHAR(64),
    installed_on VARCHAR(50) NOT NULL,
    execution_time BIGINT NOT NULL,
    success BOOLEAN NOT NULL
);
```

Поле `type` хранит текст `MigrationKind::as_str()`: `VERSIONED`, `REPEATABLE` или `BASELINE`. Поле `success` отличает применённую запись от маркера начатой или провалившейся попытки, а отсутствие `checksum` (`baseline`-запись) означает, что сверять нечего и запись никогда не считается изменённой (`SchemaHistoryEntry::is_modified`).

## Как читать раздел

Все утверждения раздела выведены из исходников `crates/rurqe-migrate` и `crates/rurqe-cli/src/{cli,stage}` и из их модульных тестов. Отдельного ключа, включающего режим, в `rurqe.toml` нет: `CodegenConfig::parse` отклоняет любую неизвестную секцию и любой неизвестный ключ, а знает ровно `schema`, `[codegen]`, `[codegen].domain_types` и `[codegen].type_overrides`. Режим определяется составом каталогов и набором команд, а не флагом конфигурации.
