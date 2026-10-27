# Nullability: внешние соединения как изменение типа

В реляционной алгебре внешнее соединение — это оператор, добавляющий кортежи с отсутствующими атрибутами. В SQL отсутствие выражается как `NULL`. В RURQE это изменение типа, вычисляемое компилятором.

## Три ортогональных трейта

```rust
/// Маркер: колонки этого отношения в данной области никогда не NULL.
pub struct NonNullableRel;
/// Маркер: колонки этого отношения в данной области могут быть NULL.
pub struct NullableRel;

/// Какова nullability отношения T в области Tables?
pub trait TableNullabilityInScope<T: Table, P = At<0>> { type Nullability; }

/// Как выглядит не-nullable тип T, если его поднять в Option?
pub trait MakeNullable { type Nullable; }

/// Применить маркер nullability N к типу T.
pub trait ApplyNullability<N> { type Output; }
```

Разделение намеренное: первые два описывают **контекст** (как отношение попало в запрос), третий — **тип значения** (как конкретный Rust-тип поднимается в nullable-форму).

## Реализации

`ApplyNullability`:

| Маркер | Результат |
|---|---|
| `NonNullableRel` | `Output = T` (тождество) |
| `NullableRel` | `Output = T::Nullable` |

`MakeNullable` реализован для `i16`, `i32`, `u32`, `i64`, `usize`, `f64`, `bool`, `String`, `Vec<u8>`, а также (под фичами) для `uuid::Uuid`, `chrono::NaiveDateTime`, `chrono::DateTime<Utc>`, `chrono::NaiveDate`, `serde_json::Value`, `crate::expr::Json<T>` и для доменных newtype-типов из `#[derive(SqlType)]`.

Отдельно и важно:

```rust
impl<T> MakeNullable for Option<T> { type Nullable = Option<T>; }
```

Двойного `Option<Option<T>>` не бывает: nullable-колонка остаётся одним слоем `Option`.

`TableNullabilityInScope` по типам областей:

| Область | Nullability своего отношения | Nullability внутренних отношений |
|---|---|---|
| сама таблица `T` | `NonNullableRel` | — |
| `Joined<Tables, T>` | `NonNullableRel` | наследуется |
| `CrossJoined<Tables, T>` | `NonNullableRel` | наследуется |
| `CrossLateralJoined<Tables, T>` | `NonNullableRel` | наследуется |
| `LeftJoined<Tables, T>` | `NullableRel` | наследуется |
| `LeftLateralJoined<Tables, T>` | `NullableRel` | наследуется |
| `FullOuterJoined<Tables, T>` | `NullableRel` | `NullableRel` для всех |

## Как из этого получается тип строки

```rust
pub trait ColumnInScope<Tables, P> { type Output; }

impl<Tables, C, P> ColumnInScope<Tables, P> for C
where
    C: Column,
    Tables: TableNullabilityInScope<C::Table, P>,
    C::RustType: ApplyNullability<<Tables as TableNullabilityInScope<C::Table, P>>::Nullability>,
{
    type Output = <C::RustType as ApplyNullability<...>>::Output;
}
```

Дальше `SelectionFor<Tables, Positions, D>::Row` собирает кортеж из `Output` каждого элемента проекции. Никакой ручной аннотации не требуется.

## Пример

```rust
let query = SelectQuery::<MockDialect>::new()
    .from::<User>()
    .left_join::<Order, _, _>(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
    .select((UserId, OrderTotal));

// UserId остаётся i64, OrderTotal становится Option<f64>.
let _: SelectQuery<MockDialect, _, (i64, Option<f64>), HasProjection> = query;
```

Чтение той же колонки как `f64` не компилируется:

```text
error[E0277]: `{...}` is not a supported selection
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
```

## Ручной маркер `Nullable<C>`

Иногда nullable-форма нужна без внешнего соединения — например, для согласования формы с уже посчитанным результатом. Для этого есть обёртка:

```rust
impl<C: Column> Column for Nullable<C> {
    type Table = C::Table;
    type RustType = Option<C::RustType>;
    const NAME: &'static str = C::NAME;
}
```

Это тот же столбец, но в типе значения один слой `Option`.

## Почему это не может быть рантайм-проверкой

`NULL` из внешнего соединения — не «особый случай данных», а следствие оператора. Если тип утверждает `T`, а значение приходит `NULL`, это дефект программы, который обязан быть замечен до выполнения. Поэтому nullability вычисляется из **типа области**, а не из содержимого строки: маркер `NullableRel` проставляется конструктором соединения и дальше только наследуется.

Обратное тоже верно: внутренние соединения не поднимают ничего (`NonNullableRel`), поэтому чтение `Option<T>` из `INNER JOIN` — это выбор автора, а не вынужденная мера.

## Связанные главы

- [Области видимости](scopes.md) — как устроены `Joined` и родственные типы.
- [Кодд → Rust](codd_to_rust.md) — место внешних соединений в общей таблице операторов.
- Декодирование строк и `Option<T>` в `FromSqlValue` — глава о рантайме (отдельный раздел книги).
