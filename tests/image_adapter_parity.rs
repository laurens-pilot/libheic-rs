#![cfg(feature = "image-integration")]

use image::{DynamicImage, ImageReader};
use libheic_rs::image_integration::register_image_decoder_hooks;
use libheic_rs::{decode_path_to_rgba, DecodedRgbaPixels};
use std::path::PathBuf;

#[test]
fn image_reader_adapter_matches_direct_decode_on_representative_fixtures() {
    let _ = register_image_decoder_hooks();

    assert_image_reader_parity("../libheif/examples/example.heic");
    assert_image_reader_parity("../libheif/tests/data/rgb_generic_compressed_defl.heif");
    assert_image_reader_parity("../libheif/examples/example.avif");
}

fn assert_image_reader_parity(relative_fixture_path: &str) {
    let fixture = fixture_path(relative_fixture_path);
    let direct = decode_path_to_rgba(&fixture).unwrap_or_else(|err| {
        panic!(
            "direct decode should succeed for {}: {err}",
            fixture.display()
        )
    });

    let adapter_image = ImageReader::open(&fixture)
        .unwrap_or_else(|err| panic!("fixture should open with image reader: {err}"))
        .with_guessed_format()
        .unwrap_or_else(|err| panic!("image reader should infer fixture format: {err}"))
        .decode()
        .unwrap_or_else(|err| {
            panic!(
                "image-reader adapter decode should succeed for {}: {err}",
                fixture.display()
            )
        });

    assert_eq!(
        (adapter_image.width(), adapter_image.height()),
        (direct.width, direct.height),
        "decoded dimensions should match direct decode for {}",
        fixture.display()
    );

    match (direct.pixels, adapter_image) {
        (DecodedRgbaPixels::U8(expected), DynamicImage::ImageRgba8(actual)) => {
            assert_eq!(
                actual.into_raw(),
                expected,
                "adapter RGBA8 samples should match direct decode for {}",
                fixture.display()
            );
        }
        (DecodedRgbaPixels::U16(expected), DynamicImage::ImageRgba16(actual)) => {
            assert_eq!(
                actual.into_raw(),
                expected,
                "adapter RGBA16 samples should match direct decode for {}",
                fixture.display()
            );
        }
        (DecodedRgbaPixels::U8(_), unexpected) => {
            panic!(
                "adapter output storage mismatch for {}: expected RGBA8, got {:?}",
                fixture.display(),
                unexpected.color()
            );
        }
        (DecodedRgbaPixels::U16(_), unexpected) => {
            panic!(
                "adapter output storage mismatch for {}: expected RGBA16, got {:?}",
                fixture.display(),
                unexpected.color()
            );
        }
    }
}

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}
