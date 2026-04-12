# Lesson 02 — Structs, Traits & Shared Types (`bonk-common`)

## What you will build

`crates/bonk-common/src/lib.rs` — the shared type library used by both `bonk-cli` (the build tool) and `bonk-runner` (the embedded runtime stub).

This file defines:

- `ContainerConfig` — stores ENTRYPOINT, CMD, ENV, WORKDIR, USER from the Docker image
- `Footer` — a binary record appended to every bonk binary so the runner can find its payload (56 bytes; embedded tool sizes are zero when using pre-installed bwrap/unsquashfs)
- `human_size(bytes)` — formats a byte count as a human-readable string

---

## Concepts

### Structs

A struct is a named collection of fields:

```rust
pub struct Point {
    pub x: f64,
    pub y: f64,
}
```

Fields are private unless you mark them `pub`. If all fields are private, code in other crates can't construct or read the struct directly.

### `impl` blocks

You attach methods to a struct with `impl`:

```rust
impl Point {
    // "associated function" (no self) — like a static method
    pub fn origin() -> Point {
        Point { x: 0.0, y: 0.0 }
    }

    // "method" (takes self by reference)
    pub fn distance_from_origin(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}
```

### Derive macros

`#[derive(...)]` automatically generates trait implementations for your struct. You don't write the code — the compiler generates it.

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Foo { ... }
```

Common derives:
- `Debug` — lets you print the struct with `{:?}`
- `Clone` — lets you call `.clone()` on it
- `Serialize` / `Deserialize` — from the `serde` crate; lets you convert to/from JSON

### Traits

A trait defines a set of methods that a type can implement. Think of it as an interface:

```rust
trait Greet {
    fn greet(&self) -> String;
}

impl Greet for Point {
    fn greet(&self) -> String {
        format!("I am at ({}, {})", self.x, self.y)
    }
}
```

The `Default` trait is special — it lets you construct a zero-value of a type with `Type::default()` or `Default::default()`. You implement it yourself or derive it.

### Integer types and byte arithmetic

Rust has explicit integer types: `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `usize` (pointer-sized unsigned). Unlike C, there is no implicit conversion between them — you must cast explicitly with `as`.

Every integer has `.to_le_bytes()` which returns a byte array in **little-endian** order, and a corresponding associated function `T::from_le_bytes([u8; N])` to convert back.

```rust
let n: u64 = 1234;
let bytes: [u8; 8] = n.to_le_bytes();
let back: u64 = u64::from_le_bytes(bytes);
assert_eq!(n, back);
```

### `Option<T>`

When a value might be absent, Rust uses `Option<T>` instead of null:

```rust
let maybe: Option<String> = Some("hello".to_string());
let nothing: Option<String> = None;
```

You can unwrap it safely with `if let Some(s) = maybe { ... }`, or use methods like `.unwrap_or("default".to_string())` and `.filter(|s| !s.is_empty())`.

### String vs &str

- `String` — owned, heap-allocated, mutable
- `&str` — borrowed reference to string data, immutable

When you need to store a string in a struct, use `String`. When you're just reading one (e.g. in a function argument), prefer `&str`. Convert with `.to_string()` or `.to_owned()`.

### Constants

```rust
pub const MAGIC: u64 = 0xB04B_B04B_B04B_B04B;
```

Underscores in number literals are ignored by the compiler — they're just for readability.

---

## Add dependencies to `bonk-common`

Before writing code, add to `crates/bonk-common/Cargo.toml`:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

---

## Tasks

### Task 1 — Declare the magic constants

In `src/lib.rs`, declare two public constants:

- `FOOTER_MAGIC: u64 = 0xB04B_B04B_B04B_0002` — footer magic number
- `FOOTER_SIZE: usize = 56` — footer size (7 × u64)

> The magic number is written into the last bytes of every bonk binary so the runner can verify it's reading a real bonk file.

### Task 2 — Define `ContainerConfig`

Create a public struct `ContainerConfig` with these fields:

| Field | Type | Meaning |
|---|---|---|
| `entrypoint` | `Vec<String>` | Docker ENTRYPOINT (e.g. `["python3"]`) |
| `cmd` | `Vec<String>` | Docker CMD (e.g. `["app.py"]`) |
| `env` | `Vec<String>` | Environment variables as `"KEY=VALUE"` strings |
| `working_dir` | `String` | The container's working directory |
| `user` | `Option<String>` | Optional user to run as |

Add `#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]` to it.

> **Hint:** `Vec<String>` is the owned type for a list of strings. You create one with `vec!["a".to_string(), "b".to_string()]` or `Vec::new()`.

### Task 3 — Implement `Default` for `ContainerConfig`

