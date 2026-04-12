# Lesson 04 — Spawning Subprocesses (`image::export_image`)

## What you will build

The first half of `crates/bonk-cli/src/image.rs` — the `export_image` function that:

1. Runs `docker save <image> -o image.tar`
2. Extracts the tar into a working directory
3. Returns the path to the extracted image directory

---

## Concepts

### `std::process::Command`

Rust's standard library provides `Command` to launch child processes:

```rust
use std::process::Command;

let status = Command::new("ls")
    .arg("-la")
    .arg("/tmp")
    .status()?;     // blocks until the process finishes, returns its exit status
```

Key methods:
- `.new(program)` — name of the executable
- `.arg(s)` — add one argument
- `.args(iter)` — add multiple arguments from an iterator
- `.status()` — run and wait, returns `ExitStatus`
- `.output()` — run and wait, returns stdout/stderr as bytes
- `.spawn()` — start without waiting, returns a `Child` handle

### Checking the exit status

```rust
let status = Command::new("docker").arg("ps").status()?;
if !status.success() {
    bail!("docker ps failed with exit code {:?}", status.code());
}
```

`status.success()` returns `true` if exit code is 0.

### Environment variables — `std::env::var`

```rust
use std::env;

match env::var("DOCKER") {
    Ok(val) => println!("DOCKER is set to {val}"),
    Err(_)  => println!("DOCKER is not set, using default"),
}
```

`env::var` returns `Result<String, VarError>`. Use `.unwrap_or_else(|_| "default".to_string())` for a fallback.

### Splitting a command string

The `DOCKER` env var might be `"sudo docker"` — one string representing program + prefix args. You need to split on whitespace:

```rust
let docker_cmd = env::var("DOCKER").unwrap_or_else(|_| "docker".to_string());
let mut parts = docker_cmd.split_whitespace();
let program = partsage: &str, workdir: &Path) -> Result<PathBuf> {
    let output = workdir.next().unwrap_or("docker");
let prefix_args: Vec<&str> = parts.collect();
```

### `PathBuf` and path manipulation

`PathBuf` is an owned, heap-allocated path — like `String` but for filesystem paths. Build one with:

```rust
use std::path::PathBuf;

let mut p = PathBuf::from("/home/user");
p.push("documents");          // appends a component
p.push("file.txt");
// p is now /home/user/documents/file.txt

let parent = p.parent();      // returns Option<&Path>
let name   = p.file_name();   // returns Option<&OsStr>
let joined = p.join("other"); // like push but returns a new PathBuf
```

### `tar::Archive` — extracting archives

```rust
use std::fs::File;
use tar::Archive;

let file = File::open("archive.tar")?;
let mut archive = Archive::new(file);
archive.unpack("/destination/directory")?;
```

`unpack` extracts all entries relative to the destination path.

---

## Tasks

### Task 1 — Function signature

In `src/image.rs`, declare a public function:

```
pub fn export_image(image: &str, work_dir: &Path) -> Result<PathBuf>
```

It takes an image name (e.g. `"alpine:latest"`) and a working directory path, and returns the path to the extracted image directory.

### Task 2 — Determine the docker command

Inside `export_image`, read the `DOCKER` environment variable. If it's not set, default to `"docker"`. Split the string into a program name (the first word) and any prefix arguments (remaining words).

> **Hint:** Collect the prefix args as `Vec<String>` or `Vec<&str>`. Be careful about lifetimes — `parts` borrows from `docker_cmd`, so `docker_cmd` must not be dropped while `parts` is alive.

### Task 3 — Run `docker save`

Construct and run this command:

```
<program> [prefix_args...] save -o <work_dir>/image.tar <image>
```

Check that the exit status is success. If it fails, use `bail!` to return a helpful error message telling the user what went wrong and suggesting they check if Docker is running.

### Task 4 — Extract the tar

Create a subdirectory `image_dir = work_dir.join("image")`. Create it with `std::fs::create_dir_all(&image_dir)`.

Open `image.tar` as a `File`, wrap it in `tar::Archive`, and call `.unpack(&image_dir)`.

> **Hint:** Add `use tar::Archive;` to your imports.

### Task 5 — Clean up

Delete `work_dir/image.tar` after extraction to free disk space. Use `std::fs::remove_file(...)`.

> This step is optional for correctness but good practice — Docker images can be several GB.

### Task 6 — Return the path

Return `Ok(image_dir)`.

### Task 7 — Wire it up

In `main.rs`, import the function with `use crate::image::export_image;`. In the `[1/5]` step, call `export_image(&cli.image, work_dir.path())?` and store the result.

> **Hint:** `tempdir.path()` gives you a `&Path` to the temp directory.

### Task 8 — Verify

Run (requires Docker to be running and accessible):

```bash
cargo run --bin bonk -- alpine:latest
```

The first step should output something like `[1/5] Exporting image...` and not crash. The rest will still be stubs.

---

## Check your understanding

1. What is the difference between `.status()`, `.output()`, and `.spawn()` on a `Command`?
2. Why does `Command::new("sudo docker save ...")` not work?
3. What happens to the `TempDir` and all its contents when it goes out of scope?

---

## Next lesson

In Lesson 05 you'll parse the extracted image — reading `manifest.json` and the image config JSON to extract ENTRYPOINT, CMD, and layer paths.
