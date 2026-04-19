# Lesson 08 — SquashFS Build & Binary Assembly (`pack.rs`)

## What you will build

`crates/bonk-cli/src/pack.rs` — the final step of the bonk build pipeline:

- `pub fn build_squashfs(rootfs: &Path) -> Result<Vec<u8>>` — invoke `mksquashfs`
  to produce a SquashFS image from the rootfs directory, return it as an in-memory buffer
- `pub fn assemble(output: &str, payload: &[u8], config: &ContainerConfig, bwrap_path: Option<&Path>, unsquashfs_path: Option<&Path>) -> Result<usize>` —
  concatenate runner ELF + SquashFS image + static tools (bwrap, unsquashfs) + config JSON + footer into the output binary
- `fn get_tool_bytes(name: &str) -> Result<Vec<u8>>` — locate and read a static tool binary for embedding
- `fn get_runner_bytes() -> Result<Vec<u8>>` — locate and read the `bonk-runner` binary

---

## Concepts

### Shelling out to a build tool

Sometimes the right answer is to delegate to a mature external tool rather than
reimplement its logic in Rust. `mksquashfs` is a C tool that has been producing
correct SquashFS images for decades; reimplementing it would be error-prone and
brittle.

```rust
let status = std::process::Command::new("mksquashfs")
    .arg(rootfs_dir)       // source directory
    .arg(&output_path)     // destination .sqfs file
    .arg("-comp").arg("zstd")  // compression algorithm
    .arg("-noappend")      // overwrite, don't append to existing image
    .arg("-quiet")         // suppress progress output
    .status()
    .context("failed to run mksquashfs — is squashfs-tools installed?")?;

if !status.success() {
    bail!("mksquashfs failed with exit code {:?}", status.code());
}
```

After the command succeeds, read the output file into memory:

```rust
let bytes = std::fs::read(&output_path)?;
```

Then remove the temp file — the bytes will be embedded in the binary.

### Writing a binary file

```rust
use std::fs::File;
use std::io::Write;

let mut file = File::create("output.bin")?;
file.write_all(&runner_bytes)?;
file.write_all(&payload)?;
file.write_all(&config_json)?;
file.write_all(&footer_bytes)?;
```

Each write appends to the file in sequence.

### Tracking byte offsets

To build the `Footer`, you need to know:
- `payload_offset` — how many bytes into the output file does the payload start
  (= the runner ELF size)
- `payload_size` — the length of the payload bytes
- `config_size` — the length of the serialised config JSON bytes
- `bwrap_size` — the length of the embedded static bwrap binary
- `unsquashfs_size` — the length of the embedded static unsquashfs binary

These come directly from `.len()` on each byte buffer.

### Locating static tool binaries

When `bonk` runs, it needs to find the static `bwrap` and `unsquashfs` binaries
to embed in the output. These are pre-built static-pie ELF binaries (compiled
from Alpine musl packages). Three search strategies in order of preference:

1. **`BONK_TOOLS_DIR` env var** — a directory containing `bwrap` and `unsquashfs`
2. **`tools/<arch>/` next to the bonk binary** — architecture-specific (e.g. `tools/x86_64/bwrap`)
3. **`tools/` next to the bonk binary** — flat directory

### Making a file executable on Unix

On Unix, files have permission bits. A file isn't executable unless the execute
bit is set. In Rust:

```rust
#[cfg(unix)]    // only compile this block on Unix systems
{
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(&output_path, perms)?;
}
```

`0o755` in octal = owner can read/write/execute, group and others can read/execute.

### Conditional logging with a `--quiet` flag

Real CLI tools let users suppress progress output. Rather than scattering
`if !quiet { eprintln!(...) }` throughout your code, add a `--quiet` / `-q`
flag to the `Cli` struct and use a simple macro or helper:

```rust
/// Print a progress message to stderr unless --quiet was given.
macro_rules! log {
    ($quiet:expr, $($arg:tt)*) => {
        if !$quiet {
            eprintln!($($arg)*);
        }
    };
}
```

Then replace bare `eprintln!` calls with `log!(cli.quiet, "bonk: exporting image {}...", cli.image);`.

