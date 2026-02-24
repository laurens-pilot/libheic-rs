use image::codecs::png::PngDecoder;
use image::{ColorType, ImageBuffer, ImageDecoder, ImageFormat, ImageReader, Rgba};
use libheic_rs::{
    decode_file_to_png, decode_path_to_rgba, decode_primary_avif_to_image,
    decode_primary_heic_to_image, DecodedRgbaPixels,
};
use std::fs::File;
use std::io::{BufReader, ErrorKind};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn image_reader_decodes_avif_output_png() {
    let fixture = fixture_path("../libheif/examples/example.avif");
    let input = std::fs::read(&fixture).expect("example.avif fixture must be readable");
    let expected =
        decode_primary_avif_to_image(&input).expect("example AVIF should decode into planes");

    let output = temp_png_path("image-reader-avif");
    let _guard = TempFileGuard(output.clone());
    decode_file_to_png(&fixture, &output).expect("AVIF decode should write PNG");

    let reader = ImageReader::open(&output)
        .expect("decoded PNG should be openable by image crate")
        .with_guessed_format()
        .expect("image crate should infer output PNG format");
    assert_eq!(reader.format(), Some(ImageFormat::Png));
    let image = reader
        .decode()
        .expect("image reader should decode AVIF-derived PNG");

    assert_eq!(
        (image.width(), image.height()),
        (expected.width, expected.height)
    );
}

#[test]
fn image_decoder_trait_reads_heic_output_png() {
    let fixture = fixture_path("../libheif/examples/example.heic");
    let input = std::fs::read(&fixture).expect("example.heic fixture must be readable");
    let expected =
        decode_primary_heic_to_image(&input).expect("example HEIC should decode into planes");

    let output = temp_png_path("image-decoder-heic");
    let _guard = TempFileGuard(output.clone());
    decode_file_to_png(&fixture, &output).expect("HEIC decode should write PNG");

    let file = File::open(&output).expect("decoded PNG should be readable");
    let decoder = PngDecoder::new(BufReader::new(file))
        .expect("image PNG decoder should parse HEIC-derived output");
    let (width, height) = decoder.dimensions();
    assert_eq!((width, height), (expected.width, expected.height));

    let color_type = decoder.color_type();
    assert!(matches!(color_type, ColorType::Rgba8 | ColorType::Rgba16));
    let bytes_per_pixel = match color_type {
        ColorType::Rgba8 => 4_usize,
        ColorType::Rgba16 => 8_usize,
        _ => unreachable!("already constrained to RGBA8/RGBA16"),
    };

    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .expect("decoded dimensions should not overflow usize");
    let expected_bytes = pixel_count
        .checked_mul(bytes_per_pixel)
        .expect("decoded byte count should not overflow usize");
    assert_eq!(decoder.total_bytes() as usize, expected_bytes);

    let mut buf = vec![0_u8; expected_bytes];
    decoder
        .read_image(&mut buf)
        .expect("ImageDecoder::read_image should decode HEIC-derived PNG");
    assert!(buf.iter().any(|sample| *sample != 0));
}

#[test]
fn decode_path_to_rgba_hands_off_owned_pixels_to_image_buffer() {
    let fixture = fixture_path("../libheif/examples/example.heic");
    let decoded = decode_path_to_rgba(&fixture)
        .expect("decode_path_to_rgba should decode HEIC fixture into RGBA samples");
    assert!(decoded.source_bit_depth > 0);
    let width = decoded.width;
    let height = decoded.height;

    match decoded.pixels {
        DecodedRgbaPixels::U8(pixels) => {
            let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, pixels)
                .expect("RGBA8 Vec should transfer directly into image::ImageBuffer");
            assert_eq!(image.dimensions(), (width, height));
        }
        DecodedRgbaPixels::U16(pixels) => {
            let image = ImageBuffer::<Rgba<u16>, Vec<u16>>::from_raw(width, height, pixels)
                .expect("RGBA16 Vec should transfer directly into image::ImageBuffer");
            assert_eq!(image.dimensions(), (width, height));
        }
    }
}

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn temp_png_path(stem: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "libheic-rs-{stem}-{}-{nanos}.png",
        std::process::id()
    ))
}

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.0) {
            if err.kind() != ErrorKind::NotFound {
                panic!("failed to remove temp file {}: {err}", self.0.display());
            }
        }
    }
}
