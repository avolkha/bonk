# Lesson 09 — Self-Reading Binaries & Caching (`bonk-runner/main.rs`)

## What you will build

`crates/bonk-runner/src/main.rs` — the stub binary that gets embedded in every bonk output. When the user runs `./alpine`, this code runs. It:

1. Reads its own bytes from `/proc/self/exe`
2. Parses the footer from the last 56 bytes of the file
3. Locates and parses the embedded `config.json`
4. Computes a cache key and checks if the rootfs is already extracted
5. Extracts embedded tool binaries (bwrap, unsquashfs) to cache directory
6. Extracts the SquashFS payload to `/tmp/bonk-<hash>/rootfs/` via embedded `unsquashfs` if needed
7. Parses user-provided flags (`-v`, `--`) from `argv`
8. Execs embedded bwrap over the extracted rootfs and exits with the container's exit code

---

## Concepts

### Reading a process's own executable

On Linux, `/proc/self/exe` is a symlink to the running process's binary. You can read it like any file:

```rust
let data = std::fs::read("/proc/self/exe")?;
// data is now a Vec<u8> containing the full executable
```

This is how `bonk-runner` finds its own embedded payload — it reads itself and then slices out the payload section using the footer offsets.

> **Note:** This is a well-known trick used by any tool that wants to ship "data + code as one file" without depending on an installer or external files. Self-extracting archives (makeself, WinRAR SFX), UPX, PyInstaller, and dockerc (the direct inspiration for bonk) all use the same approach.

### Byte-slice indexing

A `Vec<u8>` or `&[u8]` can be sliced like an array:

```rust
let data = vec![0u8; 100];

let first_ten = &data[0..10];      // bytes 0–9
let last_eight = &data[92..];      // bytes 92–99
let last_n = &data[data.len()-32..]; // last 32 bytes
```

Indexing with a range returns a `&[u8]` slice. Out-of-bounds indexing **panics** at runtime — validate lengths before indexing.

A `&[u8]` slice is just a fat pointer: a memory address plus a length. It doesn't copy any data — it's a view into the original `Vec<u8>`. This is important for bonk-runner: the executable can be tens of megabytes, but slicing out the config or payload sections is effectively free. The data only gets copied when you explicitly call `.to_vec()` or pass it to something that needs ownership.

When you need to find data at the *end* of a buffer (e.g. a footer), the idiomatic pattern is:

```rust
let footer_bytes = &data[data.len() - FOOTER_SIZE..];
```

This is exactly how `Footer::from_bytes` works internally — it reads the last 56 bytes regardless of how large the binary is.

In bonk, byte-slice indexing is used in three places:

- **`bonk-runner/main.rs`** — slices the payload and config sections out of the full executable using offsets from the footer
- **`bonk-common/src/lib.rs`** — `Footer::from_bytes` slices the last 56 bytes to parse the footer struct
- **`bonk-cli/src/pack.rs`** — `write_sections` writes each section in sequence; the offsets it records are later used by the runner to slice them back out

### Converting a slice to a fixed-size array reference

`Footer::from_bytes` takes a `&[u8]` — typically the entire executable data. It reads
the last 56 bytes from the end of the slice:

```rust
let footer = Footer::from_bytes(&exe_data)
    .context("not a bonk binary")?;
```

The footer then provides helper methods like `config_offset()`, `bwrap_offset()`,
and `unsquashfs_offset()` to locate each embedded section.

### Hashing for cache keys

The SquashFS image can be large. To avoid re-writing it on every run, bonk-runner uses a hash of the payload as a cache key:

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

let mut hasher = DefaultHasher::new();
payload[..4096.min(payload.len())].hash(&mut hasher);
payload.len().hash(&mut hasher);   // also include the size
let key: u64 = hasher.finish();

