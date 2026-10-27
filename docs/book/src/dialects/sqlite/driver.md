# Драйвер `rusqlite`

`SqliteConnection` — адаптер над `::rusqlite::Connection`, реализующий
`QueryBackend` ядра, поэтому хелперы `fetch_all`, `fetch_optional`,
`fetch_one` и `execute` из `QueryBackendExt` доступны прямо на соединении.

## Создание

```rust
use rurqe_sqlite::{rusqlite::Connection, SqliteConnection};

// База в памяти — стенд для тестов и примеров.
let mut db = SqliteConnection::open_in_memory()?;

// Файловая база: соединение открывает драйвер.
let mut file = SqliteConnection::new(Connection::open("app.db")?);
```

`open_in_memory()` возвращает `Result<Self, rusqlite::Error>`, а
`new(connection)` оборачивает уже открытое соединение. Доступ к нему дают
`connection()`, `connection_mut()` и `into_inner()`; кроме того адаптер
реализует `Deref`/`DerefMut` на `Connection`, так что сырой SQL выполняется
напрямую.

## Контракт исполнения

| Метод `QueryBackend` | Поведение |
|---|---|
| `fetch_raw` | готовит запрос, читает все строки и отдаёт `Vec<Vec<SqlVal>>` |
| `fetch_raw_limited` | прекращает чтение, как только прочитано `limit` строк |
| `execute_raw` | выполняет запрос и возвращает число затронутых строк как `u64` |
| `Error` | `::rusqlite::Error` |

Строки за пределы лимита не дочитываются: цикл завершается до следующего
шага курсора, поэтому неограниченный результат не вычитывается целиком.

## Параметры

`SqlVal` транслируется в интерфейс `rusqlite` в `backend/types.rs`:

| `SqlVal` | Привязка |
|---|---|
| `Null` | `ValueRef::Null` |
| `I32`, `I64` | `ValueRef::Integer` |
| `F64` | `ValueRef::Real` |
| `Bool` | `ValueRef::Integer(0 или 1)` |
| `Text` | `ValueRef::Text` |
| `Bytes` | `ValueRef::Blob` |
| `Array` | `ValueRef::Text` — JSON-документ элементов `[…]` |
| `EncodingError` | `ToSqlConversionFailure` с причиной |

Скалярные семейства, компилируемые фичами (`decimal`, `uuid`, `chrono`,
`serde_json`), привязываются своим текстовым представлением — это то значение,
которое крейт разбирает обратно в тот же тип.

## Декодирование

`ValueRef::Integer` → `SqlVal::I64`, `Real` → `F64`, `Blob` → `Bytes`,
`Null` → `Null`. Текстовое значение обязано быть валидным UTF-8: иначе
возвращается `FromSqlConversionFailure` с индексом проблемной колонки, а не
тихая замена символов.

## Транзакции

```rust
let outcome = db.transaction(|tx| -> Result<(), rusqlite::Error> {
    tx.execute_batch("INSERT INTO users (id, name) VALUES (1, 'root')")?;
    Ok(())
});
```

`transaction` коммитит при `Ok` и откатывает при `Err`; повторный вызов внутри
уже открытой транзакции открывает `SAVEPOINT`, поэтому вложенный сбой
откатывает только свои операторы, а внешняя транзакция остаётся целой. Паника
внутри блока тоже приводит к откату. `TransactionBackend::begin` выполняет
`BEGIN`: запрошенный уровень изоляции игнорируется, потому что SQLite
сериализует доступ блокировкой файла базы.

## Пул

`SqlitePool<M: ManageConnection<Connection = ::rusqlite::Connection>>`
оборачивает `r2d2::Pool` и сам реализует `QueryBackend`: каждый вызов берёт
соединение из пула, выполняет запрос и возвращает соединение обратно. Ошибки
пула и запроса различаются вариантами `SqlitePoolError::{Pool, Query}`.

## Покрытие тестами

`crates/rurqe-sqlite/tests/sqlite_exec.rs` прогоняет адаптер на базе в памяти:

* `execute_reports_affected_rows_from_insert`,
  `fetch_all_decodes_ordered_rows_with_parameters`,
  `fetch_one_decodes_a_single_row`,
  `fetch_optional_returns_none_for_absent_rows` — базовый CRUD-контракт;
* `ilike_filters_case_insensitively_through_lower_folding` — `ILIKE` через
  `LOWER(...) LIKE LOWER(...)`;
* `an_array_parameter_binds_as_a_json_document_and_decodes_back` — массив как
  JSON-документ и обратное декодирование;
* `transaction_commits_work_on_success`,
  `transaction_rolls_back_on_error`,
  `nested_transaction_rolls_back_to_its_savepoint`,
  `nested_transaction_releases_its_savepoint_on_success` — транзакции и
  savepoints;
* `typed_execution_errors_are_inspectable_without_boxing` — типизированные
  `ExecutionError` без боксинга.
