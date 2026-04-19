# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.0]: https://github.com/avolkha/bonk/releases/tag/v0.1.0
