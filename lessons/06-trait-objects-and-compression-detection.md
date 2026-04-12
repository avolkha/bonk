# Lesson 06 — Trait Objects & Compression Detection (`flatten::open_layer`)

## What you will build

The first function in `crates/bonk-cli/src/flatten.rs` — `open_layer`, which:

1. Opens a layer tar file
2. Reads the first 2 bytes to detect compression format (magic bytes)
3. Seeks back to the start
4. Returns the file wrapped in the appropriate decoder — or raw if uncompressed

The key challenge: the caller doesn't know whether the file is gzip, zstd, or plain. You need to return something the caller can treat uniformly as "readable bytes". That's what `Box<dyn Read>` is for.

---

## Concepts

### Traits as abstractions

A `trait` defines a shared interface — a set of method signatures that any type implementing the trait must provide. The standard library's `std::io::Read` trait is:

```rust
pub trait Read {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error>;
    // ... plus many provided methods built on top of `read`
}
```

Anything that implements `Read` can be passed to code that just needs to read bytes — a `File`, a network socket, an in-memory slice, a gzip decoder, etc.

### Trait objects: `Box<dyn Trait>`

Normally in Rust, types must be known at compile time. But sometimes you want to return "something that implements Read" where the concrete type is only known at runtime. That's a **trait object**:

```rust
fn open_something(flag: bool) -> Box<dyn Read> {
    if flag {
        Box::new(File::open("file.txt").unwrap())  // File implements Read
    } else {
        Box::new(std::io::Cursor::new(b"hello"))   // Cursor also implements Read
    }
}
```

`Box<dyn Read>` is a heap-allocated, type-erased value that implements `Read`. The caller can call `.read()` on it without knowing the concrete type. The `Box` is needed because the compiler doesn't know the size of `dyn Read` at compile time.

### `std::io::Seek` — moving the read position

Seekable types (like `File`) implement `Seek`, which lets you jump to any position:

```rust
use std::io::{Seek, SeekFrom};

file.seek(SeekFrom::Start(0))?;    // rewind to beginning
file.seek(SeekFrom::Current(-4))?; // go back 4 bytes from current position
file.seek(SeekFrom::End(0))?;      // seek to end
```

### Magic bytes — format detection

Many binary formats identify themselves with a fixed sequence of bytes at the start of the file:

| Format | First bytes (hex) |
|---|---|
| gzip | `1F 8B` |
| zstd | `28 B5 2F FD` (but you only need 2 bytes: `28 B5`) |
| plain tar | anything else |

You can read the first few bytes, check the pattern, then seek back to position 0 before passing the file to a decoder.

### Reading exactly N bytes

To read a fixed number of bytes from a `File`:

```rust
use std::io::Read;

let mut magic = [0u8; 2];
file.read_exact(&mut magic)?;  // reads exactly 2 bytes, errors if fewer available
```

`read_exact` is a provided method on `Read` — it repeatedly calls `read` until exactly the requested number of bytes are filled.

### `flate2` and `zstd` decoders

Both wrap a `Read` source:

```rust
use flate2::read::GzDecoder;
let gz = GzDecoder::new(file);   // file: impl Read

use zstd::Decoder;
let zstd = Decoder::new(file)?;  // file: impl Read
```

Both `GzDecoder` and `zstd::Decoder` themselves implement `Read` — reading from them transparently decompresses the data.

---

## Tasks

### Task 1 — Function signature

In `src/flatten.rs`, declare a private helper function:

```
fn open_layer(path: &Path) -> Result<Box<dyn std::io::Read>>
```

(Private because only `flatten.rs` uses it internally.)

### Task 2 — Open the file and read magic bytes

Open the file with `std::fs::File::open(path)`. Declare a `[u8; 2]` array and use `read_exact` to fill it with the first two bytes.

### Task 3 — Seek back to the start

After reading the magic bytes, the file's read position is 2 bytes in. Rewind it to position 0 using `Seek::seek`.

> **Hint:** To use `seek`, your `file` variable must be declared `mut`. Also bring `std::io::{Read, Seek, SeekFrom}` into scope with `use`.

### Task 4 — Match on magic and return appropriate decoder

Use a `match` (or `if`/`else if`) on the magic bytes:

- `[0x1F, 0x8B]` → wrap in `flate2::read::GzDecoder::new(file)`, box it
- `[0x28, 0xB5]` → wrap in `zstd::Decoder::new(file)`, box it, propagate errors with `?`
- anything else → treat as plain tar, box the raw `file`

All three branches must return `Ok(Box::new(...))` with a type that implements `std::io::Read`.

> **Hint:** `Box::new(thing)` allocates `thing` on the heap. `Box<dyn Read>` is the erased type. All three branches return the same Rust type (`Box<dyn Read>`) even though the concrete types differ.

### Task 5 — Verify the types compile

At this point, `open_layer` should compile cleanly. To test it in isolation, add a temporary `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn open_layer_plain() {
        // create a small plain file, call open_layer, make sure it returns Ok
    }
}
```

Run `cargo test --bin bonk`.

---

## Check your understanding

1. Why can't you just return `File` (or `GzDecoder<File>`) directly without boxing?
2. What would happen if you forgot to seek back to position 0 before passing the file to a decoder?
3. `Box<dyn Read>` involves a heap allocation. Why is that necessary here?

---

## Next lesson

In Lesson 07 you'll build `flatten_layers` and `apply_layer` — iterating over a tar archive's entries, handling OCI whiteout files, and writing the merged filesystem to disk.
