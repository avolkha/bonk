# Lesson 10 — Container Runtime (`mount.rs` + `runtime.rs`)

## What you will build

The final two files of `bonk-runner`:

- `crates/bonk-runner/src/mount.rs` — extract the embedded SquashFS payload to a
  native directory via embedded `unsquashfs` (or system `unsquashfs` if tools are not embedded)
- `crates/bonk-runner/src/runtime.rs` — build and execute the `bwrap` command that runs the container,
  using the embedded `bwrap` binary if available

After this lesson, the full rebuild is complete. You should be able to
`cargo build --release`, copy both binaries to `$PATH`, and run `bonk alpine:latest` end-to-end.
The output binary is fully self-contained — no runtime dependencies beyond a Linux kernel.

---

## Concepts

### Extracting a SquashFS image with `unsquashfs`

`unsquashfs` is a command-line tool (from the `squashfs-tools` package) that
extracts a SquashFS image to a regular directory on disk. Unlike `squashfuse`
(a FUSE daemon that mounts the image), `unsquashfs` is a one-shot process:

```rust
let status = std::process::Command::new("unsquashfs")
    .arg("-f")          // force overwrite
    .arg("-d")          // destination directory
    .arg(dest_path)
    .arg(sqfs_path)     // the .sqfs file to extract
    .stdout(Stdio::null())   // suppress file listing
    .stderr(Stdio::piped())  // capture errors
    .status()
    .context("failed to run unsquashfs — is squashfs-tools installed?")?;
```

This runs synchronously — when it returns, the rootfs is fully extracted and
ready to use. No daemon to manage, no polling for mount readiness, no cleanup
on exit.

### Why extract instead of FUSE mount?

An earlier design used `squashfuse` to mount the SquashFS image via FUSE.
This avoided writing the extracted rootfs to disk but added ~20 ms of FUSE
overhead per invocation, plus significant slowdowns on file-heavy workloads
(Python imports, DuckDB startup) where every `open()` / `stat()` syscall went
through the FUSE kernel path. Extracting once and reusing the native directory
eliminates this overhead entirely.

### Why read-only rootfs matters

bwrap uses `--overlay-src rootfs --tmp-overlay /` (on bwrap 0.9+) to create
an overlay filesystem: the extracted rootfs is the read-only lower layer, and
writes go to a temporary upper layer that is discarded when the container exits.
This gives each invocation a clean slate — the image layer stays immutable.

### `splitn` — splitting with a limit

`str::split` splits on every occurrence of a delimiter. `splitn(n, delim)` stops after at most `n` parts:

```rust
"host:guest:ro".split(':').collect::<Vec<_>>()
// → ["host", "guest", "ro"]

"host:guest:ro".splitn(3, ':').collect::<Vec<_>>()
// → ["host", "guest", "ro"]      (same here)

"host:/some:path:with:colons".splitn(3, ':')
// → ["host", "/some", "path:with:colons"]  ← third part is not split further
```

For volume specs, `splitn(3, ':')` ensures a host path like `/weird:name` in the guest side doesn't incorrectly split.

### `std::fs::canonicalize`

```rust
let abs_path = std::fs::canonicalize("./relative/path")?;
```

Resolves relative paths, symlinks, and `.` / `..` components to an absolute path. Returns `Err` if the path doesn't exist.

Use this on the *host* side of a volume mount to ensure the path exists and is absolute before passing it to `bwrap`.

### Building a `bwrap` command

`bwrap` is a lightweight Linux sandboxing tool. You pass it a series of flag-pairs that describe the container environment:

```
bwrap
  --bind <host-path> <container-path>    # bind-mount (read/write)
  --ro-bind <host-path> <container-path> # bind-mount (read-only)
  --dev /dev                             # mount a devtmpfs
  --proc /proc                           # mount procfs
  --tmpfs /tmp                           # fresh writable tmpfs
  --unshare-all                          # isolate all Linux namespaces
  --share-net                            # except re-share the network namespace
  --uid 0 --gid 0                        # run as root inside the container
  --hostname bonk                        # container hostname
  --clearenv                             # drop all host env vars
  --setenv KEY VALUE                     # set one env var
  --chdir /app                           # set working directory
  -- program arg1 arg2                   # the command to run
```

Build this with `Command::new("bwrap")` and successive `.arg(...)` calls:

