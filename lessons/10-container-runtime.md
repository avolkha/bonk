# Lesson 10 — Container Runtime (`mount.rs` + `runtime.rs`)

## What you will build

The final two files of `bonk-runner`:

- `crates/bonk-runner/src/mount.rs` — make the SquashFS payload available at `rootfs/`:
  try a kernel squashfs loop-mount (requires root), fall back to `unsquashfs` extraction
- `crates/bonk-runner/src/runtime.rs` — build and execute the `bwrap` command that runs the container,
  using the embedded `bwrap` binary if available

After this lesson, the full rebuild is complete. You should be able to
`cargo build --release`, copy both binaries to `$PATH`, and run `bonk alpine:latest` end-to-end.
The output binary is fully self-contained — no runtime dependencies beyond a Linux kernel.

---

## Concepts

### Tiered rootfs strategy: kernel mount vs. `unsquashfs`

bonk uses a tiered approach to make the rootfs available:

1. **Kernel squashfs loop-mount** (privileged path): `mount -t squashfs -o loop,ro rootfs.sqfs rootfs/`
   - Requires root (or `sudo ./myapp --mount` for first-run setup)
   - Keeps the image in compressed form on disk — no uncompressed copy
   - bwrap overlays an ephemeral upper layer so each run gets a clean slate
   - After the `--mount` setup, all subsequent invocations run unprivileged

2. **`unsquashfs` extraction** (unprivileged fallback): extracts to a plain directory
   - Works without root, same as the original strategy
   - Uses more disk (uncompressed rootfs), but no special privileges needed

`mount_or_extract` tries the kernel mount first, falls back to extraction on failure, and returns `true`
if mounted (so the caller knows to pass `rootfs_readonly: true` to `runtime::run`).

### Detecting an active squashfs mount

On warm runs, the runner needs to verify the kernel mount is still live (it disappears after a reboot).
Parse `/proc/mounts` — each line is `device mountpoint fstype options dump pass`:

```rust
pub fn is_squashfs_mounted(path: &Path) -> bool {
    let Ok(data) = std::fs::read_to_string("/proc/mounts") else { return false; };
    let path_str = path.to_string_lossy();
    data.lines().any(|line| {
        let mut parts = line.splitn(4, ' ');
        let _dev = parts.next();
        let mountpoint = parts.next().unwrap_or("");
        let fstype = parts.next().unwrap_or("");
        mountpoint == path_str.as_ref() && fstype == "squashfs"
    })
}
```

### Why not always use FUSE (`squashfuse`)?

An earlier design used `squashfuse` to mount the SquashFS image via FUSE.
This avoided writing the extracted rootfs to disk but added ~20 ms of FUSE
overhead per invocation, plus significant slowdowns on file-heavy workloads
(Python imports, DuckDB startup) where every `open()` / `stat()` syscall went
through the FUSE kernel path. The kernel squashfs driver has zero FUSE overhead
— it serves files directly from the kernel's VFS layer.

### Why read-only rootfs matters

bwrap uses `--overlay-src rootfs --tmp-overlay /` (on bwrap 0.9+) to create
an overlay filesystem: the extracted rootfs is the read-only lower layer, and
writes go to a temporary upper layer that is discarded when the container exits.
This gives each invocation a clean slate — the image layer stays immutable.

### Securing world-writable paths: the TOCTOU problem

The cache directory lives under `/tmp`, which is world-writable. Any unprivileged
process can create `/tmp/bonk-<hash>` as a symlink pointing anywhere — e.g.
`/etc/passwd` — before `bonk --mount` runs. If the privileged code blindly calls
`std::fs::create_dir_all` or `std::fs::write`, it follows the symlink and
overwrites the target as root. This is a *time-of-check / time-of-use* (TOCTOU)
race.

The fix is to create the directory atomically and, if it already exists, validate
it is a real directory owned by root before writing anything into it:

```rust
pub fn init_cache_dir_as_root(dir: &Path) -> Result<()> {
    match std::fs::create_dir(dir) {
        Ok(()) => {
            // Fresh creation — set strict permissions
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Already exists — verify it is a real root-owned directory
            // Use symlink_metadata (not metadata) so we never follow symlinks
            let meta = dir.symlink_metadata()?;
            anyhow::ensure!(meta.file_type().is_dir(),
                "cache path {} is not a directory (possible symlink attack)", dir.display());
            anyhow::ensure!(meta.uid() == 0,
                "cache dir {} is not owned by root (uid={})", dir.display(), meta.uid());
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("failed to create {}", dir.display())),
    }
}
```

