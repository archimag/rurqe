# Традиционный путь: `V...` и `R...`

Классический Flyway-совместимый цикл полностью реализован в `rurqe-migrate` и доступен как через CLI, так и как встроенный раннер.

## Именование скриптов

`Migration::parse(script, sql)` разбирает имя и сразу считает `Checksum::of(sql.as_bytes())`.

| Имя | Разбор |
|---|---|
| `V2_1__add_orders.sql` | `Versioned`, версия `2.1`, описание `add orders` |
| `V1.0.0__init.sql` | `Versioned`, версия `1.0.0` (точки и подчёркивания равнозначны) |
| `R__views_and_funcs.sql` | `Repeatable`, версии нет, описание `views and funcs` |

Правила, которые проверяет парсер:

- расширение обязано быть `.sql`;
- для версии — префикс `V`, затем непустая версия, затем `__`;
- для repeatable — префикс `R`, затем сразу `__`;
- подчёркивания в описании заменяются пробелами (так они попадают в историю);
- версия — numeric parts через `.` или `_`; порядок числовой, поэтому `1.2 < 1.10`, а `1 < 1.0`.

Нарушение даёт `Error::InvalidScriptName` или `Error::InvalidVersion` — конкретно то, что нужно исправить.

## Транзакционность

Скрипт транзакционен, пока не сказано обратное. Отключение — двумя равнозначными способами:

1. Имя: `V50__rebuild_index.notx.sql` или `V50__rebuild_index__notx.sql` (суффикс `notx.sql`, регистр не важен).
2. Директива в комментарии, занимающая целую строку:

```sql
-- executeInTransaction = false
CREATE INDEX CONCURRENTLY users_email_idx ON users (email);
```

Поддерживаемые написания ключа: `executeInTransaction`, `execute_in_transaction`, `transactional`; значение `false`. Сканер комментариев (`comment_text`) отслеживает строковые литералы, кавычки-идентификаторы, dollar-quoted тела `$$ … $$` и вложенные блочные комментарии, поэтому:

- `-- transactional = false` внутри тела `plpgsql`-функции — это данные, а не директива;
- директива во вложенном комментарии игнорируется, а во внешнем — работает;
- упоминание переключателя в прозе («set transactional = false here when…») не отключает транзакцию: строка должна быть только директивой;
- незакрытый литерал или комментарий не приводит к панике — сканер остаётся в устойчивом состоянии.

## Применение

`Migrator::migrate` работает так:

1. Берёт run-лок (`LockGuard::acquire`) и гарантирует существование таблицы истории.
2. Проверяет уникальность версий (`DuplicateMigrationVersion`).
3. Читает историю и валидирует её: провалившаяся запись → `FailedMigrationAlreadyApplied`; изменённый версионированный скрипт → `ChecksumMismatch`.
4. Сортирует скрипты: версионированные по возрастанию версии, затем repeatable по имени.
5. Для каждого решает, надо ли применять: версия ниже или равная `baseline` — пропуск; уже записанная — пропуск; repeatable — применять, если контрольная сумма отличается от последней записанной.
6. Применяет и пишет строку истории (`SchemaHistoryEntry::applied`) в той же транзакции.

Порядок деталей применения (`runner/apply.rs`):

| Вид скрипта | Что происходит |
|---|---|
| транзакционный | `begin` → выполнить SQL → записать applied-строку → `commit`; при ошибке `rollback`, best-effort запись failed-строки, `MigrationExecutionFailed` |
| нетранзакционный | сначала коммитится in-progress строка (`success = false`), затем скрипт выполняется вне транзакции |

Нетранзакционный путь имеет три исхода помимо успеха:

- backend отказал до выполнения (`NonTransactionalUnsupported`) → in-progress строка отзывается, в истории ничего не остаётся;
- скрипт упал → in-progress строка остаётся маркером попытки, следующая попытка требует `repair`;
- скрипт применился, но applied-строку записать не удалось → `HistoryNotRecorded`: схема уже изменена, история отстала, нужен `repair` или `baseline`.

## Команды

```bash
cargo db-migrate up       [--migrations-dir <dir>] [--checkpoint-path <path>] \
                          [--database-url <url>] [--schema <name>]
cargo db-migrate status   [--migrations-dir <dir>] [--database-url <url>] [--schema <name>]
cargo db-migrate repair   [--migrations-dir <dir>] [--database-url <url>] [--schema <name>]
cargo db-baseline --version <version> [--description <desc>]
```

- `up` — по умолчанию, если действие не указано. Только эта команда принимает `--checkpoint-path`: она вооружает защиту от устаревшей цели.
- `status` читает историю и печатает таблицу `SCRIPT | KIND | VERSION | STATUS`; `status` на неинициализированной базе сам создаёт таблицу истории, поэтому проверка таблицы и чтение идут под тем же run-локом, что и прогон.
- `repair` удаляет провалившиеся записи и переписывает контрольные суммы изменённых версионированных скриптов.
- `baseline` записывает `SchemaHistoryEntry::baseline` (kind `Baseline`, без контрольной суммы) и отказывается работать, если история уже непуста (`BaselineAlreadyExists`).

## Встроенный раннер

Для запуска из приложения без CLI:

```rust
use rurqe_migrate::{embed_migrations, InMemoryBackend};

let embedded = embed_migrations!(
    "V1__init.sql" => "CREATE TABLE users (id INT);",
    "R__view.sql" => "CREATE VIEW users_v AS SELECT * FROM users;",
).expect("valid migrations");

let mut backend = InMemoryBackend::new();
let report = embedded.migrate(&mut backend).expect("migration succeeds");
assert_eq!(report.applied_count, 2);
```

`EmbeddedMigrations::new` сортирует скрипты тем же `Migrator::sort_migrations`, поэтому встроенный путь и CLI видят одинаковый порядок. Синхронные методы блокируют поток и потому проверяют контекст исполнения через `guard_blocking_entry`: по умолчанию предупреждают, а при фиче `strict-async` отказываются работать на потоке Tokio. Асинхронные обёртки (`migrate_async`, `status_async`, `repair_async`, `baseline_async`, при фиче `tokio`) выполняют ту же работу на `spawn_blocking` и возвращают backend обратно вызывающему.

## Подключение CLI

Эта сборка `rurqe-cli` не линкует TLS-бэкенд, поэтому политика транспорта проверяется до драйвера (`enforce_transport_policy`):

- `sslmode=require` и `verify-ca`/`verify-full` отклоняются на любом хосте;
- удалённый хост принимается только при явном `sslmode=disable`;
- loopback и Unix-сокет принимаются без указания режима.

Это важно для `migrate`, потому что пароль от базы передаётся именно в строке подключения.

Далее: [Stage + Tail](stage_tail.md).
