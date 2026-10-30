# Status

Milestones are from the plan (M0 to M12).

- **M0 Foundations**: workspace, crate skeletons, clean-room rules, Windows MSVC and Android arm64 toolchains.
- **M1 Packages: done.** `hp3-pkg check <HP3 folder>` reads all 156,497 exports of the 109 packages of a retail
  install exactly (0 errors, 0 leftover bytes, under a second), including all 10,452 scripts. Formats:
  `docs/formats/packages.md`.
- **M2 Assets**: decoders for every native asset class exist (`hp3-assets`); texture conversion and the viewer
  are next.
- M3 onwards: not started.

## Checking

```sh
cargo run --release -p hp3-pkg -- check <HP3 folder>
HP3_DIR=<HP3 folder> cargo test --release -p hp3-pkg
```