Key points:
- `std::fs::create_dir` (not `create_dir_all`) atomically creates a single directory and fails if it already exists — there is no TOCTOU window between "check if exists" and "create"
- `symlink_metadata` (not `metadata`) returns information about the path itself, not its target — so a symlink shows as `is_symlink()` rather than `is_dir()`
- `MetadataExt::uid()` checks the POSIX owner — only directories created by root have `uid == 0`
- This function belongs in `mount.rs` and is called from the `--mount` path in `main.rs` before any writes into the cache dir

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

### Bundling many parameters into a struct

When a function takes more than ~5 arguments, Clippy flags it with `too_many_arguments`. Rather
than suppressing the lint with `#[allow(...)]`, extract the parameters into a named struct:

```rust
pub struct RunOpts<'a> {
    pub rootfs: &'a std::path::Path,
    pub config: &'a bonk_common::ContainerConfig,
    pub extra_args: &'a [String],
    pub volumes: &'a [VolumeMount],
    pub runtime_env: &'a [String],
    pub bwrap_path: Option<&'a std::path::Path>,
    pub stdin_is_tty: bool,
    pub rootfs_readonly: bool,
}

pub fn run(opts: RunOpts<'_>) -> Result<std::process::ExitStatus> {
    let RunOpts { rootfs, config, extra_args, volumes, runtime_env,
                  bwrap_path, stdin_is_tty, rootfs_readonly } = opts;
    // ...
}
```

This also makes call sites self-documenting — field names replace positional argument order.

The lifetime `'a` on `RunOpts` is needed because it holds references: the compiler must know
that all the borrowed data (`rootfs`, `config`, etc.) lives at least as long as the `RunOpts`
value itself.

```rust
let abs_path = std::fs::canonicalize("./relative/path")?;
```

Resolves relative paths, symlinks, and `.` / `..` components to an absolute path. Returns `Err` if the path doesn't exist.

Use this on the *host* side of a volume mount to ensure the path exists and is absolute before passing it to `bwrap`.

### AppArmor and unprivileged user namespaces

bwrap's rootless path (`--unshare-all`) requires the kernel to create a user namespace
(`clone(CLONE_NEWUSER)`). Ubuntu 23.10+ and other distros with AppArmor 4.0 restrict
this by default (`kernel.apparmor_restrict_unprivileged_userns=1`). Because the embedded
bwrap binary is extracted to a temporary cache directory with no installed AppArmor profile,
the syscall is denied and the container fails to start silently.

The privileged `--mount` path avoids this entirely — it runs bwrap as root using
`--unshare-ipc/pid/uts/cgroup` instead of `--unshare-all`, so no user namespace is created.
On Ubuntu 24.04+ VMs, `sudo ./myapp --mount` followed by unprivileged `./myapp` is the
reliable workaround.

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

### Root vs. rootless UID mapping

When run as a normal user, bwrap creates a user namespace and maps the calling
user to UID 0 inside the container. This works transparently with `--uid 0
--gid 0`.

But when run as **root**, bwrap's `--unshare-user` is redundant (you're already
UID 0), and some bwrap versions refuse it or behave differently. The runner
should detect this and adjust:

```rust
let is_root = unsafe { libc::getuid() } == 0;

if is_root {
    // Already root — don't create a user namespace
    cmd.arg("--unshare-ipc")
       .arg("--unshare-pid")
       .arg("--unshare-uts")
       .arg("--unshare-cgroup");
    // No --unshare-user, no --uid/--gid (already 0)
} else {
    cmd.arg("--unshare-all")
       .arg("--share-net")
       .arg("--uid").arg("0")
       .arg("--gid").arg("0");
}
```

Add `libc` as a dependency in `crates/bonk-runner/Cargo.toml`:

```toml
libc = "0.2"
```

