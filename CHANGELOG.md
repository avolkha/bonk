# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.2](https://github.com/avolkha/bonk/compare/v0.2.0...v0.2.2) - 2026-05-12

### Other

- single workspace version; bonk-cli depends on bonk-runner

## [0.2.0] - 2026-05-10

### Added
- Tiered rootfs strategy: kernel squashfs loop-mount with unsquashfs extraction fallback
- `--mount` flag on `bonk-runner` for privileged first-run setup
- `.bonk-ready` marker stores strategy used (`mount` or `extract`); warm runs re-mount if gone after reboot
- Overlay filesystem probe via `bwrap --help`; root-aware namespace flags
- `init_cache_dir_as_root` closes TOCTOU window in world-writable `/tmp`
- `/proc/mounts` parsing to detect if squashfs is already mounted

### Fixed
- `--clearenv` ordering bug in bwrap invocation

## [0.1.0] - 2026-04-19

### Added
- `bonk` CLI that exports a Docker image, flattens its layers into a SquashFS rootfs, and assembles a self-contained executable
- `bonk-runner` stub binary embedded in the output — extracts the rootfs on first run and execs the container via `bwrap`
- Magic-byte based compression detection for layer handling (`gzip`, `zstd`, `xz`, `lz4`, `bzip2`)
- Layer flattening with whiteout file support (`.wh.` and `.wh..wh..opq`)
- JSON manifest parsing from the exported Docker image tar
- Caching: extracted rootfs is reused on subsequent runs (keyed by content hash)
- GitHub Actions CI: `rustfmt`, `clippy`, and workspace unit tests on every push/PR to `main`
- GitHub Actions CI: end-to-end integration test on self-hosted runner
- GitHub Actions Release: automated static binary builds for `x86_64` and `aarch64` (musl) on every `v*.*.*` tag

[0.2.0]: https://github.com/avolkha/bonk/releases/tag/v0.2.0
[0.1.0]: https://github.com/avolkha/bonk/releases/tag/v0.1.0
