# Пользовательские типы: newtype-макросы и `#[derive(SqlType)]`

Доменный тип (хэш, адрес, идентификатор предметной области) должен вести себя как
встроенный скаляр: биндиться параметром, декодироваться из строки, участвовать в
сравнениях, попадать в аргументы скалярных функций и подниматься в `Option<Self>`
при внешнем соединении. Есть два пути — декларативные макросы и proc-macro derive.

## Декларативные макросы

`crates/rurqe-core/src/expr/val/newtype.rs` экспортирует три макроса:

```rust
rurqe::impl_sql_binary_newtype!(Txid, 32); // with exact length validation
rurqe::impl_sql_binary_newtype!(Digest);   // any length
rurqe::impl_sql_text_newtype!(Address);    // via Display + FromStr
```

Бинарный вариант требует `AsRef<[u8]>` и `TryFrom<&[u8]>`; текстовый — `Display` и
`FromStr`. Оба пути не заглядывают внутрь типа.

### Что генерируется

Обе формы раскрываются в общий `impl_sql_newtype!`, который выдаёт:

- `ToSqlVal` для `Ty` и для `&Ty`;
- `From<Ty> for SqlVal`;
- `MakeNullable` с `Nullable = Option<Ty>`;
- `FromSqlValue` с проверкой длины (для `impl_sql_binary_newtype!(Ty, N)`);
- `IntoExpr<Ty>` для `Ty` и `&Ty`;
- `SqlCompatible<Ty, &Ty> for &Ty`;
- `FunctionOperand<Ty>` для `Ty` и `&Ty`;
- текстовый вариант дополнительно даёт `SqlCompatible<Ty, &str>`, `IntoExpr<Ty>`
  для `&str` и `String` — так предикат можно писать прямо против литерала.

### Ошибки вместо паники

- неверная длина: `RowDecodeError::InvalidValue { found: "bytes", detail: "expected 32 bytes, got 31" }`;
- отказ `TryFrom`: `InvalidValue` с текстом ошибки конверсии;
- отказ `FromStr`: `InvalidValue` с текстом парсера;
- чуждый вариант `SqlVal`: `TypeMismatch`.

Пример из тестов крейта:

```rust
crate::impl_sql_text_newtype!(MockAddress);

assert_eq!(MockAddress::from_sql_value(&SqlVal::Text("addr_12345".to_owned())),
           Ok(MockAddress("addr_12345".to_owned())));
assert!(MockAddress::from_sql_value(&SqlVal::Text("invalid".to_owned())).is_err());
```

## `#[derive(SqlType)]`

`crates/rurqe-macros/src/sql_type.rs` добавляет к тем же контрактам режим
делегирования и поддержку перечислений.

| Атрибут | Смысл |
|---|---|
| `#[sql(transparent)]` | делегировать обёрнутому значению (`struct UserId(i64)` ведёт себя как `i64`) |
| `#[sql(as = Bytes)]` | кодировать через `AsRef<[u8]>`, декодировать через `TryFrom<&[u8]>` |
| `#[sql(as = Bytes, len = 32)]` | то же плюс проверка длины блоба |
| `#[sql(as = Text)]` | кодировать через `Display`, декодировать через `FromStr` |
| `#[sql(rename = "...")]` | (на варианте enum) SQL-метка, отличная от имени варианта |

Применяется к newtype-структуре ровно с одним полем (кортежным или именованным) и к
перечислению из unit-вариантов.

### Перечисления как типы БД

Перечисление хранится метками вариантов. Derive выдаёт `Display` (рендерит метку)
и `FromStr` (разбирает метку обратно), а неизвестная метка становится
`RowDecodeError::InvalidValue` с перечислением допустимых значений. `#[sql(as = Text)]`
разрешён как явное указание режима по умолчанию; любой другой режим для enum
отвергается.

```rust
#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    #[sql(rename = "active")]
    Active,
    Blocked,
}
```

### Что генерируется

Каждый режим выдаёт `ToSqlVal`, `From<Self> for SqlVal`, `FromSqlValue`,
`MakeNullable`, `SqlArrayElement` (тип можно использовать как элемент массива
`Vec<Self>`), совместимость с выражениями (`IntoExpr`) и с аргументами скалярных
функций (`FunctionOperand`). Для режима `Bytes` и `Text` добавляются заимствованные
реализации.

### Пути и зависимости

Каждый путь в раскрытии привязан к `::rurqe::core` — ре-экспорту фасада. Крейт,
зависящий только от `rurqe`, компилирует `#[derive(SqlType)]`, `select!`, `dto!`,
`sql!`/`raw_sql!` без добавления `rurqe-core` в свой манифест.

### Диагностика derive

| Ситуация | Сообщение |
|---|---|
| union | `SqlType` derives only on a newtype struct or on an enum of unit variants |
| структура не с одним полем | `SqlType` derives only on a newtype struct with exactly one field |
| нет ни одного режима | `SqlType` needs a storage option: `#[sql(transparent)]`, `#[sql(as = Bytes)]`, or `#[sql(as = Text)]` |
| `transparent` вместе с `as` | `transparent` cannot be combined with `as = …` |
| `len` без `Bytes` | `len` requires `as = Bytes` |
| неизвестный `as` | unsupported `sql` storage; expected `Bytes` or `Text` |
| повтор атрибута | `as` is given more than once / `len` is given more than once / `rename` is given more than once |
| `transparent` на enum | `transparent` is not supported on an enum; an enum is stored as its label |
| пустой enum | `SqlType` derives on an enum with at least one variant |
| enum с полями у варианта | `SqlType` derives only on an enum of unit variants |

## Кодогенерация доменных типов

`rurqe.toml` позволяет закрепить доменный тип за колонкой, чтобы он пережил
кодогенерацию, а не деградировал до `String`/`Vec<u8>`:

```toml
[codegen.type_overrides]
"users.id" = "uuid::Uuid"
users.created_at = "chrono::NaiveDateTime"
```

Override попадает в маркер колонки и в поле записи дословно; если значение уже
содержит `Option<…>`, повторная обёртка не добавляется. Подробнее — в главе
[CLI, кодогенерация и drift](../toolchain/overview.md).

## Итог

- Два пути, один контракт: и макрос, и derive дают полный набор реализаций.
- Валидация (длина, парсинг) всегда возвращает типизированную ошибку
  декодирования, а не паникует.
- Доменный тип участвует в выражениях, функциях, массивах и внешних соединениях
  на равных со встроенными скалярами.