let cache_dir = std::path::PathBuf::from(format!("/tmp/bonk-{:016x}", key));
```

`{:016x}` formats a `u64` as a 16-character zero-padded lowercase hex string.

### Marker files

After writing the SquashFS image, create an empty "marker" file to signal that the write completed successfully:

```rust
let marker = cache_dir.join(".bonk-ready");
if !marker.exists() {
    let _ = std::fs::remove_dir_all(&cache_dir);
    std::fs::create_dir_all(&cache_dir)?;
    mount::write_squashfs(payload, &sqfs_path)?;
    std::fs::write(&marker, b"")?;  // create the marker
}
```

On future runs, if the marker exists the write is skipped. If a previous run was interrupted (crash during write), the partial `.sqfs` won't have the marker, so the write will be retried from scratch.

### Manual argument parsing with `std::env::args()`

`clap` is not used in `bonk-runner` — it would add size to the embedded stub. Instead, parse arguments manually:

```rust
let args: Vec<String> = std::env::args().collect();
// args[0] is the program name, args[1..] are the user's arguments

let mut volumes: Vec<VolumeMount> = Vec::new();
let mut extra_args: Vec<String> = Vec::new();
let mut quiet = false;
let mut saw_sep = false;   // true once we see "--"

let mut i = 1;
while i < args.len() {
    let arg = &args[i];
    if saw_sep {
        extra_args.push(arg.clone());
    } else if arg == "--" {
        saw_sep = true;
    } else if arg == "-q" || arg == "--quiet" {
        quiet = true;
    } else if arg == "-v" || arg == "--volume" {
        i += 1;
        // args[i] is the volume spec
    } else if arg.starts_with("-v") {
        // inline: "-v/host:/guest"
        let spec = &arg[2..];  // strip the "-v" prefix
    } else {
        extra_args.push(arg.clone());
        saw_sep = true;  // first non-flag arg: everything after is CMD
    }
    i += 1;
}
```

### TTY detection with `std::io::IsTerminal`

When a container binary is invoked via a pipe (`echo hi | ./alpine ...`), stdin
is not a terminal. Many container runtimes default to `terminal: true` in the
OCI config, which causes a `tcgetattr` failure when stdin is not a TTY
(dockerc #52). Detect this at runtime:

```rust
use std::io::IsTerminal;

let stdin_is_tty = std::io::stdin().is_terminal();
```

The runner should pass this information to the runtime so it can decide whether
to allocate a pseudo-terminal. In practice, for bwrap-based containers this
means you **don't** need to do anything special (bwrap doesn't set `terminal`),
but you should avoid wrapping in `script` or `socat` for TTY emulation unless
stdin is actually a terminal.

### `std::process::exit`

To propagate the container's exit code to the caller:

```rust
std::process::exit(code);   // terminates the process immediately with the given code
```

---

## Add dependencies

Add to `crates/bonk-runner/Cargo.toml`:

```toml
[dependencies]
bonk-common = { path = "../bonk-common" }
anyhow = "1"
serde_json = "1"
```

Note: no `tar` or `zstd` — the runner no longer decompresses anything itself.

### Task 0 — Declare modules

In `bonk-runner/src/main.rs`, declare two modules: `mod mount;` and `mod runtime;`. Create empty placeholder files for them.

---

## Tasks

### Task 1 — Print usage

If the first argument is `"--help"` or `"-h"`, print a usage message to stdout and exit with code 0. The usage message should explain:

- That this is a bonk-generated container binary
- The `-v HOST:GUEST[:ro]` flag for volume mounts
- The `-q` / `--quiet` flag to suppress progress output
- The `--` separator for CMD arguments
- The `BONK_BWRAP=<path>` environment variable

### Task 2 — Parse arguments

Implement the argument parsing loop described in the concepts section. You'll need `VolumeMount` from `bonk-runner::runtime` — declare a placeholder `pub struct VolumeMount` in `runtime.rs` for now, and import it.

Collect:
- `volumes: Vec<VolumeMount>` — one per `-v` flag
- `extra_args: Vec<String>` — CMD override arguments
- `quiet: bool` — set to `true` if `-q` or `--quiet` is given
- `stdin_is_tty: bool` — detect with `std::io::stdin().is_terminal()`

Use the same `log!` macro pattern from lesson 08 to guard progress messages
behind `!quiet`. The runner should use `log!` for cache extraction messages.

### Task 3 — Read the executable

Read `/proc/self/exe` into a `Vec<u8>`.

Validate that it's long enough to contain a footer (at least `FOOTER_SIZE` bytes), otherwise bail with a clear error.

### Task 4 — Parse the footer

Pass the entire `&exe_data` to `Footer::from_bytes()`. It reads the last 56 bytes and checks the magic. If it returns `None`, bail with a message like `"not a bonk binary — footer magic does not match"`.

### Task 5 — Extract config and payload slices

Use the footer's helper methods to compute byte ranges:

```
payload:  data[footer.payload_offset .. footer.payload_offset + footer.payload_size]
config:   data[footer.config_offset() .. footer.config_offset() + footer.config_size]
```

Parse the config slice with `serde_json::from_slice::<ContainerConfig>(config_slice)?`.

### Task 6 — Cache check and extraction

Compute the cache key by hashing the first 4 KB of the payload and the payload size.

Define the cache layout:
```
/tmp/bonk-<hash>/
    bin/            ← extracted embedded tools (bwrap, unsquashfs)
    rootfs/         ← extracted rootfs directory
    .bonk-ready     ← marker: extraction complete
