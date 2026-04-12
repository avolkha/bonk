# Lesson 07 — Iterators & OCI Layer Flattening (`flatten.rs`)

## What you will build

The remaining functions in `crates/bonk-cli/src/flatten.rs`:

- `pub fn flatten_layers(layers: &[PathBuf], rootfs: &Path) -> Result<()>` — applies layers in order
- `fn apply_layer(layer_tar: Box<dyn Read>, rootfs: &Path) -> Result<()>` — applies one layer, handling OCI whiteout files

Docker images are built in layers. When you pull `python:3.12-slim`, you get a base layer (Debian), an upgrade layer, a Python install layer, etc. Each layer is a tar of file changes. To reconstruct the full filesystem you apply them bottom-up, letting later layers override earlier ones. Some entries are *deletions* — those are represented by special whiteout files.

---

## Concepts

### Iterators

Rust's iterator system is powerful. Most collections have an `.iter()` method returning an iterator, and iterators have many adaptor methods:

```rust
let numbers = vec![1, 2, 3, 4, 5];
let doubled: Vec<i32> = numbers.iter()
    .map(|x| x * 2)
    .filter(|x| *x > 4)
    .collect();       // [6, 8, 10]
```

Key `Iterator` methods you'll use:
- `.enumerate()` — pairs each item with its index: `(0, item), (1, item), ...`
- `.collect::<Vec<_>>()` — pull all items into a `Vec`
- `.next()` — take the next item (returns `Option<T>`)
- `.for_each(|item| { ... })` — consume the iterator

### `for` loops over iterators

```rust
for item in collection.iter() {
    // item is a reference &T
}

for (i, item) in collection.iter().enumerate() {
    // i is usize, item is &T
}
```

### Slices: `&[T]`

A slice `&[T]` is a reference to a contiguous sequence of `T` values. `&[PathBuf]` lets you pass both a `Vec<PathBuf>` and an array `[PathBuf; N]` to the same function — it's more general than `&Vec<PathBuf>`.

### `tar::Archive` — reading entries

```rust
use std::io::Read;
use tar::Archive;

fn process_tar(source: impl Read) {
    let mut archive = Archive::new(source);
    for entry in archive.entries()? {
        let mut entry = entry?;            // each entry is also a Result
        let path = entry.path()?.to_owned();
        // `entry` implements Read — you can also read file content from it
        entry.unpack_in("/some/dir")?;     // extract this entry to disk
    }
}
```

Note: `entries()` borrows from `archive`, so `archive` must be `mut` and must live long enough.

### String operations

```rust
let s = "foo/bar/.wh.baz";

s.starts_with(".wh.")       // → true
s.strip_prefix(".wh.")      // → Some("baz")
s.contains('/')             // → true
```

### OCI whiteout specification

The OCI (Open Container Initiative) layer format uses whiteout files to represent deletions:

| Entry name | Meaning |
|---|---|
| `.wh.<name>` | Delete file or directory `<name>` in the same directory |
| `.wh..wh..opq` | Opaque whiteout — delete **everything** in the parent directory |

A whiteout file is never extracted to disk. Instead, you look at its name to decide what to delete from `rootfs/` before unpacking the rest of the layer.

**Example:** If layer 3 contains `etc/.wh.passwd`, you should:
1. Delete `rootfs/etc/passwd` if it exists
2. Not write `etc/.wh.passwd` to disk

**Example:** If layer 4 contains `var/cache/.wh..wh..opq`, you should:
1. Delete all contents of `rootfs/var/cache/`
2. Keep the `rootfs/var/cache/` directory itself (it may have new files in this same layer)

### Filesystem operations

```rust
std::fs::remove_file(&path)?;           // delete a file
std::fs::remove_dir_all(&path)?;        // delete directory and all contents
std::fs::create_dir_all(&path)?;        // create directory (and parents)
```

These return `Result` — but for `remove_*` operations in the layer-flattening context, errors are often non-fatal (the file to delete might not exist). You can use `.ok()` to ignore errors or `if path.exists()` before removing.

---

## Tasks

### Task 1 — `flatten_layers` function

Implement:

```
pub fn flatten_layers(layers: &[PathBuf], rootfs: &Path) -> Result<()>
```

1. Create `rootfs` with `std::fs::create_dir_all(rootfs)`
2. Iterate over `layers` in order
3. For each layer path, call `open_layer(layer_path)?` to get a `Box<dyn Read>`
4. Call `apply_layer(reader, rootfs)?`
5. Return `Ok(())`

### Task 2 — `apply_layer` function

Implement:

```
fn apply_layer(layer_tar: Box<dyn Read>, rootfs: &Path) -> Result<()>
```

1. Create a `tar::Archive` from `layer_tar`
2. Iterate over `archive.entries()?`; for each entry:
   a. Let `entry = entry?` (handle iteration errors)
   b. Get the entry's path as an owned value — `let path = entry.path()?.into_owned()`
   c. Get just the filename component: `path.file_name()` (returns `Option<&OsStr>`)
   d. Convert to `&str` if possible; if not, unpack normally and continue

### Task 3 — Handle opaque whiteout

If the filename equals `".wh..wh..opq"`:
- Determine the parent directory by calling `.parent()` on the full `path`
- Build the corresponding path in rootfs: `rootfs.join(parent_dir)`
- If that directory exists, iterate over its contents with `std::fs::read_dir` and remove each entry with `fs::remove_file` or `fs::remove_dir_all`
- `continue` to the next tar entry (don't unpack this whiteout file)

### Task 4 — Handle regular whiteout

If the filename starts with `".wh."` but is not the opaque whiteout:
- Strip the `".wh."` prefix to get the real filename
- Build the target path: `rootfs.join(parent_dir).join(real_filename)`
- Remove it: try `fs::remove_file` first, then `fs::remove_dir_all` if it's a directory — or use `if target.is_dir()` to decide which
- `continue` to the next tar entry

### Task 5 — Handle normal entries

For any entry that is not a whiteout file:
- Call `entry.unpack_in(rootfs)?`
- If this returns an `Err`, print a warning with `eprintln!` but don't propagate the error — some entries in real Docker images can legitimately fail to unpack (e.g. special device files that require root)

> **Hint:** Use `if let Err(e) = entry.unpack_in(rootfs) { eprintln!("warning: ..."); }` to handle non-fatal errors.

### Task 6 — Wire it up

In `main.rs`, after parsing the image, call:

```rust
flatten::flatten_layers(&layers, &work_dir.path().join("rootfs"))?;
```

### Task 7 — Verify

Run `cargo run --bin bonk -- alpine:latest`. Steps 1–3 should now complete. Check that `target/debug/` doesn't have weird leftover temp dirs — the `TempDir` should clean up automatically.

---

## Check your understanding

1. Why do we iterate layers in order from the first (oldest/base) to the last (newest)? What would happen if you reversed the order?
2. Why are whiteout errors non-fatal (we just log them), but JSON parse errors are fatal?
3. What does `path.file_name()` return for a path like `"usr/bin/python3"`? What about for `"usr/"`?

---

## Next lesson

In Lesson 08 you'll build `pack.rs` — building a SquashFS image from the flattened rootfs with `mksquashfs` and assembling the final self-contained executable.
