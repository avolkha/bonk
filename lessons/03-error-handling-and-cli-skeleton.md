# Lesson 03 — Error Handling & CLI Skeleton (`bonk-cli/main.rs`)

## What you will build

`crates/bonk-cli/src/main.rs` — the entry point for the `bonk` build tool. This file:

1. Parses command-line arguments (`bonk alpine:latest -o ./alpine`)
2. Calls through the five-step build pipeline (the modules you'll write in later lessons)
3. Reports progress to the user

---

## Concepts

### `Result<T, E>` — Rust's error type

Rust has no exceptions. Functions that can fail return `Result<T, E>`:

```rust
enum Result<T, E> {
    Ok(T),    // success, contains the value
    Err(E),   // failure, contains the error
}
```

To handle a `Result` you must either:
- Match on it: `match result { Ok(v) => ..., Err(e) => ... }`
- Propagate it with `?`: `let v = might_fail()?;` — if it's `Err`, return early from the current function

### `anyhow` — ergonomic errors

The `anyhow` crate gives you a convenient `anyhow::Result<T>` type where the error type is a dynamic, human-readable error that can hold any error:

```rust
use anyhow::Result;

fn my_func() -> Result<String> {
    let content = std::fs::read_to_string("file.txt")?;  // any IO error becomes anyhow::Error
    Ok(content)
}
```

Key `anyhow` tools:
- `anyhow::Result<T>` — shorthand for `Result<T, anyhow::Error>`
- `?` operator — propagates any error that implements `std::error::Error`
- `.context("message")` — wraps an error with extra context info
- `bail!("message")` — immediately returns `Err(anyhow!("message"))`

```rust
use anyhow::{bail, Context, Result};

fn example(path: &str) -> Result<String> {
    if path.is_empty() {
        bail!("path must not be empty");  // return Err immediately
    }
    let content = std::fs::read_to_string(path)
        .context("failed to read config file")?;  // adds context on error
    Ok(content)
}
```

### `clap` with `#[derive(Parser)]`

`clap` is the standard Rust library for CLI argument parsing. With the `derive` feature, you define your CLI as a struct and clap generates all the parsing code:

```rust
use clap::Parser;

#[derive(Parser)]
struct Cli {
    /// The image to squash (positional argument)
    image: String,

    /// Output path for the generated binary
    #[arg(short, long)]
    output: Option<String>,
}
```

- Doc comments `///` become help text
- `#[arg(short, long)]` enables `-o` and `--output`
- `Option<String>` makes the argument optional
- `String` (no Option) makes it required

Parse with `Cli::parse()` which reads `std::env::args()` and exits with a help message if invalid.

### `eprintln!` vs `println!`

`println!` writes to stdout. `eprintln!` writes to stderr. Progress messages and errors should go to stderr so they don't pollute stdout when the program's output is piped elsewhere.

### Deriving an output name from an image name

The image string `"registry.example.com/user/alpine:latest"` should become the output filename `"alpine"`. This involves two string operations:

- Strip the registry/path prefix by splitting on `'/'` and taking the last segment
- Strip the tag by splitting on `':'` and taking the first segment

Rust's `str::split()` returns an iterator. You can call `.last()` or `.next()` on it.

---

## Add dependencies

Add to `crates/bonk-cli/Cargo.toml`:

```toml
[dependencies]
bonk-common = { path = "../bonk-common" }
anyhow = "1"
clap = { version = "4", features = ["derive"] }
tempfile = "3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tar = "0.4"
flate2 = "1"
zstd = "0.13"
```

---

## Tasks

### Task 1 — Define the `Cli` struct

In `src/main.rs`, define a struct called `Cli` with:

- A required positional field `image: String` — the Docker image to squash
- An opti
onal flag `-o` / `--output` of type `Option<String>` — where to write the output binary
- An optional flag `--bwrap-path` of type `Option<String>` — explicit path to the bwrap binary to embed
- An optional flag `--unsquashfs-path` of type `Option<String>` — explicit path to the unsquashfs binary to embed

Derive `clap::Parser` on it and add doc-comment help text to each field.

> **Why these flags?** By default `bonk` searches for bwrap and unsquashfs automatically (env var → tools dir → PATH). Providing an explicit path lets you override this — useful in CI, cross-architecture builds, or when you want to pin exact verified versions of the tools.

### Task 2 — Derive the output name

Write a helper function (or inline logic) that converts an image name like `"registry.io/user/myapp:v1.2"` into an output filename like `"myapp"`.

Rules:
- Split on `'/'`, take the last segment
- Split that on `':'`, take the first segment
- Wrap in `"./"` (so it becomes `"./myapp"` — a relative path in the current directory)

If the user passed `--output`, use that instead.

### Task 3 — Write the `main` function

`main()` should:

1. Parse `Cli::parse()`
2. Derive the output path using your Task 2 logic
3. Create a temporary working directory with `tempfile::tempdir()`
4. Call through five pipeline steps (stubbed for now — just `eprintln!` placeholders):
   - `[1/5] Exporting image...`
   - `[2/5] Parsing image manifest...`
   - `[3/5] Flattening layers...`
   - `[4/5] Compressing rootfs...`
   - `[5/5] Assembling binary...`
5. Print a success line: `"→ output_path (X bytes)"`

The function signature must be `fn main() -> anyhow::Result<()>` so you can use `?` inside it.

> **Hint:** `tempfile::tempdir()` returns a `TempDir`. It deletes itself when dropped. Store it in a variable — if you let it be immediately dropped, the directory disappears.

### Task 4 — Add module declarations

Make sure `main.rs` declares the three modules: `mod image;`, `mod flatten;`, `mod pack;`. They can stay empty for now.

### Task 5 — Verify

Run:

```bash
cargo run --bin bonk -- --help
```

You should see a help message listing `image`, `--output`, `--bwrap-path`, and `--unsquashfs-path`. Then run:

```bash
cargo run --bin bonk -- myregistry.io/user/myapp:v1.0
```

It should print the five progress lines and exit cleanly.

---

## Check your understanding

1. What happens if you use `?` inside a function that returns `()` instead of `Result`?
2. Why use `Option<String>` for `--output` rather than `String` with a default?
3. What does `.context("...")` actually do to an error — does it change the error type?
4. `--bwrap-path` and `--unsquashfs-path` use `Option<String>` even though the path types in Rust are `Path` / `PathBuf`. Why not use `Option<PathBuf>` directly in the `Cli` struct?

---

## Next lesson

In Lesson 04 you'll implement `image::export_image` — running `docker save` as a subprocess and extracting the resulting tar archive.