Implement the `Default` trait for `ContainerConfig` manually (don't derive it). The default values should be:

- `entrypoint`: empty vec
- `cmd`: a vec containing the single string `"/bin/sh"`
- `env`: a vec containing `"PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"`
- `working_dir`: `"/"`
- `user`: `None`

> **Hint:** Implement this with `impl Default for ContainerConfig { fn default() -> Self { ... } }`.

### Task 4 — Define `Footer`

Create a public struct `Footer` with five `pub` fields, all `u64`:

- `payload_offset` — byte offset where the compressed rootfs starts within the binary
- `payload_size` — length of the compressed rootfs in bytes
- `config_size` — length of the serialised `ContainerConfig` JSON in bytes
- `bwrap_size` — size of the embedded static bwrap binary (0 if not embedded)
- `unsquashfs_size` — size of the embedded static unsquashfs binary (0 if not embedded)

The footer is **56 bytes**: 5 × 8 bytes for the fields + 8 bytes reserved + 8 bytes for the magic.

When `bwrap_size` and `unsquashfs_size` are both zero the runner falls back to bwrap/unsquashfs found on PATH — this is how you support pre-installed tools without a separate binary format.

Add a method `has_embedded_tools(&self) -> bool` that returns `true` if both `bwrap_size > 0` and `unsquashfs_size > 0`.

Add helper offset methods:
- `bwrap_offset(&self) -> u64` — `payload_offset + payload_size`
- `unsquashfs_offset(&self) -> u64` — `bwrap_offset() + bwrap_size`
- `config_offset(&self) -> u64` — `unsquashfs_offset() + unsquashfs_size`

### Task 5 — Implement `Footer::to_bytes`

Add a method `pub fn to_bytes(&self) -> Vec<u8>` that serialises the footer as 56 bytes:

```
bytes  0– 7 : payload_offset   (little-endian u64)
bytes  8–15 : payload_size     (little-endian u64)
bytes 16–23 : config_size      (little-endian u64)
bytes 24–31 : bwrap_size       (little-endian u64)
bytes 32–39 : unsquashfs_size  (little-endian u64)
bytes 40–47 : reserved         (0u64)
bytes 48–55 : FOOTER_MAGIC     (little-endian u64)
```

When `bwrap_size` and `unsquashfs_size` are both zero (no embedded tools), the footer is still 56 bytes — those fields simply hold zero.

### Task 6 — Implement `Footer::from_bytes`

Add an associated function `pub fn from_bytes(data: &[u8]) -> Option<Footer>` that:

1. Returns `None` if `data.len() < 56`.
2. Reads the last 56 bytes, checks the magic at bytes 48–55 against `FOOTER_MAGIC`. Returns `None` if it doesn’t match.
3. Parses all 5 fields and returns `Some(Footer { … })`.

> **Note:** `from_bytes` takes a `&[u8]` (the entire binary data), not a fixed-size array. It reads from the end of the slice. This lets the runner pass the whole `/proc/self/exe` contents.

> **Hint:** To convert a slice to a fixed-size array, use `slice.try_into().ok()?`. The `?` in an `Option`-returning function short-circuits with `None` on failure.

### Task 7 — Implement `human_size`

Write a public function `pub fn human_size(bytes: usize) -> String` that formats a byte count:

- If `bytes >= 1_073_741_824` (1 GiB) → format as `"X.XX GB"`
- If `bytes >= 1_048_576` (1 MiB) → format as `"X.XX MB"`
- Otherwise → format as `"X.XX KB"`

> **Hint:** Cast `bytes` to `f64` and divide. Use `format!("{:.2} MB", value)` for two decimal places.

### Task 8 — Verify

Add `bonk-common` as a dependency to `bonk-cli`'s `Cargo.toml`:

```toml
[dependencies]
bonk-common = { path = "../bonk-common" }
```

In `bonk-cli/src/main.rs`, add `use bonk_common::ContainerConfig;` and construct a `ContainerConfig::default()`. Print it with `{:?}`. Run `cargo run --bin bonk` and confirm it prints the struct.

> **Note:** Cargo converts hyphens in crate names to underscores in Rust identifiers (`bonk-common` → `bonk_common`).

---

## Check your understanding

1. Why does `Footer` need `to_bytes()` / `from_bytes()` rather than just deriving `Serialize`?
2. What does `Option<String>` let you express that a plain `String` cannot?
3. What would happen if you tried to store a `&str` in `ContainerConfig` instead of `String`?

---

## Next lesson

In Lesson 03 you'll build the CLI entry point — option parsing with `clap` and idiomatic Rust error handling with `anyhow`.
