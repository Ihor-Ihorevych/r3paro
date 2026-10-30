# hp3-engine

An open-source engine for *Harry Potter and the Prisoner of Azkaban* (PC, 2004), written in Rust. It runs the
game from your own retail install; no game data ships with it. It supports HP3 only.

Status: early. The package reader loads HP3's version-129 packages; see `docs/status.md`.

## Building

```sh
cargo build --release
cargo run --release -p hp3-pkg -- check <HP3 install folder>
```

Android (arm64): `cargo ndk -t arm64-v8a build --release` with NDK 27.

## Rules

`CLEANROOM.md` lists the sources this project may use. Licensed GPLv3 (`LICENSE`).