```rust
let mut cmd = Command::new("bwrap");
cmd.arg("--overlay-src").arg(rootfs).arg("--tmp-overlay").arg("/");  // rootfs = extracted directory
cmd.arg("--proc").arg("/proc");
// ...
cmd.arg("--").args(&command_parts);
let status = cmd.status()?;
return Ok(status);  // caller handles exit and unmount
```

### Docker ENTRYPOINT + CMD semantics

Docker defines the container's startup command via two optional fields:

| ENTRYPOINT | extra args given at runtime | CMD | What runs |
|---|---|---|---|
| `["python3"]` | (none) | `["app.py"]` | `python3 app.py` |
| `["python3"]` | `["-c", "print(1)"]` | anything | `python3 -c "print(1)"` |
| (empty) | `["bash"]` | anything | `bash` |
| (empty) | (none) | `["bash"]` | `bash` |
| (empty) | (none) | (empty) | `/bin/sh` |

Rules:
- ENTRYPOINT always comes first if set
- Runtime extra args replace CMD (not append to it)
- Fall back to CMD if no extra args are given
- Fall back to `/bin/sh` if everything is empty

### Environment variable splitting

The config `env` field is a `Vec<String>` where each entry is `"KEY=VALUE"`. To pass these to `bwrap --setenv`, split on the first `=`:

```rust
for kv in &config.env {
    if let Some((k, v)) = kv.split_once('=') {
        cmd.arg("--setenv").arg(k).arg(v);
    }
}
```

`split_once` splits on the first occurrence only — important for values that contain `=`.

---

## Tasks

### Task 1 — `extract_rootfs`

In `src/mount.rs`, implement a single function:

```
pub fn extract_rootfs(payload: &[u8], dest: &Path, unsquashfs_bin: Option<&Path>) -> Result<()>
```

1. Write `payload` to a temporary file (`dest.with_extension("sqfs")`)
2. Determine the unsquashfs command: use `unsquashfs_bin` if provided (embedded tool), otherwise fall back to `"unsquashfs"` from PATH
3. Run `<unsquashfs> -f -d <dest> <sqfs_path>` with stdout suppressed and stderr piped
4. Check the exit status — bail with a context message including stderr if non-zero
5. Delete the temporary `.sqfs` file
6. Return `Ok(())`

This is the entire file — about 40 lines including imports.

### Task 2 — `VolumeMount` struct

In `src/runtime.rs`, define a `pub struct VolumeMount` with three fields:

- `host: String` — the absolute path on the host
- `guest: String` — the path inside the container (must be absolute)
- `read_only: bool`

### Task 3 — `parse_volume`

Implement:

```
pub fn parse_volume(spec: &str) -> Result<VolumeMount>
```

Rules:
1. Split `spec` with `splitn(3, ':')` into parts
2. Bail if fewer than 2 parts, or if host/guest are empty
3. Bail if the guest path doesn't start with `'/'`
4. Canonicalize the host path with `fs::canonicalize` — bail if it doesn't exist
5. The optional third part is `"ro"` for read-only; anything else (or absent) is read-write
6. Return `Ok(VolumeMount { host, guest, read_only })`

### Task 4 — Update the Lesson 09 arg parser

In `bonk-runner/src/main.rs`, for each `-v` argument, call `runtime::parse_volume(spec)?` and push the result into the `volumes` vec.

### Task 5 — `resolve_command`

Implement a private function:

```
fn resolve_command(config: &ContainerConfig, extra_args: &[String]) -> Vec<String>
```

Implement the ENTRYPOINT + CMD logic from the table in the Concepts section. The result is a `Vec<String>` — the flat list of strings to pass to `bwrap` after `--`.

> **Hint:** Use `.is_empty()` to check whether entrypoint, extra_args, or cmd are empty. Build the result by chaining iterators or with a series of `extend` calls.

### Task 6 — `run`

Implement:

```
pub fn run(
    rootfs: &Path,
    config: &ContainerConfig,
    extra_args: &[String],
    volumes: &[VolumeMount],
    bwrap_bin: Option<&Path>,
) -> Result<ExitStatus>
```

Build the `bwrap` command step by step:

