# Lesson 09 — Self-Reading Binaries & Caching (`bonk-runner/main.rs`)

## What you will build

`crates/bonk-runner/src/main.rs` — the stub binary that gets embedded in every bonk output. When the user runs `./alpine`, this code runs. It:

1. Reads its own bytes from `/proc/self/exe`
2. Parses the footer from the last 56 bytes of the file
3. Locates and parses the embedded `config.json`
4. Computes a cache key and checks if the rootfs is already ready
5. Extracts embedded tool binaries (bwrap, unsquashfs) to cache directory
6. Makes the rootfs available at `/tmp/bonk-<hash>/rootfs/` — kernel squashfs loop-mount if privileged (`--mount`), otherwise `unsquashfs` extraction — both cached; skipped on warm runs
7. Parses user-provided flags (`-e`, `-v`, `--`) from `argv` using `clap`
8. Execs embedded bwrap over the rootfs and exits with the container's exit code

The file is structured as a set of focused types and functions that `main` orchestrates — `main` itself is only ~20 lines.

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

After making the rootfs ready, write a "marker" file to signal which strategy was used and that setup completed successfully. The content is either `"mount"` or `"extract"`:

```rust
let marker = cache_dir.join(".bonk-ready");
if !marker.exists() {
    let _ = std::fs::remove_dir_all(&cache_dir);
    std::fs::create_dir_all(&cache_dir)?;
    let mounted = mount::mount_or_extract(payload, &sqfs_path, &rootfs_path, unsquashfs.as_deref())?;
    std::fs::write(&marker, if mounted { "mount" } else { "extract" })?;
}
```

On future runs, reading the marker tells the runner whether the rootfs is a live kernel mount (needs
a `/proc/mounts` check + re-mount if it disappeared after reboot) or a plain extracted directory
(just use it directly). Without this distinction the runner might try to overlay a directory that
is no longer a squashfs mount, leading to a bwrap permission error.

### Argument parsing with `clap`

`bonk-runner` uses `clap` with the `derive` feature, the same as `bonk-cli`. Annotate a struct and clap generates the full parser — including `--help` and `--version` — automatically:

```rust
use clap::Parser;

/// A bonk-generated container binary.
#[derive(Parser)]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Args {
    /// Set an environment variable inside the container.
    #[arg(short = 'e', long = "env", value_name = "KEY=VALUE")]
    runtime_env: Vec<String>,

    /// Bind-mount a host path into the container.
    #[arg(short = 'v', long = "volume", value_name = "HOST:GUEST[:ro]")]
    volumes: Vec<String>,

    /// Mount the embedded squashfs rootfs (requires root).
    #[arg(long)]
    mount: bool,

    /// Suppress progress output.
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Command to run inside the container (overrides image's default CMD).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    extra_args: Vec<String>,
}
```

`trailing_var_arg = true` makes clap collect everything after the last known flag as positional args, including flags that start with `-`. This matches the Docker convention where `./alpine -c "code"` passes `-c "code"` to the container's entrypoint, not to the runner itself.

> **Size note:** `clap` with `derive` adds ~400 KB to the stripped binary (749 KB → ~1.1 MB). For bonk-runner this is acceptable — the tools embedded alongside it (`bwrap` + `unsquashfs`) total ~1.2 MB, so the runner itself is already not the bottleneck. If absolute minimum stub size were required, manual parsing would be the answer.

### TTY detection with `std::io::IsTerminal`