This addresses a class of bugs reported in similar tools (dockerc #44) where
running as root panics due to missing UID mappings.

### TTY-aware stdin handling

Pass the `stdin_is_tty` flag from the argument parser (lesson 09) through to
`runtime::run`. When stdin is not a terminal, the runner should avoid any
terminal-related setup. For bwrap this is straightforward — bwrap doesn't
allocate a PTY by default — but if you ever add OCI runtime support (crun),
you'll need to set `"terminal": false` in the config when `!stdin_is_tty`.

For now, the practical effect is that the runner should **not** pass `--new-session`
to bwrap when stdin is piped, since `--new-session` creates a new session which
detaches from the controlling terminal and can cause signal-handling issues with
piped input.

---

## Tasks

### Task 0 — `init_cache_dir_as_root`

In `src/mount.rs`, implement the security-critical helper described in the Concepts section:

```
pub fn init_cache_dir_as_root(dir: &Path) -> Result<()>
```

- Use `std::fs::create_dir` (not `create_dir_all`) for atomic creation
- On `AlreadyExists`: call `dir.symlink_metadata()` and verify `is_dir()` and `uid() == 0`
- On fresh creation: set permissions to `0o755`
- Add `use std::os::unix::fs::{MetadataExt, PermissionsExt}` for `uid()` and `from_mode`

This function is called from `main.rs` at the top of the `--mount` path, before any writes into the cache dir.

### Task 1 — `mount_or_extract`, `try_squashfs_mount`, `is_squashfs_mounted`

In `src/mount.rs`, implement three public functions:

```
pub fn mount_or_extract(
    payload: &[u8],
    sqfs_path: &Path,
    dest: &Path,
    unsquashfs: Option<&Path>,
) -> Result<bool>
```

1. Write `payload` to `sqfs_path`
2. Create `dest` directory
3. Call `try_squashfs_mount(sqfs_path, dest)` — if it succeeds, return `Ok(true)`
4. On failure, call the private `extract_via_unsquashfs(sqfs_path, dest, unsquashfs)` helper
5. Delete `sqfs_path` after successful extraction
6. Return `Ok(false)`

```
pub fn try_squashfs_mount(sqfs_path: &Path, dest: &Path) -> Result<()>
```

Run `mount -t squashfs -o loop,ro <sqfs_path> <dest>` and bail if the exit status is non-zero.

```
pub fn is_squashfs_mounted(path: &Path) -> bool
```

Parse `/proc/mounts` and return `true` if any line has `mountpoint == path && fstype == "squashfs"`.

Also implement the private helper `extract_via_unsquashfs(sqfs_path, dest, unsquashfs)` that runs
`<unsquashfs> -f -d <dest> <sqfs_path>` with stdout/stderr suppressed.

### Task 1b — Unit tests for `mount.rs`

Add a `#[cfg(test)]` module to `mount.rs` with two tests. Because you can't trigger a real kernel squashfs mount in a test environment, fake the `unsquashfs` binary with a small shell script:

```rust
fn fake_unsquashfs(dir: &Path, script_body: &str) -> std::path::PathBuf {
    let script = dir.join("fake-unsquashfs.sh");
    fs::write(&script, format!("#!/bin/sh\nset -eu\n{script_body}\n")).unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}
```

**Test 1 — extraction happy path:**
- Create a `tempdir`; define `sqfs_path` and `dest` inside it
- Pass a fake unsquashfs that writes a sentinel file to its `$3` arg (the dest dir)
- Assert `mount_or_extract` returns `Ok(false)` (not mounted)
- Assert the sentinel file exists
- Assert `sqfs_path` was deleted after extraction

**Test 2 — extraction failure propagates:**
- Pass a fake unsquashfs that exits with a non-zero code
- Assert the returned `Err` contains `"unsquashfs failed with status"`

These tests verify the fallback path end-to-end without requiring root or a real squashfs image.
Add `tempfile = "3"` to `[dev-dependencies]` in `crates/bonk-runner/Cargo.toml`.

### Task 2 — `VolumeMount` struct

In `src/runtime.rs`, define a `pub struct VolumeMount` with three fields:

- `host: String` — the absolute path on the host
- `guest: String` — the path inside the container (must be absolute)
- `read_only: bool`

### Task 3 — `VolumeMount::parse`

Implement an infallible associated function:

```
impl VolumeMount {
    pub fn parse(spec: &str) -> Self
}
```

Rules:
1. Split `spec` with `splitn(3, ':')` into at most three parts
2. Take `parts[0]` as `host` and `parts[1]` as `guest` (default to `""` if absent)
3. `read_only` is `true` iff `parts[2] == "ro"`
4. Return `VolumeMount { host, guest, read_only }`

This is intentionally forgiving — validation (e.g. checking that paths are non-empty
and absolute) is handled at the call site in `main.rs`.

### Task 4 — Update the Lesson 09 arg parser

In `bonk-runner/src/main.rs`, for each `-v` argument, call `runtime::VolumeMount::parse(spec)` and push the result into the `volumes` vec.

### Task 5 — `resolve_command`

Implement a private function:

```
fn resolve_command(config: &ContainerConfig, extra_args: &[String]) -> Vec<String>
```

Implement the ENTRYPOINT + CMD logic from the table in the Concepts section. The result is a `Vec<String>` — the flat list of strings to pass to `bwrap` after `--`.

> **Hint:** Use `.is_empty()` to check whether entrypoint, extra_args, or cmd are empty. Build the result by chaining iterators or with a series of `extend` calls.

### Task 6 — `run`

Implement using the `RunOpts` struct described in the Concepts section:

```
pub fn run(opts: RunOpts<'_>) -> Result<ExitStatus>
```

Destructure `opts` at the top of the function body. Build the `bwrap` command step by step:

1. Determine the `bwrap` binary: use `bwrap_path` if provided (embedded tool), then check `BONK_BWRAP` env var, then fall back to `which::which("bwrap")` from PATH
2. Probe for bwrap overlay support: run `bwrap --help` and check whether the output contains `"--overlay-src"`. This avoids a live mount probe (which can fail with `EINVAL` on some kernels due to `userxattr`). If `rootfs_readonly` is `true` and overlay is not available, bail with a clear error.
3. Overlay mode: `--overlay-src rootfs / --tmp-overlay /` — read-only lower layer + disposable upper
4. Fallback bind mode (when not `rootfs_readonly` and overlay unavailable): `--bind rootfs /`
5. `--dev /dev`, `--proc /proc`, `--tmpfs /tmp`, `--tmpfs /run`
6. `--hostname bonk`, `--ro-bind /etc/resolv.conf /etc/resolv.conf`
7. **Namespace and UID handling (root-aware):** detect with `unsafe { libc::getuid() } == 0`
   - If **rootless**: `--unshare-all --share-net --uid 0 --gid 0`
   - If **root**: `--unshare-ipc --unshare-pid --unshare-uts --unshare-cgroup`
8. For each volume: `--bind host guest` or `--ro-bind host guest`
9. **`--clearenv` must come before any `--setenv`** — bwrap processes args left-to-right
10. For each `KEY=VALUE` in `config.env`: `--setenv KEY VALUE` (split on first `=`)
11. Pass through `TERM` from the host: `--setenv TERM <value>`
12. For each `KEY=VALUE` in `runtime_env`: `--setenv KEY VALUE` — these come **after** image vars so they override image defaults
13. Only add `--new-session` if `stdin_is_tty` is `true`
14. `--chdir <config.working_dir>`
15. `--` followed by `resolve_cmd(config, extra_args)?`

Run with `.status()` and return `Ok(status)`.

### Task 7 — End-to-end test

> **AppArmor note:** On Ubuntu 23.10+ and other distros with AppArmor 4.0, unprivileged user
> namespaces are restricted by default. The embedded bwrap binary has no AppArmor profile, so
> `./myapp` will fail silently on affected VMs. Run `sudo ./myapp --mount` once to set up the
> kernel loop-mount (which bypasses user namespaces), then all subsequent `./myapp` invocations
> run unprivileged without hitting the restriction.

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

# Quiet mode — no progress output
bonk -q alpine:latest -o alpine-quiet
./alpine-quiet -q echo "silent run"

# Piped stdin (verifies TTY detection — dockerc #52)
echo "hello" | ./alpine cat
# Should print "hello" without crashing

# Verify it works when run as root (dockerc #44)
sudo ./alpine id
# Should print uid=0(root) without panic

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
4. Why do we skip `--unshare-user` when running as root? What would happen if we kept it?
5. Why is `--new-session` only safe when stdin is a terminal? What goes wrong with piped input?
6. Why must `init_cache_dir_as_root` use `symlink_metadata` rather than `metadata`? What attack does this prevent? Why is `std::fs::create_dir` (not `create_dir_all`) the right call here?

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
| 09 | `bonk-runner/main.rs` | self-reading binaries, hashing, `clap` derive, struct decomposition, cache management, tool extraction |
| 10 | `mount.rs` + `runtime.rs` | kernel squashfs loop-mount, `unsquashfs` fallback, embedded tools, `bwrap` overlay, ENTRYPOINT logic |
