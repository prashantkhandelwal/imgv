# imgv

A fast native image viewer built with Rust and [iced](https://iced.rs/).

## Features

- Pick any local folder with the native folder chooser.
- Browse image thumbnails and filenames in a scrollable grid.
- Preview an image at full available size with one click.
- Decode thumbnails in background tasks to keep the interface responsive.

Supported formats include AVIF, BMP, GIF, ICO, JPEG, PNG, TIFF, and WebP.

## Run

Install a current stable Rust toolchain, then run:

```sh
cargo run --release
```