When a container binary is invoked via a pipe (`echo hi | ./alpine ...`), stdin
is not a terminal. Many container runtimes default to `terminal: true` in the
OCI config, which causes a `tcgetattr` failure when stdin is not a TTY
(dockerc #52). Detect this at runtime:

```rust
use std::io::IsTerminal;

let stdin_is_tty = std::io::stdin().is_terminal();
```

The runner passes this to `runtime::run` so it can decide whether to pass `--new-session` to bwrap.

### Decomposing a large `main` into focused types

When `main` grows beyond ~50 lines it becomes hard to read and reason about. The solution is not to add comments — it's to extract cohesive units into named types and functions:

```rust
// Owns parsed CLI flags
struct Args { ... }

// Owns the executable bytes, footer, and config
struct BinaryData {
    exe_data: Vec<u8>,
    footer: Footer,
    config: ContainerConfig,
}
impl BinaryData {
    fn load() -> Result<Self> { ... }   // reads /proc/self/exe
    fn payload(&self) -> &[u8] { ... }  // slice from footer offsets
}

// Groups all cache paths derived from the payload hash
struct CachePaths { dir, rootfs, sqfs, marker: PathBuf }
impl CachePaths {
    fn new(binary: &BinaryData) -> Self { ... }
}

// Privileged first-run setup
fn run_mount_setup(cache: &CachePaths, payload: &[u8], quiet: bool) -> Result<()> { ... }

// Warm/cold start: ensures rootfs is ready, returns rootfs_readonly
fn prepare_rootfs(binary: &BinaryData, cache: &CachePaths, ...) -> Result<bool> { ... }
```

With these in place, `main` becomes a ~20-line orchestrator:

```rust
fn main() -> Result<()> {
    let args = Args::parse();
    let binary = BinaryData::load()?;
    let cache = CachePaths::new(&binary);

    if args.mount {
        return run_mount_setup(&cache, binary.payload(), args.quiet);
    }

    let rootfs_readonly = prepare_rootfs(&binary, &cache, &bin_name, args.quiet)?;
    let tools = extract_embedded_tools(&binary.footer, &binary.exe_data, &cache.dir)?;
    let status = runtime::run(runtime::RunOpts { ... })?;
    std::process::exit(status.code().unwrap_or(1));
}
```

Each type captures a distinct phase. Naming the phases makes the control flow obvious — you can read `main` top-to-bottom without scrolling through 200 lines of setup logic.

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
clap = { version = "4", features = ["derive"] }
serde_json = "1"
```

Note: no `tar` or `zstd` — the runner no longer decompresses anything itself.

### Task 0 — Declare modules

In `bonk-runner/src/main.rs`, declare two modules: `mod mount;` and `mod runtime;`. Create empty placeholder files for them.

---

## Tasks

### Task 1 — Define the `Args` struct

Define the `Args` struct using `#[derive(Parser)]` as shown in the Concepts section. You'll need `VolumeMount` from `bonk-runner::runtime` — declare a placeholder `pub struct VolumeMount` in `runtime.rs` for now.

Use `trailing_var_arg = true` on `extra_args` so positional arguments (the container command) are collected after all flags, without requiring `--` as a separator.

Note that `clap` now generates `--help` and `--version` for free — no manual printing needed.

### Task 2 — Parse arguments in `main`

Call `Args::parse()` at the top of `main`. The `volumes` field will be `Vec<String>` at this point — convert each spec to a `VolumeMount` by calling `runtime::VolumeMount::parse(spec)` when building `RunOpts`.

Derive `stdin_is_tty` inline in `main`:

```rust
let stdin_is_tty = std::io::stdin().is_terminal();
```

Use the same `log!` macro pattern from lesson 08 to guard progress messages behind `!quiet`.

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

### Task 6 — Cache check and rootfs setup

Compute the cache key by hashing the first 4 KB of the payload and the payload size.

Define the cache layout:
```
/tmp/bonk-<hash>/
    bin/            ← extracted embedded tools (bwrap, unsquashfs)
    rootfs/         ← squashfs mountpoint or extracted rootfs directory
    rootfs.sqfs     ← squashfs file (kept when loop-mounted; removed after extraction)
    .bonk-ready     ← marker: content is "mount" or "extract"
```

The `--mount` flag enables a privileged first-run setup path (requires root / `sudo`):
- **Before any writes**, call `mount::init_cache_dir_as_root(&cache_dir)?` — this atomically creates or validates the cache dir, protecting against TOCTOU symlink attacks in the world-writable `/tmp`. (Implemented in Lesson 10.)
- Write the `.sqfs` to the cache dir, create `rootfs/` and `bin/`
- Call `mount::try_squashfs_mount(&sqfs_path, &rootfs_path)` to kernel loop-mount it
- Write marker `"mount"`
- Chown `bin/`, `rootfs.sqfs`, and the marker back to `SUDO_UID:SUDO_GID` (recursively), then chown `cache_dir` itself (non-recursively) so unprivileged runs can access and clean up the cache without touching the squashfs mountpoint

On a normal (non-`--mount`) cold start:
1. `remove_dir_all` the cache dir (clean any partial state)
2. Extract embedded tools into `bin/` as before
3. Call `mount::mount_or_extract(payload, &sqfs_path, &rootfs_path, unsquashfs.as_deref())?`
   which tries a kernel mount first and falls back to `unsquashfs` extraction; returns `true` if mounted
4. Write marker `"mount"` or `"extract"` depending on the return value

On a warm run, read the marker:
- `"mount"` → check `mount::is_squashfs_mounted(&rootfs_path)`; re-mount if it’s gone (reboot)
- `"extract"` → rootfs dir is already on disk, nothing to do
- absent/other → treat as cold start

The `rootfs_readonly` flag passed to `runtime::run` should be `true` when the marker is `"mount"`,
because bwrap must use an overlay filesystem over a read-only squashfs mountpoint.

Create a helper function `extract_embedded_tools` that takes the footer, exe
data, and cache dir, and returns `Result<EmbeddedTools>` containing optional
paths to the bwrap and unsquashfs binaries.

### Task 7 — Launch

Call `runtime::run` with a `RunOpts` struct — this returns a `Result<std::process::ExitStatus>`.
`rootfs_readonly` is `true` when the marker was `"mount"` (squashfs loop-mounted); bwrap must use an overlay filesystem in that case.

No cleanup is needed (no FUSE daemon to unmount), so just exit with the code:

```rust
let status = runtime::run(runtime::RunOpts {
    rootfs: &rootfs_path,
    config: &config,
    extra_args: &extra_args,
    volumes: &volumes,
    runtime_env: &runtime_env,
    bwrap_path: tools.bwrap.as_deref(),
    stdin_is_tty,
    rootfs_readonly,
})?;
std::process::exit(status.code().unwrap_or(1));
```

### Task 8 — Decompose `main` into focused types

Once all the logic is working, refactor `main` using the struct-based decomposition described in the Concepts section. Extract:

- `BinaryData` — owns `exe_data`, `footer`, `config`; has `load() -> Result<Self>` and `payload() -> &[u8]`
- `CachePaths` — groups `dir`, `rootfs`, `sqfs`, `marker`; has `new(binary: &BinaryData) -> Self`
- `run_mount_setup(cache, payload, quiet)` — the entire `--mount` branch
- `prepare_rootfs(binary, cache, bin_name, quiet) -> Result<bool>` — the `match prior_strategy` block

After this refactor, `main` should be around 20 lines with no inline logic — just calling into the above.

Verify with `cargo build --package bonk-runner` (no warnings) and re-running `tests/e2e.sh`.

---

## Check your understanding

1. Why read from `/proc/self/exe` instead of `std::env::args()[0]`?
2. What is the purpose of the `.bonk-ready` marker file? What problem would
   occur without it if the process was killed mid-extraction?
3. Why does the runner use a content-based hash (first 4 KB + size) as the
   cache key instead of, say, the full SHA-256 of the payload?
4. `trailing_var_arg = true` on `extra_args` means `-v` after a positional argument is consumed as part of the command, not as a volume flag. What does this mean for users? Is this the right trade-off?
5. Why does `BinaryData::payload()` return a `&[u8]` slice rather than a `Vec<u8>`? What would the cost of returning a `Vec<u8>` be?

---

## Next lesson

In Lesson 10 — the final lesson — you'll implement `mount.rs` and `runtime.rs`:
the tiered squashfs mount strategy (kernel loop-mount with `unsquashfs` fallback) and the full `bwrap`
command that fires up the container.
