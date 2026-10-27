# Транзакции и savepoints

Синхронные адаптеры (`PgConnection`, `SqliteConnection`) не реализуют транзакции
каждый по-своему. Политика `BEGIN` / `SAVEPOINT` / `ROLLBACK` живёт в одном месте —
`TransactionGuard` из `crates/rurqe-core/src/query/transaction.rs`, — а адаптер
предоставляет только учёт состояния через `TransactionalConnection`.

## Учёт состояния: `TransactionStatus`

```rust
pub struct TransactionStatus {
    context: TransactionContext,
    open: bool,
    poisoned: bool,
}
```

- `is_open()` — открыта ли внешняя транзакция;
- `is_poisoned()` — остался ли `ROLLBACK`, который драйвер отказался выполнить;
- `depth()` — число открытых savepoint’ов;
- `current_savepoint()` — имя самого внутреннего savepoint’а.

`TransactionContext` нумерует вложенные блоки: имена генерируются как
`rurqe_sp_1`, `rurqe_sp_2`, … через `SavepointName::generated`, а `SavepointName`
принимает только `[A-Za-z0-9_]`, поэтому имя никогда не может внести SQL в
отрендеренный оператор.

## Открытие блока

```rust
pub fn begin(connection: &'a mut B) -> Result<Self, B::Error>;
pub fn begin_with(
    connection: &'a mut B,
    isolation: Option<IsolationLevel>,
) -> Result<Self, B::Error>;
```

Логика `begin_with`:

1. `recover(connection)` — если соединение отравлено, повторяется брошенный
   `ROLLBACK`. Успех очищает состояние, повторный отказ драйвера возвращается
   вызывающему коду как ошибка: такое соединение надо выбрасывать, а не
   переиспользовать.
2. Если транзакция уже открыта — открывается `SAVEPOINT`, guard помечается как
   вложенный (`outer: false`). Ошибка `SAVEPOINT` снимает имя из учёта и
   возвращается наружу.
3. Иначе вызывается `TransactionBackend::begin(isolation)`, состояние помечается
   открытым, guard становится внешним (`outer: true`).

## Рендеримые операторы

| Действие | Внешний блок | Вложенный блок |
|---|---|---|
| открытие | `BEGIN` | `SAVEPOINT rurqe_sp_N` |
| открытие с уровнем | `BEGIN ISOLATION LEVEL <LEVEL>` | `SAVEPOINT rurqe_sp_N` (уровень игнорируется) |
| `commit()` | `COMMIT` | `RELEASE SAVEPOINT rurqe_sp_N` |
| `Drop` без `commit()` | `ROLLBACK` | `ROLLBACK TO SAVEPOINT rurqe_sp_N`, затем `RELEASE SAVEPOINT rurqe_sp_N` |

`IsolationLevel::as_str` возвращает ровно `READ COMMITTED`, `REPEATABLE READ` или
`SERIALIZABLE`. По умолчанию `TransactionBackend::begin` собирает строку
`BEGIN` и дописывает ` ISOLATION LEVEL …` только при заданном уровне.

## Уровень изоляции принадлежит только внешней транзакции

```rust
pub enum IsolationLevel {
    ReadCommitted,
    RepeatableRead,
    Serializable,
}
```

SQL не определяет изоляцию на уровне savepoint’а, поэтому `begin_with`,
вызванный внутри уже открытой транзакции, создаёт обычный `SAVEPOINT` и
игнорирует запрошенный уровень: внешняя транзакция сохраняет свой. Это
зафиксировано тестом `a_nested_block_ignores_the_requested_isolation_level`,
который проверяет, что в списке операторов нет ни одного вхождения
`ISOLATION LEVEL`.

## Откат, коммит и отравление соединения

`commit(self)` вызывает `release()`, затем помечает guard завершённым.
Для внешнего блока `release` делает `COMMIT` и закрывает состояние; для
вложенного — `RELEASE SAVEPOINT` и уменьшает глубину. `Drop` завершённого guard’а
ничего не делает.

Если guard уничтожается без `commit()`:

- внешний блок — `ROLLBACK`; при успехе состояние закрывается и сбрасывается
  вложенность, при отказе драйвера соединение **отравляется**
  (`mark_poisoned`): работа блока может быть ещё не отменена, поэтому
  зафиксировать транзакцию закрытой нельзя;
- вложенный блок — `ROLLBACK TO SAVEPOINT` + `RELEASE SAVEPOINT`; если откат
  не удался, глубина сохраняется и соединение тоже отравляется.

Отравленное соединение — это не «закрытая транзакция», а явно неизвестное
состояние: следующий `begin` сначала пытается добить откат через `recover`.
Так исключается запуск новой транзакции поверх незакрытой.

## Наблюдаемость

События guard’а эмитируются в фасад `tracing` под
`#[cfg(feature = "tracing")]`: `debug!` на создание и освобождение savepoint’а,
`info!` на `BEGIN`/`COMMIT`, `warn!` на откат и на успешное восстановление
после неудавшегося отката, `error!` на повторный отказ. Без включённой фичи
`rurqe-core` не тянет `tracing` в граф зависимостей.

## Эргономичные обёртки

`PgConnection`:

```rust
pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
where
    F: FnOnce(&mut PgConnection) -> Result<T, E>,
    E: From<PgError>;

pub fn transaction_with_isolation<T, E, F>(
    &mut self,
    isolation: IsolationLevel,
    f: F,
) -> Result<T, E>
where
    F: FnOnce(&mut PgConnection) -> Result<T, E>,
    E: From<PgError>;
```

`SqliteConnection::transaction` имеет ту же форму с `E: From<rusqlite::Error>`.
Адаптер SQLite переопределяет `TransactionBackend::begin` и выполняет
`execute_batch("BEGIN")`, игнорируя уровень изоляции: SQLite сериализует доступ к
базе блокировкой файла, и уровень изоляции не отображается на отдельный оператор.
Это осознанное решение, а не потеря функциональности — см.
[Конкурентность миграций](../dialects/sqlite/concurrency.md).

Асинхронный драйвер опирается на `tokio_postgres::Transaction`: `commit()` и
`rollback()` вызываются явно, а `Drop` без коммита откатывает транзакцию
средствами драйвера, поэтому отменённая future или паника не оставляют
незавершённую работу.

## Гарантии, которые даёт дизайн

| Инвариант | Где закреплён |
|---|---|
| Паника в блоке откатывает транзакцию | `Drop for TransactionGuard` |
| Отказ драйвера в `ROLLBACK` не маскируется под успех | `mark_poisoned` + `recover` |
| Имя savepoint’а не может содержать SQL | `SavepointName::new` (только `[A-Za-z0-9_]`) |
| Вложенный блок не меняет уровень изоляции | `begin_with` + тест в `transaction.rs` |
| Уровень изоляции доступен только внешнему `BEGIN` | `TransactionBackend::begin` |
