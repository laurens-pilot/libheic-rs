# libheic-rs

Pure Rust HEIF/HEIC/AVIF decoder (work in progress).

## CLI

Build:

```bash
cargo build --release --bin heif-decode
```

Usage:

```bash
target/release/heif-decode <input.heif|.heic|.avif> <output.png>
```

## Status

- Project scaffolding and validation harness are in place.
- Decoder implementation is not complete yet.
