---
name: aura-rust-enum
description: Use when adding or changing Aura Historia Rust enums with stable machine-readable identifiers, EnumIter, canonical string mappings, persisted enum values, string discriminators, or enum evolution across adapters.
---

# Aura Rust Enum

Use for stable enum identity and enum evolution.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` mapping/serialization and naming rules.
- Load the relevant repository/reader/API/projection skill too when an enum crosses that boundary.

## First classify the enum

Choose one:

1. **Internal enum** — no stable external text needed.
2. **Canonical fieldless enum** — one stable machine identifier should mean the same thing across consumers/adapters.
3. **Data-carrying enum** — variants contain fields but a boundary needs a textual discriminator.
4. **Versioned adapter/wire enum** — representation belongs to a specific storage/API/event schema version.

Do not give every enum a string representation by default.

## Canonical fieldless enum

When inverse textual lookup is needed:

- derive `strum_macros::EnumIter`;
- define the canonical identifier with an explicit exhaustive `as_str()`;
- use `self` for `Copy` enums;
- keep the value allocation-free as `&'static str`;
- keep canonical values unique.

Pattern:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum UserTier {
    Free,
    Pro,
    Ultimate,
}

impl UserTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "FREE",
            Self::Pro => "PRO",
            Self::Ultimate => "ULTIMATE",
        }
    }
}
```

The exhaustive `match` is intentional. Adding/removing/renaming a variant must force a compile-time decision about the canonical identifier.

## Never infer stable identifiers from Rust names

Do not define persistence/wire identity only through:

```rust
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
```

or similar automatic case conversion when the value is a stable contract.

A Rust refactor must not silently rename stored/wire data.

Use explicit `as_str()` for canonical domain identity.

Standard identifiers keep their standard form:

- language: `en`, `de`, ...
- currency: `EUR`, `GBP`, ...
- domain enum identifiers: normally `SCREAMING_SNAKE_CASE`;
- established provider/protocol identifiers keep their canonical contract spelling.

## Inverse lookup

Do not maintain a second string-to-variant table.

At the boundary:

```rust
use strum::IntoEnumIterator;

let tier = UserTier::iter()
    .find(|tier| tier.as_str() == value)
    .ok_or(MappingError::InvalidTier)?;
```

Unknown strings still need an error path because strings are open-world. `EnumIter` removes duplicated known-value matching.

## Persisted values are exact

For Postgres persisted state, do not normalize first:

```rust
value.to_ascii_uppercase()
value.to_ascii_lowercase()
```

Canonical database values should already satisfy schema constraints.

Noncanonical persisted text is invalid persisted state.

Keep the adapter's existing typed error semantics.

## PostgreSQL boundary

For text enum columns:

- keep SQLx row fields as `String` unless a separate design explicitly changes error semantics;
- convert `String` to the domain enum in row/domain mapping;
- report unknown values as invalid persisted state;
- write values with `enum.as_str()`;
- do not derive SQLx persistence traits on domain enums just to remove mapping code.

Existing precedent: `fxrate-postgres` maps `Currency` with `Currency::iter().find(|currency| currency.as_str() == persisted)`.

## Data-carrying enums

Do not directly use `EnumIter` as a string parser for payload variants.

Example:

```rust
enum Payload {
    Existing { id: Id },
    New { id: Id },
}
```

If a boundary stores a discriminator, use a fieldless discriminator enum.

Keep it adapter-local if it is storage-specific. Promote it to core only if it is a real domain concept.

Require exhaustive conversions:

```rust
match payload {
    Payload::Existing { .. } => Kind::Existing,
    Payload::New { .. } => Kind::New,
}
```

Then give `Kind` `EnumIter` + exhaustive `as_str()`, parse the text through `Kind::iter()`, and reconstruct the payload with an exhaustive `match`.

Adding a payload variant must break an exhaustive conversion.

## Versioned DTO enums

Adapter-local serde/event/storage enums are allowed and often preferred.

Keep a surrogate enum when:

- the representation belongs to a specific payload/schema version;
- domain and wire/storage evolution need independence;
- the adapter enum is explicitly converted to/from the domain enum.

Do not collapse versioned DTO enums into domain enums merely to reuse `as_str()`.

## Enum evolution checklist

When adding/removing/renaming a canonical variant or changing its identifier:

- fix the exhaustive `as_str()` mapping;
- inspect all exhaustive domain behavior matches;
- search migrations and PostgreSQL `CHECK` constraints;
- search raw SQL literals;
- search API/event/OpenSearch representations that intentionally share the identifier;
- add a migration before writing a new persisted value;
- define compatibility for existing rows before removing/renaming an identifier;
- update targeted integration tests.

The Rust compiler covers Rust matches. It cannot update SQL text.

## Tests

For canonical enums:

- test expected contract strings where useful;
- test identifiers are unique using `EnumIter`;
- adapter parser tests should iterate all variants rather than duplicate the variant list;
- test one unknown value;
- for persisted state, test a noncanonical casing is rejected when relevant.

Example:

```rust
for expected in UserTier::iter() {
    assert_eq!(Ok(expected), parse_tier(expected.as_str()));
}
```

## Raw SQL

Prefer bind parameters using `enum.as_str()` when practical.

If SQL logic must embed enum literals in `CASE`, `IN`, constraints, or migrations:

- keep the SQL explicit;
- treat it as a separate contract;
- audit it on every enum evolution.

Do not pretend Rust exhaustiveness covers SQL literals.

## Avoid

- duplicate enum→string and string→enum match tables;
- wildcard arms in enum→canonical-string matches;
- generated canonical names coupled to Rust variant spelling;
- case-normalizing persisted values;
- SQLx dependencies in domain crates;
- generic enum codec macros without a demonstrated need;
- adapter-specific storage names leaking into domain unless they are truly canonical;
- `unwrap`, `expect`, or swallowed parse failures.

## Verification

Run the narrowest affected core and adapter checks/tests first, then:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --lib --all-features
cargo depgraph-check check
```

Run targeted real-Postgres integration tests when persistence mappings or constraints change.

Before completion, load `aura-rust-review-architecture`.