This addresses a common complaint with similar tools (dockerc #51): noisy
progress output that interferes with scripts or piped usage.

### Locating `bonk-runner` at runtime

When `bonk` runs, it needs to find the `bonk-runner` binary to embed it. Three strategies in order of preference:

1. **`BONK_RUNNER` env var** — an absolute path the user can set
2. **Same directory as the current executable** — `std::env::current_exe()?.parent()?.join("bonk-runner")`
3. **`$PATH`** — run `which bonk-runner` and read its output

```rust
// current executable path
let exe = std::env::current_exe()?;
let sibling = exe.parent().unwrap().join("bonk-runner");
if sibling.exists() {
    return Ok(fs::read(sibling)?);
}
```

For the `which` fallback, run `Command::new("which").arg("bonk-runner").output()`,
then parse the stdout as a path.

---

## Tasks

### Task 1 — `build_squashfs`

Implement:

```
pub fn build_squashfs(rootfs: &Path) -> Result<Vec<u8>>
```

1. Choose a temp output path: `rootfs.with_extension("sqfs")`
2. Remove any stale file at that path with `let _ = std::fs::remove_file(&sqfs_path);`
3. Run `mksquashfs <rootfs> <sqfs_path> -comp zstd -noappend -quiet` via `Command::new("mksquashfs")`
4. Check the exit status; bail if it failed
5. Read the `.sqfs` file into a `Vec<u8>` with `std::fs::read`
6. Remove the temp file
7. Return `Ok(bytes)`

> **Hint:** Use `.context("failed to run mksquashfs — is squashfs-tools installed?")` on the
> `.status()` call to give the user a clear error when the tool is missing.

### Task 2 — `get_tool_bytes`

Implement a private function:

```
fn get_tool_bytes(name: &str, override_path: Option<&Path>) -> Result<Vec<u8>>
```

Locate a static tool binary (`bwrap` or `unsquashfs`) for embedding. If `override_path` is `Some`, read that file directly. Otherwise try in order:
1. Check `BONK_TOOLS_DIR` env var — look for `<dir>/<name>`
2. Look for `tools/<arch>/<name>` next to the bonk binary (where `<arch>` = `std::env::consts::ARCH`)
3. Look for `tools/<name>` next to the bonk binary (flat layout)
4. If all three fail, `bail!` with a message explaining where to place the tools

### Task 3 — `get_runner_bytes`

Implement a private function:

```
fn get_runner_bytes() -> Result<Vec<u8>>
```

Try in order:
1. Read `BONK_RUNNER` env var; if set, read that file with `fs::read` and return it
2. Find the current executable's directory with `std::env::current_exe()`, look
   for `bonk-runner` as a sibling, return it if it exists
3. Run `which bonk-runner` as a subprocess, parse the output path (trim whitespace
   with `.trim()`), return the file contents
4. If all three fail, `bail!` with a message like `"bonk-runner not found — run: cargo build --release"`

### Task 4 — `assemble`

Implement:

```
pub fn assemble(output: &str, payload: &[u8], config: &ContainerConfig, bwrap_path: Option<&Path>, unsquashfs_path: Option<&Path>) -> Result<usize>
```

1. Load runner bytes with `get_runner_bytes()`
2. Load tool binaries with `get_tool_bytes("bwrap", bwrap_path)` and `get_tool_bytes("unsquashfs", unsquashfs_path)`
3. Serialise `config` to JSON bytes with `serde_json::to_vec(config)?`
4. Build the `Footer`:
   - `payload_offset = runner_bytes.len() as u64`
   - `payload_size = payload.len() as u64`
   - `config_size = config_json.len() as u64`
   - `bwrap_size = bwrap.len() as u64`
   - `unsquashfs_size = unsquashfs.len() as u64`
5. Create (or truncate) the output file with `File::create(output)`
6. Write in sequence: runner, payload, bwrap, unsquashfs, config_json, `footer.to_bytes()`
7. On Unix, set permissions to `0o755`
8. Compute total size: sum of all section lengths
9. Return `Ok(total_size)`

### Task 5 — Add `--quiet` flag to the CLI

In `main.rs`, add a `--quiet` / `-q` flag to the `Cli` struct:

```rust
/// Suppress progress output
#[arg(short, long)]
quiet: bool,
```

Define the `log!` macro as shown in the Concepts section, and replace all
bare `eprintln!` progress calls with `log!(cli.quiet, ...)`. Error messages
should still always print — only progress/status messages should be suppressed.

### Task 6 — Wire it up

In `main.rs`:

```rust
let payload = pack::build_squashfs(&rootfs_path)?;
let total = pack::assemble(&output_path, &payload, &config)?;
eprintln!("bonk: wrote {} ({})", output_path, bonk_common::human_size(total));
```

### Task 7 — Verify

```bash
# Install squashfs-tools if needed
sudo apt install squashfs-tools

cargo build --release
./target/release/bonk alpine:latest -o ./my-alpine
ls -lh ./my-alpine  # should be a single executable file
file ./my-alpine    # should show ELF executable
```

---

## Check your understanding

1. Why is it correct for `mksquashfs` to fail if the tool is not installed, rather than
   bonk silently producing a broken binary?
2. The `payload_offset` in the footer equals `runner_bytes.len()`. Why? What does
   the runner use this for?
3. Why do we remove the temporary `.sqfs` file after reading it into memory?

---

## Next lesson

In Lesson 09 you'll build `bonk-runner/src/main.rs` — the code that runs when a
user executes the generated binary. It reads itself from disk, parses the footer,
extracts the SquashFS image to a cache directory via `unsquashfs`, and
dispatches to the runtime.