```

If the marker is absent:
1. `remove_dir_all` the cache dir (clean any partial state)

2. Extract embedded tools: if `footer.has_embedded_tools()`, create `bin/` in the
   cache dir and write the bwrap and unsquashfs binaries from the embedded data
   (using `footer.bwrap_offset()` / `footer.unsquashfs_offset()` to locate them).
   Set permissions to `0o755`. This is idempotent — skip if the files already exist.

3. Call `mount::extract_rootfs(payload, &rootfs_path, unsquashfs_path.as_deref())?`
   where `unsquashfs_path` is the path to the extracted embedded unsquashfs
   (or `None` for binaries without embedded tools, which fall back to system `unsquashfs`)

4. Write the marker file

Create a helper function `extract_embedded_tools` that takes the footer, exe
data, and cache dir, and returns `Result<(Option<PathBuf>, Option<PathBuf>)>`
for the bwrap and unsquashfs paths.

### Task 7 — Launch

Call `runtime::run(&rootfs_path, &config, &extra_args, &volumes, bwrap_path.as_deref(), stdin_is_tty)` — this returns
a `Result<std::process::ExitStatus>`. The `bwrap_path` is `Some(path)` if the
footer has embedded tools, or `None` for binaries without embedded tools (falls back to system `bwrap`).

No cleanup is needed (no FUSE daemon to unmount), so just exit with the code:

```rust
let status = runtime::run(&rootfs_path, &config, &extra_args, &volumes, bwrap_path.as_deref(), stdin_is_tty)?;
std::process::exit(status.code().unwrap_or(1));
```

### Task 8 — `run()` wrapper

Wrap all the above logic in a helper `fn run() -> anyhow::Result<()>` and call it from `main()`:

```rust
fn main() {
    if let Err(e) = run() {
        eprintln!("bonk: error: {e:#}");
        std::process::exit(1);
    }
}
```

`{e:#}` prints the full error chain including all `.context()` messages.

---

## Check your understanding

1. Why read from `/proc/self/exe` instead of `std::env::args()[0]`?
2. What is the purpose of the `.bonk-ready` marker file? What problem would
   occur without it if the process was killed mid-extraction?
3. Why does the runner use a content-based hash (first 4 KB + size) as the
   cache key instead of, say, the full SHA-256 of the payload?

---

## Next lesson

In Lesson 10 — the final lesson — you'll implement `mount.rs` and `runtime.rs`:
extracting the SquashFS image via `unsquashfs` and building the full `bwrap`
command that fires up the container.
