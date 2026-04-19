# bonk — Docker for cavemen

> **A guided Rust project.** Build a real tool from scratch, one lesson at a time.

Smash a Docker image into a single self-contained executable. No daemon. No registry. No YAML. Just a binary.

```
bonk alpine:latest
./alpine echo "ooga booga"
```

Ship anywhere Linux runs — zero runtime dependencies on the target:

```
bonk python:3.12-slim -o python3
scp python3 someserver:
ssh someserver ./python3 -c "print('hello')"
```

`bwrap` and `unsquashfs` are embedded in the output binary — the target machine needs nothing pre-installed.

---

## How it works

1. `bonk` exports a Docker image, flattens its layers, and builds a SquashFS image with `mksquashfs`
2. Appends the SquashFS image + static tool binaries (`bwrap`, `unsquashfs`) + container config to a small runner binary
3. The output is a single self-contained executable that extracts the rootfs on first run and execs via the embedded `bwrap`

```
┌──────────────────────┐
│  bonk-runner ELF     │  small Rust binary
├──────────────────────┤
│  rootfs.sqfs         │  SquashFS image (zstd-compressed)
├──────────────────────┤
│  bwrap (static)      │  ~134 KB embedded container runtime
├──────────────────────┤
│  unsquashfs (static) │  ~1.1 MB embedded SquashFS extractor
├──────────────────────┤
│  config.json         │  entrypoint, cmd, env, workdir
├──────────────────────┤
│  footer (56 bytes)   │  offsets + sizes + magic number
└──────────────────────┘
```

At runtime:
1. Runner reads itself (`/proc/self/exe`), locates payload via the footer
2. Extracts `bwrap` + `unsquashfs` to `/tmp/bonk-<hash>/bin/` (cached)
3. Extracts rootfs to `/tmp/bonk-<hash>/rootfs/` via `unsquashfs` (cached — skipped on warm runs)
4. Execs `bwrap` over the extracted rootfs directory
5. Exits with the container's exit code

---

## Install

### Download a release (recommended)