1. Determine the `bwrap` binary: use `bwrap_bin` if provided (embedded tool), then check `BONK_BWRAP` env var, then fall back to `"bwrap"` from PATH
2. Probe for bwrap overlay support: spawn `bwrap --overlay-src / --tmp-overlay / -- true` and check the exit code. If it succeeds, use overlay mode; otherwise fall back to `--bind rootfs /`
3. Overlay mode: `--overlay-src rootfs / --tmp-overlay /` — read-only lower layer + disposable upper
4. Fallback mode: `--bind rootfs /` — direct read-write access (bwrap < 0.9)
3. `--dev /dev` — expose device nodes
4. `--proc /proc` — mount procfs
5. `--tmpfs /tmp` and `--tmpfs /run`
6. For each volume: `--bind host guest` or `--ro-bind host guest`
7. `--unshare-all` and `--share-net`
8. `--uid 0 --gid 0`
9. `--hostname bonk`
10. `--ro-bind /etc/resolv.conf /etc/resolv.conf` (for DNS)
11. `--clearenv`
12. For each `KEY=VALUE` in `config.env`: `--setenv KEY VALUE`
13. Pass through `TERM` from the host: `--setenv TERM <value-of-TERM>`
14. `--chdir <config.working_dir>`
15. `--` followed by `resolve_command(config, extra_args)`

Run with `.status()?` and return `Ok(status)`.

The caller (lesson 09's `run()` function) just calls
`std::process::exit(status.code().unwrap_or(1))`.

### Task 7 — End-to-end test

```bash
# Install build prerequisites if needed
sudo apt install squashfs-tools

cargo build --release
cp target/release/bonk target/release/bonk-runner ~/.cargo/bin/

# Ensure static tools are available
# (pre-built in tools/x86_64/ or set BONK_TOOLS_DIR)
ls tools/x86_64/bwrap tools/x86_64/unsquashfs

# Basic test
bonk alpine:latest
./alpine echo "hello from a bonk container"

# Verify embedded tools were extracted
ls /tmp/bonk-*/bin/
# Should show: bwrap  unsquashfs

# Volume mount test
echo "test file" > /tmp/bonk-test.txt
./alpine -v /tmp/bonk-test.txt:/data/test.txt cat /data/test.txt

# Entrypoint test
docker build -t pyduck tests/pyduck/
bonk pyduck -o ./pyduck
./pyduck   # should print DuckDB query result

# Second run (should be faster — rootfs already extracted, no unsquashfs needed)
time ./alpine echo "cached"
```

### Task 8 — Compare against the real implementation

Now that you've built everything, look at the real source code. For each file, compare your implementation to the original. Note:

- Any error handling you added or omitted
- Any edge cases the original handles that yours doesn't
- Any places where your approach is different but equivalent

This is not about being identical — it's about understanding the tradeoffs.

---

## Check your understanding

1. Why does `--unshare-all --share-net` make sense for a container tool? What would break with `--unshare-all` alone?
2. Why does `resolve_command` replace CMD with extra_args rather than appending to CMD?
3. If the user passes `./alpine -v ./data:/data -- bash -c "ls /data"`, trace through the argument parser and `runtime.rs` step by step. What exactly does bwrap receive?

---

## Congratulations

You have rebuilt `bonk` from scratch. Here is what you implemented across 10 lessons:

| Lesson | File | Core Rust concepts |
|---|---|---|
| 01 | Workspace `Cargo.toml` | workspaces, `[[bin]]`, `mod`, `pub` |
| 02 | `bonk-common/lib.rs` | structs, `impl`, traits, `#[derive]`, byte arithmetic, `Option` |
| 03 | `bonk-cli/main.rs` | `Result`, `?`, `anyhow`, `clap` |
| 04 | `image::export_image` | `Command`, env vars, tar extraction |
| 05 | `image::parse_image` | file I/O, `serde_json`, `PathBuf`, `Option` chaining |
| 06 | `flatten::open_layer` | trait objects, `Box<dyn Read>`, `Seek`, magic bytes |
| 07 | `flatten::flatten_layers` | iterators, tar archives, OCI whiteouts, fs operations |
| 08 | `pack.rs` | shelling out (`Command`), binary file assembly, `0o755` permissions |
| 09 | `bonk-runner/main.rs` | self-reading binaries, hashing, manual arg parsing, cache management, tool extraction |
| 10 | `mount.rs` + `runtime.rs` | SquashFS extraction, embedded tools, `bwrap` overlay, ENTRYPOINT logic |
