# Пулы соединений

Пул — это тоже бэкенд. Прикладной код не отличает пул от соединения: как только
тип реализует `QueryBackend` или `AsyncQueryBackend`, на нём бесплатно появляются
`fetch_all`, `fetch_optional`, `fetch_one` и `execute` из `*Ext`.

## Асинхронный пул: `PgAsyncPool` (deadpool-postgres)

Feature: `deadpool-postgres`. Является `Clone` и реализует `AsyncQueryBackend`.

```rust
pub struct PgAsyncPool {
    pool: deadpool_postgres::Pool,
}

impl PgAsyncPool {
    pub fn new(pool: deadpool_postgres::Pool) -> Self;
    pub fn pool(&self) -> &deadpool_postgres::Pool;
}
```

Каждый вызов проверяет соединение из пула, ждёт выполнения и возвращает
соединение обратно. `fetch_raw_limited` передаёт лимит в поток строк
`tokio_postgres`, поэтому поиск одной строки останавливается на второй.

```rust
use rurqe::postgres::prelude::*;

let rows = pool
    .fetch_all(&select_from(Users).select(cols!(UsersId)))?;
```

Ошибка пула входит в `PgError::Pool`:

```rust
impl From<deadpool_postgres::PoolError> for PgError {
    fn from(error: deadpool_postgres::PoolError) -> Self {
        PgError::Pool(error)
    }
}
```

поэтому `?` в асинхронном коде пробрасывает и ошибку пула, и ошибку драйвера в
один тип.

## Синхронный пул PostgreSQL: `PgPool<M>` (r2d2)

Feature: `r2d2`. Пул параметризован менеджером соединений `r2d2`.

```rust
pub struct PgPool<M: ManageConnection> {
    pool: r2d2::Pool<M>,
}

impl<M: ManageConnection<Connection = postgres::Client>> PgPool<M> {
    pub fn new(pool: r2d2::Pool<M>) -> Self;
    pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error>;
}
```

Ошибки разделены явно:

```rust
pub enum PgPoolError {
    Pool(r2d2::Error),
    Query(PgError),
}
```

`fetch_raw_limited` передаёт лимит в итератор строк `postgres`
(`query_raw(...).take(limit)`), поэтому семантика та же, что и у `PgConnection`.

## Синхронный пул SQLite: `SqlitePool<M>` (r2d2)

Feature: `r2d2`. Менеджер — любой `r2d2` менеджер над `rusqlite::Connection`.

```rust
pub struct SqlitePool<M: ManageConnection> {
    pool: r2d2::Pool<M>,
}

impl<M: ManageConnection<Connection = ::rusqlite::Connection>> SqlitePool<M> {
    pub fn new(pool: r2d2::Pool<M>) -> Self;
    pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error>;
}
```

```rust
pub enum SqlitePoolError {
    Pool(r2d2::Error),
    Query(::rusqlite::Error),
}
```

Документационный пример крейта собирается и исполняется как doctest при включённой
фиче `r2d2`:

```rust
use r2d2_sqlite::SqliteConnectionManager;

let manager = SqliteConnectionManager::memory();
let pool = r2d2::Pool::builder().max_size(4).build(manager)?;
let _pool = rurqe_sqlite::SqlitePool::new(pool);
```

## Правила выбора

| Сценарий | Адаптер |
|---|---|
| асинхронный сервис на `tokio` | `PgAsyncPool` (`deadpool-postgres`) |
| синхронный воркер / CLI на `postgres` | `PgPool<M>` (`r2d2`) |
| синхронный сервис на SQLite | `SqlitePool<M>` (`r2d2`) |
| одна долгоживущая сессия | `PgConnection` / `PgAsyncConnection` / `SqliteConnection` |

Транзакции оформляются на конкретном соединении (`PgConnection::transaction`),
а не на пуле: пул выдаёт соединение на время вызова, а `TransactionGuard`
удерживает `&mut` на всё время блока.

## Итог

- Пул реализует тот же трейт, что и соединение, — переключение не меняет
  прикладной код.
- Лимит строк доходит до драйвера во всех встроенных адаптерах.
- Ошибки пула и ошибки запроса — разные варианты одного перечисления.
