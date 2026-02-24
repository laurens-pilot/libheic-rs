use libheic_rs::isobmff::{extract_primary_heic_item_data_with_grid, HeicPrimaryItemDataWithGrid};
use libheic_rs::{decode_file_to_png, decode_primary_heic_to_image};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn decodes_real_heic_grid_fixture_with_expected_descriptor_and_tile_coverage() {
    let fixture = fixture_path("tests/fixtures/heic_grid_primary_32.heic");
    let input = std::fs::read(&fixture).expect("HEIC grid fixture must be readable");

    let grid = match extract_primary_heic_item_data_with_grid(&input)
        .expect("HEIC grid fixture should parse")
    {
        HeicPrimaryItemDataWithGrid::Grid(grid) => grid,
        HeicPrimaryItemDataWithGrid::Coded(_) => {
            panic!("fixture primary item must resolve to HEIC grid data")
        }
    };

    assert_eq!((grid.descriptor.columns, grid.descriptor.rows), (2, 2));
    assert_eq!(
        (grid.descriptor.output_width, grid.descriptor.output_height),
        (32, 32)
    );
    assert_eq!(grid.tiles.len(), 4);

    let decoded = decode_primary_heic_to_image(&input)
        .expect("HEIC grid fixture should decode through stitched tile path");
    assert_eq!((decoded.width, decoded.height), (32, 32));
    assert_eq!((decoded.y_plane.width, decoded.y_plane.height), (32, 32));

    let quadrant_sums = [
        sum_luma_quadrant(&decoded.y_plane.samples, 32, 0, 0, 16, 16),
        sum_luma_quadrant(&decoded.y_plane.samples, 32, 16, 0, 16, 16),
        sum_luma_quadrant(&decoded.y_plane.samples, 32, 0, 16, 16, 16),
        sum_luma_quadrant(&decoded.y_plane.samples, 32, 16, 16, 16, 16),
    ];
    let unique_quadrants = quadrant_sums.into_iter().collect::<HashSet<_>>();
    assert_eq!(unique_quadrants.len(), 4);
}

#[test]
fn matches_libheif_oracle_png_for_real_heic_grid_fixture() {
    let fixture = fixture_path("tests/fixtures/heic_grid_primary_32.heic");
    let oracle_bin = fixture_path("../.ralph/tools/libheif-oracle-build/examples/heif-dec");
    if !oracle_bin.is_file() {
        eprintln!(
            "skipping HEIC grid oracle parity test because {} is missing",
            oracle_bin.display()
        );
        return;
    }

    let rust_output = test_output_png_path("heic-grid-rust");
    let _rust_guard = TempFileGuard(rust_output.clone());
    let oracle_output = test_output_png_path("heic-grid-libheif");
    let _oracle_guard = TempFileGuard(oracle_output.clone());

    decode_file_to_png(&fixture, &rust_output)
        .expect("Rust HEIC grid decode should produce a PNG for oracle comparison");

    let oracle = Command::new(&oracle_bin)
        .arg(&fixture)
        .arg(&oracle_output)
        .output()
        .expect("heif-dec should run for HEIC grid oracle parity");
    assert!(
        oracle.status.success(),
        "heif-dec failed with status {:?}",
        oracle.status.code()
    );
    assert!(
        oracle_output.is_file(),
        "heif-dec did not produce output PNG at {}",
        oracle_output.display()
    );

    let rust_png = image::open(&rust_output)
        .expect("Rust grid PNG should be readable")
        .to_rgb8();
    let oracle_png = image::open(&oracle_output)
        .expect("libheif grid PNG should be readable")
        .to_rgb8();
    assert_eq!(rust_png.dimensions(), oracle_png.dimensions());
    assert_eq!(
        rust_png.as_raw(),
        oracle_png.as_raw(),
        "Rust grid decode should match libheif oracle pixel-for-pixel"
    );
}

fn sum_luma_quadrant(
    samples: &[u16],
    image_width: usize,
    origin_x: usize,
    origin_y: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sum = 0_u64;
    for y in origin_y..(origin_y + height) {
        let row_offset = y
            .checked_mul(image_width)
            .expect("quadrant row offset should fit usize");
        for x in origin_x..(origin_x + width) {
            sum = sum.saturating_add(samples[row_offset + x] as u64);
        }
    }
    sum
}

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn test_output_png_path(label: &str) -> PathBuf {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH");
    std::env::temp_dir().join(format!(
        "libheic-rs-{label}-{}-{}.png",
        std::process::id(),
        since_epoch.as_nanos()
    ))
}
