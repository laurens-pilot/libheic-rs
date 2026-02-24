use libheic_rs::decode_primary_uncompressed_to_image;
use libheic_rs::isobmff::parse_primary_uncompressed_item_properties;
use std::path::PathBuf;

#[test]
fn parses_and_decodes_tiled_rgb_pixel_row_tile_alignment_fixture() {
    let input =
        read_fixture("../libheif/tests/data/uncompressed_pix_RGB_tiled_row_tile_align.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("RGB pixel row/tile-align fixture properties should parse");

    assert_eq!(
        properties.unc_c.interleave_type, 1,
        "expected pixel interleave"
    );
    assert_eq!(properties.unc_c.pixel_size, 0);
    assert_eq!(properties.unc_c.row_align_size, 30);
    assert_eq!(properties.unc_c.tile_align_size, 37);
    assert_eq!(properties.unc_c.num_tile_cols, 2);
    assert_eq!(properties.unc_c.num_tile_rows, 4);
    assert_eq!(properties.unc_c.components.len(), 3);
    assert!(
        properties
            .unc_c
            .components
            .iter()
            .all(|component| component.component_align_size == 0),
        "fixture should keep per-component alignment disabled"
    );

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("RGB pixel row/tile-align fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
}

#[test]
fn parses_and_decodes_tiled_rgb_row_row_tile_alignment_fixture() {
    let input =
        read_fixture("../libheif/tests/data/uncompressed_row_RGB_tiled_row_tile_align.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("RGB row row/tile-align fixture properties should parse");

    assert_eq!(
        properties.unc_c.interleave_type, 3,
        "expected row interleave"
    );
    assert_eq!(properties.unc_c.pixel_size, 0);
    assert_eq!(properties.unc_c.row_align_size, 30);
    assert_eq!(properties.unc_c.tile_align_size, 37);
    assert_eq!(properties.unc_c.num_tile_cols, 2);
    assert_eq!(properties.unc_c.num_tile_rows, 4);
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("RGB row row/tile-align fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
}

#[test]
fn parses_and_decodes_tiled_rgb_component_alignment_fixture() {
    let input = read_fixture("../libheif/tests/data/uncompressed_comp_R7+1G7+1B7+1_tiled.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("RGB component-align fixture properties should parse");

    assert_eq!(
        properties.unc_c.interleave_type, 0,
        "expected component interleave"
    );
    assert_eq!(properties.unc_c.pixel_size, 0);
    assert_eq!(properties.unc_c.row_align_size, 0);
    assert_eq!(properties.unc_c.tile_align_size, 0);
    assert_eq!(properties.unc_c.num_tile_cols, 2);
    assert_eq!(properties.unc_c.num_tile_rows, 4);
    assert_eq!(properties.unc_c.components.len(), 3);
    assert!(
        properties
            .unc_c
            .components
            .iter()
            .all(|component| component.component_align_size == 1),
        "fixture should require 1-byte component alignment padding"
    );
    assert!(
        properties
            .unc_c
            .components
            .iter()
            .all(|component| component.component_bit_depth == 7),
        "fixture should carry 7-bit components padded to byte alignment"
    );

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("RGB component-align fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 7);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
}

#[test]
fn parses_and_decodes_tiled_monochrome_pixel_alignment_fixture() {
    let input = read_fixture("../libheif/tests/data/uncompressed_pix_M_tiled.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("monochrome pixel fixture properties should parse");

    assert_eq!(properties.unc_c.components.len(), 1);
    assert_eq!(
        properties.unc_c.interleave_type, 1,
        "expected pixel interleave"
    );
    assert_eq!(properties.unc_c.pixel_size, 0);
    assert_eq!(properties.unc_c.row_align_size, 0);
    assert_eq!(properties.unc_c.tile_align_size, 0);
    assert_eq!(properties.unc_c.num_tile_cols, 2);
    assert_eq!(properties.unc_c.num_tile_rows, 4);
    assert_eq!(properties.unc_c.components[0].component_align_size, 0);
    assert_eq!(properties.unc_c.components[0].component_bit_depth, 8);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("monochrome pixel fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
    for pixel in decoded.rgba.chunks_exact(4) {
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
    }
}

#[test]
fn parses_and_decodes_tiled_rgb_non_zero_pixel_size_fixture() {
    let input = read_fixture("../libheif/tests/data/uncompressed_pix_R8G8B8_bsz0_psz10_tiled.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("RGB pixel-size fixture properties should parse");

    assert_eq!(
        properties.unc_c.interleave_type, 1,
        "expected pixel interleave"
    );
    assert_eq!(properties.unc_c.pixel_size, 10);
    assert_eq!(properties.unc_c.row_align_size, 0);
    assert_eq!(properties.unc_c.tile_align_size, 0);
    assert_eq!(properties.unc_c.num_tile_cols, 2);
    assert_eq!(properties.unc_c.num_tile_rows, 4);
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("RGB non-zero pixel-size fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
}

#[test]
fn decodes_tile_component_rgb_fixture() {
    let input = read_fixture("../libheif/fuzzing/data/corpus/uncompressed_tile_RGB_tiled.heic");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("tile-component RGB fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 4,
        "expected tile-component"
    );
    assert_eq!(properties.unc_c.sampling_type, 0, "expected no subsampling");

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("tile-component RGB fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
}

#[test]
fn decodes_tile_component_b16r16g16_fixture() {
    let input =
        read_fixture("../libheif/fuzzing/data/corpus/uncompressed_tile_B16R16G16_tiled.heic");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("tile-component B16R16G16 fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 4,
        "expected tile-component"
    );
    assert_eq!(properties.unc_c.sampling_type, 0, "expected no subsampling");

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("tile-component B16R16G16 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 16);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
}

#[test]
fn decodes_tile_component_r7_plus_1_fixture() {
    let input =
        read_fixture("../libheif/fuzzing/data/corpus/uncompressed_tile_R7+1G7+1B7+1_tiled.heic");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("tile-component R7+1 fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 4,
        "expected tile-component"
    );
    assert_eq!(properties.unc_c.sampling_type, 0, "expected no subsampling");
    assert!(
        properties
            .unc_c
            .components
            .iter()
            .all(|component| component.component_align_size == 1),
        "fixture should keep one-byte component alignment"
    );

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("tile-component R7+1 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 7);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
}

#[test]
fn decodes_component_interleave_yuv_tiled_fixture() {
    let input = read_fixture("../libheif/fuzzing/data/corpus/uncompressed_comp_YUV_tiled.heic");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("component-interleave YUV fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 0,
        "expected component interleave"
    );
    assert_eq!(properties.unc_c.sampling_type, 0, "expected no subsampling");
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("component-interleave YUV fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
    assert!(
        decoded
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2]),
        "YUV fixture should produce chroma-varying RGB output"
    );
}

#[test]
fn decodes_pixel_interleave_yuv_tiled_fixture() {
    let input = read_fixture("../libheif/fuzzing/data/corpus/uncompressed_pix_YUV_tiled.heic");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("pixel-interleave YUV fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 1,
        "expected pixel interleave"
    );
    assert_eq!(properties.unc_c.sampling_type, 0, "expected no subsampling");
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("pixel-interleave YUV fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
    assert!(
        decoded
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2]),
        "YUV fixture should produce chroma-varying RGB output"
    );
}

#[test]
fn decodes_tile_component_yuv_tiled_fixture() {
    let input = read_fixture("../libheif/fuzzing/data/corpus/uncompressed_tile_YUV_tiled.heic");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("tile-component YUV fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 4,
        "expected tile-component interleave"
    );
    assert_eq!(properties.unc_c.sampling_type, 0, "expected no subsampling");
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("tile-component YUV fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(decoded.bit_depth, 8);
    assert_eq!(
        decoded.rgba.len(),
        decoded.width as usize * decoded.height as usize * 4
    );
    assert!(
        decoded
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2]),
        "YUV fixture should produce chroma-varying RGB output"
    );
}

#[test]
fn decodes_component_interleave_yuv_422_fixture() {
    let input = read_fixture("../libheif/tests/data/uncompressed_comp_YUV_422.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("component-interleave YUV 4:2:2 fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 0,
        "expected component interleave"
    );
    assert_eq!(properties.unc_c.sampling_type, 1, "expected 4:2:2 sampling");
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("component-interleave YUV 4:2:2 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (32, 20));
    assert_eq!(decoded.bit_depth, 8);

    let p00 = rgba_pixel_at(&decoded.rgba, decoded.width, 0, 0);
    let p10 = rgba_pixel_at(&decoded.rgba, decoded.width, 1, 0);
    let p01 = rgba_pixel_at(&decoded.rgba, decoded.width, 0, 1);
    assert_eq!(p00, [253, 1, 0, 255]);
    assert_eq!(p00, p10);
    assert_eq!(p00, p01);

    let p40 = rgba_pixel_at(&decoded.rgba, decoded.width, 4, 0);
    let p50 = rgba_pixel_at(&decoded.rgba, decoded.width, 5, 0);
    assert_eq!(p40, [0, 129, 0, 255]);
    assert_eq!(p40, p50);
}

#[test]
fn decodes_component_interleave_y16u16v16_420_fixture() {
    let input = read_fixture("../libheif/tests/data/uncompressed_comp_Y16U16V16_420.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("component-interleave Y16U16V16 4:2:0 fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 0,
        "expected component interleave"
    );
    assert_eq!(properties.unc_c.sampling_type, 2, "expected 4:2:0 sampling");
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("component-interleave Y16U16V16 4:2:0 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (32, 20));
    assert_eq!(decoded.bit_depth, 16);

    let p00 = rgba_pixel_at(&decoded.rgba, decoded.width, 0, 0);
    assert_eq!(p00, rgba_pixel_at(&decoded.rgba, decoded.width, 1, 0));
    assert_eq!(p00, rgba_pixel_at(&decoded.rgba, decoded.width, 0, 1));
    assert_eq!(p00, rgba_pixel_at(&decoded.rgba, decoded.width, 1, 1));

    let p40 = rgba_pixel_at(&decoded.rgba, decoded.width, 4, 0);
    assert_eq!(p40, rgba_pixel_at(&decoded.rgba, decoded.width, 5, 0));
    assert_eq!(p40, rgba_pixel_at(&decoded.rgba, decoded.width, 4, 1));
    assert_eq!(p40, rgba_pixel_at(&decoded.rgba, decoded.width, 5, 1));
    assert_ne!(p00, p40);
}

#[test]
fn decodes_mixed_interleave_yuv_422_fixture() {
    let input = read_fixture("../libheif/tests/data/uncompressed_mix_YUV_422.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("mixed-interleave YUV 4:2:2 fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 2,
        "expected mixed interleave"
    );
    assert_eq!(properties.unc_c.sampling_type, 1, "expected 4:2:2 sampling");
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("mixed-interleave YUV 4:2:2 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (32, 20));
    assert_eq!(decoded.bit_depth, 8);

    let p00 = rgba_pixel_at(&decoded.rgba, decoded.width, 0, 0);
    let p10 = rgba_pixel_at(&decoded.rgba, decoded.width, 1, 0);
    let p01 = rgba_pixel_at(&decoded.rgba, decoded.width, 0, 1);
    assert_eq!(p00, [253, 1, 0, 255]);
    assert_eq!(p00, p10);
    assert_eq!(p00, p01);

    let p40 = rgba_pixel_at(&decoded.rgba, decoded.width, 4, 0);
    let p50 = rgba_pixel_at(&decoded.rgba, decoded.width, 5, 0);
    assert_eq!(p40, [0, 129, 0, 255]);
    assert_eq!(p40, p50);
}

#[test]
fn decodes_mixed_interleave_y16u16v16_420_fixture() {
    let input = read_fixture("../libheif/tests/data/uncompressed_mix_Y16U16V16_420.heif");
    let properties = parse_primary_uncompressed_item_properties(&input)
        .expect("mixed-interleave Y16U16V16 4:2:0 fixture properties should parse");
    assert_eq!(
        properties.unc_c.interleave_type, 2,
        "expected mixed interleave"
    );
    assert_eq!(properties.unc_c.sampling_type, 2, "expected 4:2:0 sampling");
    assert_eq!(properties.unc_c.components.len(), 3);

    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("mixed-interleave Y16U16V16 4:2:0 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (32, 20));
    assert_eq!(decoded.bit_depth, 16);

    let p00 = rgba_pixel_at(&decoded.rgba, decoded.width, 0, 0);
    assert_eq!(p00, rgba_pixel_at(&decoded.rgba, decoded.width, 1, 0));
    assert_eq!(p00, rgba_pixel_at(&decoded.rgba, decoded.width, 0, 1));
    assert_eq!(p00, rgba_pixel_at(&decoded.rgba, decoded.width, 1, 1));

    let p40 = rgba_pixel_at(&decoded.rgba, decoded.width, 4, 0);
    assert_eq!(p40, rgba_pixel_at(&decoded.rgba, decoded.width, 5, 0));
    assert_eq!(p40, rgba_pixel_at(&decoded.rgba, decoded.width, 4, 1));
    assert_eq!(p40, rgba_pixel_at(&decoded.rgba, decoded.width, 5, 1));
    assert_ne!(p00, p40);
}

#[test]
fn decodes_tiled_r5g6b5_component_fixture_without_mixed_depth_double_scaling() {
    let input = read_fixture("../libheif/fuzzing/data/corpus/uncompressed_comp_R5G6B5_tiled.heic");
    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("tiled component-interleave R5G6B5 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(
        decoded.bit_depth, 8,
        "mixed 5/6/5 channels should normalize directly to 8-bit output"
    );

    let pixel = rgba_pixel_at(&decoded.rgba, decoded.width, 28, 0);
    assert_eq!(pixel, [123, 125, 123, 255]);
    let high_pixel = rgba_pixel_at(&decoded.rgba, decoded.width, 28, 8);
    assert_eq!(high_pixel, [231, 130, 231, 255]);
}

#[test]
fn decodes_tiled_r5g6b5_pixel_fixture_without_mixed_depth_double_scaling() {
    let input = read_fixture("../libheif/tests/data/uncompressed_pix_R5G6B5_tiled.heif");
    let decoded = decode_primary_uncompressed_to_image(&input)
        .expect("tiled pixel-interleave R5G6B5 fixture should decode");
    assert_eq!((decoded.width, decoded.height), (30, 20));
    assert_eq!(
        decoded.bit_depth, 8,
        "mixed 5/6/5 channels should normalize directly to 8-bit output"
    );

    let pixel = rgba_pixel_at(&decoded.rgba, decoded.width, 28, 0);
    assert_eq!(pixel, [123, 125, 123, 255]);
    let high_pixel = rgba_pixel_at(&decoded.rgba, decoded.width, 28, 8);
    assert_eq!(high_pixel, [231, 130, 231, 255]);
}

fn read_fixture(relative: &str) -> Vec<u8> {
    std::fs::read(fixture_path(relative)).expect("fixture must be readable")
}

fn rgba_pixel_at(rgba: &[u16], width: u32, x: usize, y: usize) -> [u16; 4] {
    let width = width as usize;
    let offset = (y * width + x) * 4;
    [
        rgba[offset],
        rgba[offset + 1],
        rgba[offset + 2],
        rgba[offset + 3],
    ]
}

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}
