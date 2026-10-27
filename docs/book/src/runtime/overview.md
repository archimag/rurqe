# Рантайм: обзор

Ядро `rurqe-core` не знает ни одного драйвера. Оно описывает, *что* именно надо
выполнить, а адаптеры крейтов `rurqe-postgres` и `rurqe-sqlite` отвечают за *как*.
Граница между ними зафиксирована набором трейтов в
`crates/rurqe-core/src/query/execution.rs`, и ни один адаптер не переписывает этот
контракт заново.

## Пять трейтов контракта исполнения

| Трейт | Роль | Кто реализует |
|---|---|---|
| `ExecutableQuery` | превращает построенный запрос в `BuiltQuery` (SQL + параметры) | `SelectQuery`, `InsertQuery`, `UpdateQuery`, `DeleteQuery`, `GroupedQuery` |
| `QueryBackend` | синхронное исполнение готового SQL | `PgConnection`, `PgPool`, `SqliteConnection`, `SqlitePool` |
| `QueryBackendExt` | декодирование строк (`fetch_all`, `fetch_optional`, `fetch_one`, `execute`) | автоматически для любого `QueryBackend` |
| `AsyncQueryBackend` | асинхронное исполнение готового SQL | `PgAsyncConnection`, `PgAsyncTransaction`, `PgAsyncPool` |
| `AsyncQueryBackendExt` | те же хелперы для асинхронных бэкендов | автоматически для любого `AsyncQueryBackend` |

`ExecutableQuery` реализован ровно для тех состояний билдера, которые можно
отрендерить: `SelectQuery<…, HasProjection, Lock>` — для любого состояния
блокировки, `InsertQuery` с непустым источником строк (`Values: InsertSource<T>`),
`UpdateQuery` только при `Assignments: AssignmentsPresent` и
`State: ExecutableMutation`, `DeleteQuery` при `State: ExecutableMutation`, и
`GroupedQuery`. Всё, что не проходит эти границы, не компилируется.

## Синхронный бэкенд

```rust
pub trait QueryBackend {
    type Error;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error>;

    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error>;

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error>;
}
```

`fetch_raw_limited` имеет реализацию по умолчанию: она вызывает `fetch_raw` и
обрезает результат. Все три встроенных синхронных адаптера переопределяют метод и
передают лимит в сам драйвер — `postgres` через `query_raw(...).take(limit)`,
`rusqlite` через остановку цикла `rows.next()` — поэтому запрос, от которого нужна
одна строка, не материализует весь набор.

## Асинхронный бэкенд

```rust
pub trait AsyncQueryBackend: Sync {
    type Error: Send;

    fn fetch_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send;

    fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send;

    fn execute_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<u64, Self::Error>> + Send;
}
```

Бэкенд принимается по `&self` и требует `Sync`: соединение разделяется между
задачами, а возвращаемые future имеют явный `+ Send`, поэтому запрос можно
переместить в `tokio::spawn`. Ограничение по лимиту уходит в
`tokio_postgres::RowStream`, который опрашивается построчно и останавливается на
нужном количестве строк.

## Декодирование строк

Хелперы `QueryBackendExt` / `AsyncQueryBackendExt` выполняют одну и ту же политику:

- `fetch_all` — читает все строки и декодирует каждую через
  `FromRow::try_from_row`; ошибка декодирования несёт индекс строки в
  `ExecutionError::RowDecode { row_index, source }`.
- `fetch_optional` — запрашивает **не более двух** строк
  (`fetch_raw_limited(&built, Some(2))`). Ноль строк — `Ok(None)`, одна —
  декодирование, больше — `ExecutionError::MultipleRows { count }` *до* попытки
  декодировать строку.
- `fetch_one` — `fetch_optional(..)?.ok_or(ExecutionError::NoRows)`.
- `execute` — рендер и `execute_raw`, возвращает число затронутых строк.

Таким образом, запрос, который по контракту возвращает одну строку, физически не
может вытянуть весь набор из сети, и ошибка «слишком много строк» возникает без
декодирования лишней строки.

## Ошибка исполнения

```rust
pub enum ExecutionError<E> {
    Build(crate::query::BuildError),
    Backend(E),
    RowDecode { row_index: usize, source: crate::row::RowDecodeError },
    NoRows,
    MultipleRows { count: usize },
}
```

Практическое следствие двухуровневой архитектуры: `ExecutionError::Build`
возникает **до** любого сетевого ввода-вывода — это отказ пре-флайт валидации AST
(`BuildError`), а не ответ сервера. Реализация `From<E> for ExecutionError<E>`
упаковывает драйверную ошибку в `Backend`, поэтому `?` в прикладном коде работает
без ручного маппинга.

Хелперы проверки: `is_build` / `as_build`, `is_not_found`, `is_multiple_rows`,
`is_row_decode` / `as_row_decode`, `as_backend` / `into_backend`.

## Адаптеры

| Адаптер | Крейт | Cargo feature | Контракт |
|---|---|---|---|
| `PgConnection` | `rurqe-postgres` | `postgres` (или `r2d2`) | `QueryBackend`, `TransactionBackend`, `TransactionalConnection` |
| `PgAsyncConnection` | `rurqe-postgres` | `tokio-postgres` | `AsyncQueryBackend` |
| `PgAsyncTransaction` | `rurqe-postgres` | `tokio-postgres` | `AsyncQueryBackend` |
| `PgAsyncPool` | `rurqe-postgres` | `deadpool-postgres` | `AsyncQueryBackend` |
| `PgPool<M>` | `rurqe-postgres` | `r2d2` | `QueryBackend` |
| `SqliteConnection` | `rurqe-sqlite` | `sqlite` | `QueryBackend`, `TransactionBackend`, `TransactionalConnection` |
| `SqlitePool<M>` | `rurqe-sqlite` | `r2d2` | `QueryBackend` |

Диалектная часть (`Postgres`, `Pg14/15/16`, `Sqlite`, `Sqlite3_0 … Sqlite3_38`)
компилируется при любом адаптерном feature, поэтому типизированный запрос можно
собрать и отрендерить, не подключая драйвер.

## Дальше

- [Транзакции и savepoints](transactions.md)
- [Пулы соединений](pools.md)
- [Экосистемные типы](types.md)
- [Пользовательские типы](custom_types.md)