Pre-built static binaries are available for every [GitHub Release](https://github.com/avolkha/bonk/releases).

**x86_64 (Linux)**
```bash
VERSION=v0.1.0
curl -fsSL https://github.com/avolkha/bonk/releases/download/${VERSION}/bonk-x86_64-unknown-linux-musl -o bonk
curl -fsSL https://github.com/avolkha/bonk/releases/download/${VERSION}/bonk-runner-x86_64-unknown-linux-musl -o bonk-runner
chmod +x bonk bonk-runner
sudo mv bonk bonk-runner /usr/local/bin/
```

**ARM64 (Linux)**
```bash
VERSION=v0.1.0
curl -fsSL https://github.com/avolkha/bonk/releases/download/${VERSION}/bonk-aarch64-unknown-linux-musl -o bonk
curl -fsSL https://github.com/avolkha/bonk/releases/download/${VERSION}/bonk-runner-aarch64-unknown-linux-musl -o bonk-runner
chmod +x bonk bonk-runner
sudo mv bonk bonk-runner /usr/local/bin/
```

Both binaries are fully static (musl) — no runtime dependencies on the target machine.

### Build from source

```bash
cargo build --release
cp target/release/bonk target/release/bonk-runner ~/.cargo/bin/
```

Both `bonk` and `bonk-runner` must be locatable — either in the same directory or on `$PATH`.

### Prerequisites

**Build machine:**
- Rust toolchain
- Docker (for `docker save`)
- `mksquashfs` (from `squashfs-tools`)

```bash
# Ubuntu/Debian
sudo apt install squashfs-tools
```

**Target machine:** Linux with user namespaces enabled. That's it.

---

## Usage

```bash
# Output name derived from image name
bonk alpine:latest          # → ./alpine

# Custom output path
bonk -o myapp my_image:latest

# Run the generated binary
./alpine                    # launches default CMD
./alpine echo hello         # override CMD
./alpine --help             # show runner help
```

### Volume mounts

```bash
./myapp -v /host/data:/data                      # read-write
./myapp -v /etc/hosts:/etc/hosts:ro cat /etc/hosts  # read-only
./myapp -v ./input:/input -v ./output:/output -- process.sh
```

### CLI args replace CMD

Following Docker semantics, extra args replace `CMD` while `ENTRYPOINT` is preserved:

```bash
# Image: ENTRYPOINT ["python3"], CMD ["app.py"]
./myapp              # runs: python3 app.py
./myapp -c "print(42)"  # runs: python3 -c "print(42)"
```

### Build-time flags

| Flag | Effect |
|------|--------|
| `-o <path>` | Output binary path (default: `./<image_name>`) |
| `--bwrap-path <path>` | Embed this specific bwrap binary |
| `--unsquashfs-path <path>` | Embed this specific unsquashfs binary |

### Environment variables

| Variable | Effect |
|----------|--------|
| `BONK_TOOLS_DIR=<dir>` | Directory containing `bwrap` and `unsquashfs` to embed |
| `BONK_RUNNER=<path>` | Path to the `bonk-runner` binary to embed |

When `--bwrap-path` / `--unsquashfs-path` are omitted, `bonk` searches in order:
1. `BONK_TOOLS_DIR` environment variable
2. `tools/<arch>/` next to the bonk binary
3. `tools/` next to the bonk binary (flat)

---

## Project structure

```
bonk/
├── Cargo.toml                    # workspace
├── crates/
│   ├── bonk-common/              # shared types (footer, config)
│   ├── bonk-cli/                 # `bonk` — the build tool
│   │   └── src/
│   │       ├── main.rs           # CLI entry point
│   │       ├── image.rs          # docker save + manifest parsing
│   │       ├── flatten.rs        # layer flattening + whiteout handling
│   │       └── pack.rs           # squashfs build + binary assembly
│   └── bonk-runner/              # embedded runner stub
│       └── src/
│           ├── main.rs           # payload dispatch + cache management
│           ├── mount.rs          # unsquashfs extraction
│           └── runtime.rs        # bwrap invocation + volume mounts
├── lessons/                      # guided curriculum (see below)
└── tests/
    └── e2e.sh
```

---

## Lessons

This repo is structured as a guided Rust curriculum. Each lesson introduces language concepts through a concrete piece of the tool.

| # | Lesson | Concepts |
|---|--------|----------|
| 01 | [Workspace & Project Structure](lessons/01-workspace-and-project-structure.md) | Cargo workspaces, crate layout |
| 02 | [Structs, Traits & Shared Types](lessons/02-structs-traits-and-shared-types.md) | Structs, `serde`, shared crates |
| 03 | [Error Handling & CLI Skeleton](lessons/03-error-handling-and-cli-skeleton.md) | `anyhow`, `clap`, `Result` |
| 04 | [Spawning Subprocesses](lessons/04-spawning-subprocesses.md) | `std::process::Command`, I/O piping |
| 05 | [File I/O & JSON Parsing](lessons/05-file-io-and-json-parsing.md) | `std::fs`, `serde_json`, tar reading |
| 06 | [Trait Objects & Compression Detection](lessons/06-trait-objects-and-compression-detection.md) | `dyn Read`, dynamic dispatch |
| 07 | [Iterators & Layer Flattening](lessons/07-iterators-and-layer-flattening.md) | Iterators, whiteout handling, `HashMap` |
| 08 | [SquashFS Build & Binary Assembly](lessons/08-compression-pipelines-and-binary-assembly.md) | File I/O, binary layout, footer writing |
| 09 | [Self-Reading Binaries & Caching](lessons/09-self-reading-binaries-and-caching.md) | `/proc/self/exe`, seeks, cache logic |
| 10 | [Container Runtime](lessons/10-container-runtime.md) | `exec`, namespaces, volume mounts |

---

## Comparison with dockerc

[dockerc](https://github.com/NilsIrl/dockerc) is the closest comparable tool — it also converts Docker images into single self-contained executables with no runtime dependencies. The key architectural difference is in how the rootfs is served at runtime.

| | bonk | dockerc |
|---|---|---|
| Rootfs strategy | Extract once → native directory | Mount squashfs via FUSE at runtime |
| Embedded tools | `bwrap` + `unsquashfs` (1.2 MB) | `crun` + `squashfuse` + `fuse-overlayfs` |
| Container runtime | bwrap (user namespaces) | crun (OCI) |
| Disk usage | Uncompressed rootfs in cache | None (mounts directly from squashfs) |
| aarch64 16K-page kernels | ✅ works | ❌ crashes (Zig runtime panic) |
| alpine binary size | ~5.2 MB | ~11 MB |
| Runner stub size | ~720 KB | ~7.2 MB |

**Why bonk is faster at runtime:** dockerc mounts squashfs via FUSE — every `open()` and `stat()` goes user → kernel → FUSE daemon → kernel → back, through two FUSE layers. bonk extracts to a native directory once; subsequent runs hit the filesystem directly with zero overhead.

**Where dockerc wins:** no disk space used for extracted rootfs — it mounts directly from the compressed squashfs.

---

## Limitations

- **Linux only** — bwrap is Linux-specific
- **No cgroup isolation** — no memory/CPU limits
- **No multi-container orchestration** — single binaries, not a compose replacement
- **Cache in `/tmp`** — extracted rootfs is lost on reboot; first run after reboot re-extracts
- **Disk usage** — rootfs cache uses disk space equal to the uncompressed image

---

## License

MIT
