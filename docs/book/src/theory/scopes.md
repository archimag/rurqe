# Области видимости: TableScope и ExprScope

Реляционная алгебра требует, чтобы каждый атрибут был привязан к отношению, а каждое отношение — присутствовать в выражении. В RURQE это две независимые системы трейтов: одна описывает отношения запроса, другая — область, на которую имеет право ссылаться выражение.

## TableScope — отношения запроса

```rust
pub trait TableScope {
    fn push_table_names(out: &mut Vec<&'static str>);
    fn push_table_refs(out: &mut Vec<TableRef>) { /* по умолчанию из имён */ }
}
```

- Одиночная таблица реализует `TableScope` тривиально.
- Каждый тип соединения реализует его, добавляя своё отношение в конец:
  `Joined<Tables, T>`, `LeftJoined<Tables, T>`, `CrossJoined<Tables, T>`, `FullOuterJoined<Tables, T>`, `CrossLateralJoined<Tables, T>`, `LeftLateralJoined<Tables, T>`.
- `Compound` реализует `TableScope`, не добавляя ни одного отношения: комбинированный запрос адресует только выходные колонки своей проекции, а не хранимые таблицы.

Именно поэтому `TableInScope` не имеет реализации для `Compound`: ссылка на таблицу в комбинированном запросе структурно недостижима.

## TableInScope — позиция отношения

```rust
pub trait TableInScope<T: Table, P = At<0>> {}
```

- Таблица тривиально находится в области самой себя: `impl<T: Table> TableInScope<T, At<0>> for T`.
- Соединённая область добавляет новое отношение на позиции `At<0>` и сдвигает все прочие на `Next<P>`:

```rust
impl<Tables, T: Table> TableInScope<T, At<0>> for Joined<Tables, T> {}
impl<Tables, T: Table, Target: Table, P> TableInScope<Target, Next<P>> for Joined<Tables, T>
where Tables: TableInScope<Target, P> {}
```

Позиции `At` / `Next` нужны только для однозначного вывода: одна и та же таблица не может встретиться в области дважды под одним именем, и компилятору нужен маркер, различающий вхождения.

## ExprScope и ExprInScope — область выражения

Выражение несёт свою область как фантомный параметр: `Expr<T, Scope, Aggr, D>`. `Scope` — это `()` для выражения без колонок, одиночная таблица, кортеж таблиц или тип соединённой области.

```rust
pub trait ExprScope {}
pub trait ExprInScope<Tables, Positions = At<0>> {}
```

Реализации `ExprInScope`:

| `Self` | Условие |
|---|---|
| `()` | всегда — выражение не ссылается на колонки |
| `Unscoped` | всегда — область снята намеренно, проверку выполняет объемлющий запрос |
| `T: Table` | `Tables: TableInScope<T, P>` |
| `(T1, T2, ...)` | каждая `Tables: TableInScope<Ti, Pi>` |

`Unscoped` отличается от `()` намеренно: он говорит «это выражение ссылается на таблицы, но проверку делает тот, кто его вложил», что нужно для подзапросов и `CAST`-подобных конструкций.

## ScopePositions — вывод позиций

```rust
pub trait ScopePositions<P = At<0>> {}
```

Пустая область всегда стоит на `At<0>`, одиночная таблица — на своей позиции, кортеж — по одной позиции на элемент. Без этого трейта `P` в `ExprInScope<Tables, P>` оставался бы неоднозначным и давал бы `E0282`.

## Как это выглядит в использовании

```rust
// Компилируется: Order в области после соединения.
let ok = select_from::<Postgres, User>()
    .inner_join_table(Order, Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
    .select((UserId, OrderTotal));
```

```rust
// Не компилируется: OrderTotal принадлежит Order, которого нет в FROM.
let bad = select_from::<Postgres, User>().select((UserId, OrderTotal));
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

## Область и внешние соединения

Область отвечает не только на вопрос «есть ли отношение», но и на вопрос «какова его nullability». Это делает второй трейт:

```rust
pub trait TableNullabilityInScope<T: Table, P = At<0>> {
    type Nullability;
}
```

- Базовая таблица и `Joined` / `CrossJoined` / `CrossLateralJoined` → `NonNullableRel`.
- `LeftJoined` / `LeftLateralJoined` → `NullableRel` для своего отношения, `NonNullableRel`-наследование для внутренних.
- `FullOuterJoined` → `NullableRel` для **всех** отношений области.

Как это превращается в `Option<T>`, разобрано в главе [Nullability](nullability.md).

## Область и DML

Для мутаций область задаётся иначе: `UPDATE ... FROM` и `DELETE ... USING` расширяют её через `ExtendScope`, а `RETURNING` ограничен `TableSelection<T>` — можно вернуть только колонки мутируемой таблицы. Сами методы `using` / `from` доступны лишь на диалектах с `SupportsDeleteUsing` / `SupportsUpdateFrom`.

## Итог

| Вопрос | Трейт |
|---|---|
| Какие отношения есть в запросе? | `TableScope` |
| Присутствует ли конкретное отношение? | `TableInScope<T, P>` |
| Допустима ли область выражения? | `ExprScope` |
| Все ли таблицы выражения присутствуют в запросе? | `ExprInScope<Tables, P>`, `ScopePositions<P>` |
| Какова nullability отношения в этой области? | `TableNullabilityInScope<T, P>` |
| Какие таблицы добавляет мутация? | `ExtendScope<S>` |
