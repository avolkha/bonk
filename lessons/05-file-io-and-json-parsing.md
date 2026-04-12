# Lesson 05 — File I/O & JSON Parsing (`image::parse_image`)

## What you will build

The second half of `crates/bonk-cli/src/image.rs` — the `parse_image` function that:

1. Reads `manifest.json` from the extracted image directory
2. Reads the image's config JSON file
3. Converts the Docker-format config into your `ContainerConfig`
4. Returns `(ContainerConfig, Vec<PathBuf>)` — the config and the list of layer tar paths

---

## Concepts

### Reading files

```rust
use std::fs;

// Simplest: read entire file as String
let content = fs::read_to_string("path/to/file.txt")?;

// Or read as bytes
let bytes = fs::read("path/to/file.bin")?;
```

### `serde_json` — JSON parsing

The `serde` / `serde_json` ecosystem lets you deserialize JSON directly into Rust structs.

**Step 1:** Define a struct that mirrors the JSON structure, and derive `Deserialize`:

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct Person {
    name: String,
    age: u32,
}
```

**Step 2:** Parse:

```rust
let json = r#"{"name": "Alice", "age": 30}"#;
let person: Person = serde_json::from_str(json)?;
```

**Handling missing or renamed fields:**

```rust
#[derive(Deserialize)]
struct DockerManifest {
    #[serde(rename = "Config")]     // JSON key is "Config", Rust field is "config"
    config: String,
    #[serde(rename = "Layers")]
    layers: Vec<String>,
}
```

Use `Option<T>` for fields that may be absent in the JSON — `serde` will set them to `None` if missing.

### Docker image structure

When you run `docker save`, you get a tar containing:

```
manifest.json              ← top-level list of image manifests
<sha256>.json              ← image config (ENTRYPOINT, CMD, ENV, etc.)
<sha256>/
  layer.tar                ← one tar per layer, bottom to top
```

`manifest.json` is a JSON array. The first element has two fields you need:
- `Config` — the filename of the image config JSON
- `Layers` — array of relative paths to each layer tar

The image config JSON has a nested structure. The fields you need are inside `config.Config`:

```json
{
  "config": {
    "Entrypoint": ["python3"],
    "Cmd": ["app.py"],
    "Env": ["PATH=/usr/bin", "LANG=en_US.UTF-8"],
    "WorkingDir": "/app",
    "User": ""
  }
}
```

Note that Docker capitalises these field names.

### `Option` chaining

When converting from Docker's config (which may have empty strings or null) to your `ContainerConfig`, you'll want to treat an empty string the same as absent:

```rust
let user = docker_user.filter(|s| !s.is_empty());
```

`.filter(|s| predicate)` on an `Option` returns `None` if the predicate is false or if the option was already `None`.

```rust
.unwrap_or_else(|| Vec::new())   // use an empty vec if None
.unwrap_or_default()             // use Default::default() if None
```

### Private structs for deserialization

These structs are implementation details — don't make them `pub`. Define them at the top of `image.rs` and only expose the converted `ContainerConfig` to the outside world.

---

## Tasks

### Task 1 — Private Docker manifest struct

In `src/image.rs`, define a private struct `DockerManifest` that can be deserialized from JSON:

```
config: String          ← renamed from "Config"
layers: Vec<String>     ← renamed from "Layers"
```

> **Hint:** JSON field names in docker manifests are PascalCase. Use `#[serde(rename = "Config")]`.

### Task 2 — Private Docker config structs

Define private structs matching this hierarchy:

```
DockerImageConfig
  └─ config: Option<DockerContainerConfig>

DockerContainerConfig
  └─ entrypoint: Option<Vec<String>>  ← "Entrypoint"
  └─ cmd:        Option<Vec<String>>  ← "Cmd"
  └─ env:        Option<Vec<String>>  ← "Env"
  └─ working_dir: Option<String>      ← "WorkingDir"
  └─ user:        Option<String>      ← "User"
```

All fields in `DockerContainerConfig` can be absent in real images, so use `Option`.

### Task 3 — Function signature

Declare:

```
pub fn parse_image(image_dir: &Path) -> Result<(ContainerConfig, Vec<PathBuf>)>
```

### Task 4 — Parse `manifest.json`

Inside `parse_image`:

1. Build the path to `manifest.json` inside `image_dir`
2. Read it with `fs::read_to_string`
3. Deserialize as `Vec<DockerManifest>` — it's a JSON array
4. Take the first element, or bail with a message if the vec is empty

> **Hint:** `serde_json::from_str::<Vec<DockerManifest>>(&content)?` or let type inference figure out the type from how you use it.

### Task 5 — Parse the image config

1. Build the path to the config file using `image_dir.join(&manifest.config)`
2. Read the file as a string
3. Deserialize as `DockerImageConfig`
4. Extract the inner `DockerContainerConfig` from the `config` field (or use `Default::default()` if it's `None`)

### Task 6 — Build `ContainerConfig`

Convert from `DockerContainerConfig` to your `ContainerConfig`:

- For `entrypoint` and `cmd`: use `.unwrap_or_default()` to fall back to an empty vec
- For `env`: same
- For `working_dir`: use `.filter(|s| !s.is_empty()).unwrap_or_else(|| "/".to_string())`
- For `user`: use `.filter(|s| !s.is_empty())`

> **Hint:** If `env` from Docker is empty, fall back to your `ContainerConfig::default().env` so the container at least has a sane PATH.

### Task 7 — Resolve layer paths

For each entry in `manifest.layers`, build a full `PathBuf` with `image_dir.join(layer_path)`.

Validate that each path actually exists on disk using `std::path::Path::exists()`. If a layer file is missing, bail with a clear error message.

Collect the results into a `Vec<PathBuf>` and return it.

### Task 8 — Return

Return `Ok((config, layer_paths))`.

### Task 9 — Wire it up

In `main.rs`, call `parse_image` with the image directory from Lesson 04. Store the `ContainerConfig` and `Vec<PathBuf>`. Print the number of layers found.

### Task 10 — Verify

Run `cargo run --bin bonk -- alpine:latest`. Steps 1 and 2 should complete without errors and report the number of layers in the image.

---

## Check your understanding

1. Why are the Docker manifest structs private (not `pub`)? What would change if you made them public?
2. What does `serde` do when a JSON field is present but your Rust field is `Option<T>`? What if the JSON field is absent?
3. Why do we validate that layer files exist immediately after parsing, rather than waiting until we try to open them?

---

## Next lesson

In Lesson 06 you'll write `flatten::open_layer` — detecting whether a layer file is gzip, zstd, or plain tar by reading its magic bytes, and returning a `Box<dyn Read>` trait object.
