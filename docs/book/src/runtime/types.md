# Экосистемные типы, значения и декодирование

Все значения проходят через один тип — `SqlVal` из
`crates/rurqe-core/src/expr/val.rs`. Он же — единица обмена между ядром и
драйверами: параметры уходят в адаптер как `Vec<SqlVal>`, строки возвращаются как
`Vec<Vec<SqlVal>>`.

## `SqlVal`

```rust
pub enum SqlVal {
    Null,
    I32(i32),
    I64(i64),
    F64(f64),
    Decimal(rust_decimal::Decimal),  // feature = "decimal"
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<SqlVal>),
    Uuid(uuid::Uuid),                // feature = "uuid"
    NaiveDateTime(chrono::NaiveDateTime), // feature = "chrono"
    DateTimeUtc(chrono::DateTime<chrono::Utc>), // feature = "chrono"
    NaiveDate(chrono::NaiveDate),    // feature = "chrono"
    Json(serde_json::Value),         // feature = "serde_json"
    EncodingError(String),
}
```

Перечисление помечено `#[non_exhaustive]`, потому что часть вариантов существует
только при включённой Cargo-фиче. Следствие для прикладного кода: `match` по
`SqlVal` вне крейта обязан иметь ветку с подстановочным шаблоном — она остаётся
корректной при любом наборе фич.

`SqlVal::EncodingError` появился не для красоты: если значение нельзя закодировать
(например, `usize`, не влезающий в `i64`), параметр не подменяется на `NULL`, а
несёт причину отказа — драйвер вернёт её вызывающему коду.

## Кодирование параметров: `ToSqlVal`

Реализации предоставлены для `i16`, `i32`, `u32`, `i64`, `usize`, `f64`, `bool`,
`String`, `&str`, `&String`, `Vec<u8>`, `&[u8]`, `SqlVal`, `Option<T>`, а также для
гейтед-семейств: `rust_decimal::Decimal` и `&Decimal`, `uuid::Uuid` и `&Uuid`,
`chrono::NaiveDateTime`, `chrono::DateTime<Utc>`, `chrono::NaiveDate`,
`serde_json::Value`, `Json<T>` и `&Json<T>`.

Особенности, зафиксированные кодом:

- `i16` кодируется как `SqlVal::I32`, `u32` — как `SqlVal::I64`;
- `usize` пытается стать `i64`, иначе даёт `EncodingError` с текстом
  «the value … cannot be represented as a 64-bit signed integer»;
- `Option<T>::None` — это `SqlVal::Null`;
- `Json<T>` сериализуется через `serde`; отказ сериализации превращается в
  `EncodingError`, а не в панику.

## Декодирование значений: `FromSqlValue`

Каждая реализация `FromSqlValue` парная к `ToSqlVal`, поэтому значение
round-trip’ится из параметра в строку результата без промежуточного текста.

Декодирование намеренно терпимо там, где движок не имеет отдельного класса
хранения:

| Целевой тип | Принимает |
|---|---|
| `i16`, `i32`, `u32`, `usize` | `I32` и `I64` в пределах диапазона |
| `i64` | `I64`; `Decimal` (если влезает); `F64` без дробной части |
| `f64` | `F64`, `I32`, `I64`, `Decimal` |
| `bool` | `Bool`, а также целые `0` / `1` |
| `String` | `Text`, а также текстовые формы `Uuid`, временных типов, `Json`, `Decimal` |
| `Vec<u8>` | `Bytes`, а также `Text` в шестнадцатеричном виде (префикс `\x` допускается, регистр не важен) |
| `Decimal` | `Decimal`, `I32`, `I64`, `F64`, `Text` |
| `uuid::Uuid` | `Uuid`, `Text`, `Bytes` |
| `serde_json::Value`, `Json<T>` | `Json`, `Text`, `Bytes` |
| `Option<T>` | `Null` → `None`, иначе `T::from_sql_value` |

Выход за диапазон — это `RowDecodeError::InvalidValue`, а не тихое усечение: `i64`
не примет `F64(0.5)`, `NaN`, бесконечность или `1e30`. `String` принимает текстовую
форму «доменного» значения именно потому, что кодогенератор по умолчанию
отображает такие колонки на `String`.

Шестнадцатеричный обмен для бинарных значений (`hex_encode` / `hex_decode`)
существует ради вложенных JSON-документов: бинарное поле вложенной строки
приходит текстом и декодируется обратно в байты.

## Ошибка декодирования

```rust
pub enum RowDecodeError {
    TypeMismatch { target: &'static str, found: &'static str },
    InvalidValue { target: &'static str, found: &'static str, detail: String },
    ArityMismatch { expected: usize, actual: usize },
}
```

`target` — `std::any::type_name` целевого типа, `found` — `SqlVal::variant_name()`.
Отображение даёт читаемые сообщения: «cannot decode a SQL `text` value into `i64`»,
«cannot decode a row of 1 values into a row type of 2 values».

## Строки: `FromRow`

`FromRow` реализован для кортежей от 1 до 12 элементов. `try_from_row` проверяет
арность **до** декодирования значений, поэтому несоответствие числа колонок не
маскируется ошибкой типа. `from_row` — сокращение, возвращающее `Option`.

```rust
let row = <(i64, String)>::try_from_row(&[
    SqlVal::I64(1),
    SqlVal::Text("root".to_owned()),
]);
assert_eq!(row, Ok((1, "root".to_owned())));
```

Сгенерированный `FromRow` в `schema.rs` никогда не индексирует срез напрямую:
он читает через `values.get(i)` и сообщает `RowDecodeError::ArityMismatch` на
короткий ответ.

## Подстановка ошибки декодирования в исполнение

`ExecutionError::RowDecode { row_index, source }` несёт индекс первой строки,
которая не декодировалась. Индекс нулевой, если запрос запрашивал одну строку
через `fetch_optional`/`fetch_one`: там декодируется максимум одна строка.

## Что происходит на стороне PostgreSQL

Адаптер кодирует параметр в бинарное представление **типа колонки**, а не типа
значения:

- цепочка доменов раскрывается (`base_type`), поэтому домен-колонка получает ту же
  раскладку, что и тип, над которым он объявлен;
- целочисленные значения сужаются до `INT2`/`INT4`, если колонка такой ширины;
  выход за диапазон — ошибка кодирования;
- `NUMERIC` кодируется точно, через текстовую форму десятичного значения, а не
  через `f64`;
- массивы кодируются поэлементно кодеком элемента, `NULL`-элемент остаётся
  `NULL`;
- колонки без отдельного варианта `SqlVal` (UUID, JSON/JSONB, временные типы при
  выключенных фичах) получают текстовую форму, которую сервер разбирает сам;
- `NULL` биндится для колонки любого типа.

При декодировании набор правил классифицируется один раз на колонку и
переиспользуется для всех строк результата; полезная нагрузка читается из
заимствованного буфера строки без промежуточной копии.

## Что происходит на стороне SQLite

`rusqlite` не имеет отдельного класса для boolean и узких целых: значения
приходят как `I64`/`F64`/`Text`/`Blob`/`Null`. Поэтому `bool` принимает `0`/`1`, а
`Vec<T>` кодируется как JSON-документ (см. [Массивы в SQLite](../dialects/sqlite/overview.md)).
Текстовая колонка обязана быть валидным UTF-8: иначе возвращается ошибка
конвертации для конкретной колонки, и значение не подменяется символами замены.
