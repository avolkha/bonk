# Lesson 01 — Workspace & Project Structure

## What you will build

A Cargo workspace containing three crates:

- `bonk-common` — a **library** crate (shared types, no binary)
- `bonk-cli` — a **binary** crate, compiled to an executable called `bonk`
- `bonk-runner` — a **binary** crate, compiled to an executable called `bonk-runner`

By the end of this lesson `cargo build` should compile successfully with empty placeholder stubs in each crate.

---

## Concepts

### Cargo workspaces

A workspace is a directory containing a root `Cargo.toml` that lists *member* crates. When you run `cargo build` in the workspace root, all members are compiled together and share a single `target/` output directory.

```toml
[workspace]
members = ["crates/bonk-common", "crates/bonk-cli", "crates/bonk-runner"]
resolver = "2"
```

`resolver = "2"` tells Cargo to use its modern feature resolver. You rarely need to understand the details yet — just include it.

### Library vs binary crates

Every crate has a manifest (`Cargo.toml`) that describes what it produces:

| Crate type | Cargo.toml section | Output |
|---|---|---|
| Library | `[lib]` (implicit if `src/lib.rs` exists) | `.rlib` — linkable by other crates |
| Binary | `[[bin]]` — double brackets because multiple binaries are allowed | A runnable executable |

### Modules (`mod`)

Inside a crate you can split code across files using `mod`:

```rust
// in src/main.rs
mod image;   // tells the compiler to look for src/image.rs
mod flatten;
mod pack;
```

The `mod` keyword declares a module. Without it, `image.rs` would be invisible to the compiler even if the file exists.

### Visibility (`pub`)

By default everything in Rust is **private** — only the code in the same module can see it. Put `pub` in front of anything you want to share with other modules or crates:

```rust
pub struct Footer { ... }   // visible to anyone
pub fn human_size(...) { }  // visible to anyone
struct Internal { ... }     // only this module
```

### The `use` keyword

`use` brings a path into scope so you don't have to write the full path every time:

```rust
use std::path::PathBuf;   // now you can write PathBuf instead of std::path::PathBuf
use anyhow::Result;
```

### Ownership in one paragraph

Rust enforces memory safety at compile time without a garbage collector. Every value has exactly one *owner*. When the owner goes out of scope the value is dropped (freed). You can temporarily *borrow* a value with references (`&T` for read, `&mut T` for write), but you can't have both a mutable and an immutable borrow alive at the same time. This will make more sense as you hit compiler errors — the compiler messages are very good.

---

## Tasks

Work inside a fresh directory (e.g. `~/my-bonk`). Do **not** copy any code from the real codebase.

### Task 1 — Create the workspace root

Create `Cargo.toml` at the root of your project. It must:

- Declare a `[workspace]` with three members: `crates/bonk-common`, `crates/bonk-cli`, `crates/bonk-runner`
- Set `resolver = "2"`

> **Hint:** A workspace `Cargo.toml` does **not** have a `[package]` section.

### Task 2 — Create `bonk-common` as a library crate

Under `crates/bonk-common/`:

- Write a `Cargo.toml` with:
  - `[package]` block: `name = "bonk-common"`, `version = "0.1.0"`, `edition = "2021"`
  - No `[[bin]]` section (library crates don't need one)
- Create `src/lib.rs` with a single public function stub: `pub fn hello() {}`

> **Hint:** `cargo new --lib crates/bonk-common` will scaffold this for you.

### Task 3 — Create `bonk-cli` as a binary crate

Under `crates/bonk-cli/`:

- Write a `Cargo.toml` that includes a `[[bin]]` section with `name = "bonk"` and `path = "src/main.rs"`
- Create `src/main.rs` with a `main()` function that prints `"bonk placeholder"`

> **Hint:** When you specify `[[bin]]` manually, you control the output binary name independently of the crate name.

### Task 4 — Create `bonk-runner` as a binary crate

Same as Task 3, but:

- Crate name `bonk-runner`
- Binary name `bonk-runner`
- Print `"bonk-runner placeholder"`

### Task 5 — Add module stubs to `bonk-cli`

In `bonk-cli/src/main.rs`, declare three modules: `image`, `flatten`, `pack`.

Create empty placeholder files `src/image.rs`, `src/flatten.rs`, `src/pack.rs` inside `bonk-cli/src/`.

> **Hint:** An empty file is valid Rust. Just `mod image;` in `main.rs` and an empty `image.rs` is enough.

### Task 6 — Verify

Run:

```bash
cargo build
```

from your workspace root. All three crates should compile without errors. You should find:

- `target/debug/bonk` — the `bonk-cli` binary
- `target/debug/bonk-runner` — the `bonk-runner` binary

---

## Check your understanding

Answer these before moving on:

1. Why does the workspace `Cargo.toml` not have a `[package]` section?
2. What is the difference between `mod image;` and `use image::something;`?
3. If you define a function `fn helper() {}` inside `src/image.rs`, can `main.rs` call it? What would you need to change?

---

## Next lesson

In Lesson 02 you'll implement `bonk-common/src/lib.rs` — the shared types used by both crates. You'll meet structs, impl blocks, traits, and Rust's number types.
