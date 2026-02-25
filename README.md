# libheic-rs

Pure Rust HEIF/HEIC/AVIF decoder with first-class `image` crate integration.

## What This Crate Provides

- Decode `.heif`, `.heic`, and `.avif` into RGBA buffers (`u8` or `u16`).
- Decode from `bytes`, `Read`, `BufRead`, and file `Path`.
- Optional guardrails for bounded production use:
  - max input bytes
  - max decoded pixel count
  - max temporary spool bytes for non-seek inputs
  - custom temp spool directory
- Optional `image` integration feature that registers decoder hooks so `image::ImageReader` can open HEIF/HEIC/AVIF directly.

## Install

`Cargo.toml` (local path dependency):

```toml
[dependencies]
libheic-rs = { path = "../libheic-rs" }
```

With `image` integration:

```toml
[dependencies]
libheic-rs = { path = "../libheic-rs", features = ["image-integration"] }
image = { version = "0.25", default-features = false, features = ["png"] }
```

## Decode Example

```rust
use libheic_rs::{decode_path_to_rgba_with_guardrails, DecodeGuardrails};
use std::path::Path;

fn decode_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let guardrails = DecodeGuardrails {
        max_input_bytes: Some(128 * 1024 * 1024),
        max_pixels: Some(64_000_000),
        max_temp_spool_bytes: Some(128 * 1024 * 1024),
        temp_spool_directory: None,
    };

    let decoded = decode_path_to_rgba_with_guardrails(path, guardrails)?;
    println!(
        "decoded {}x{} (storage={}bit)",
        decoded.width,
        decoded.height,
        decoded.storage_bit_depth()
    );

    Ok(())
}
```

## Hook Into The `image` Crate

```rust
use image::ImageReader;
use libheic_rs::image_integration::register_image_decoder_hooks_with_guardrails;
use libheic_rs::DecodeGuardrails;

fn init_image_hooks() {
    let guardrails = DecodeGuardrails {
        max_input_bytes: Some(128 * 1024 * 1024),
        max_pixels: Some(64_000_000),
        max_temp_spool_bytes: Some(128 * 1024 * 1024),
        temp_spool_directory: None,
    };

    let registered = register_image_decoder_hooks_with_guardrails(guardrails);
    assert!(registered.any_decoder_hook_registered());
}

fn decode_with_image(path: &str) -> image::ImageResult<image::DynamicImage> {
    ImageReader::open(path)?.with_guessed_format()?.decode()
}
```

## CLI

Build:

```bash
cargo build --manifest-path libheic-rs/Cargo.toml --release --bin heif-decode
```

Usage:

```bash
libheic-rs/target/release/heif-decode \
  --max-input-bytes 134217728 \
  --max-pixels 64000000 \
  --max-temp-spool-bytes 134217728 \
  <input.heif|.heic|.avif> <output.png>
```

## License

`libheic-rs` is licensed under `LGPL-3.0-or-later`. See `LICENSE`.
