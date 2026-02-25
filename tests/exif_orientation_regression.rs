use libheic_rs::{decode_path_to_rgba, exif_orientation_hint};
use std::path::PathBuf;

#[test]
fn applies_exif_orientation_rotate_90_cw_from_exif_item() {
    let fixture = fixture_path("tests/fixtures/1718_rotate_90_cw.HEIC");
    let decoded = decode_path_to_rgba(&fixture).unwrap_or_else(|err| {
        panic!(
            "decode_path_to_rgba should decode {} without implicit EXIF orientation: {err}",
            fixture.display()
        )
    });

    assert_eq!(
        (decoded.width, decoded.height),
        (2439, 3504),
        "default decode should remain libheif-parity for orientation=6 source geometry"
    );

    let input = std::fs::read(&fixture).expect("fixture should be readable");
    let hint = exif_orientation_hint(&input);
    assert_eq!(hint.orientation_to_apply(), Some(6));

    let oriented = decoded
        .apply_exif_orientation(
            hint.orientation_to_apply()
                .expect("orientation should be present"),
        )
        .expect("apply_exif_orientation should rotate decoded image");
    assert_eq!(
        (oriented.width, oriented.height),
        (3504, 2439),
        "orientation=6 should rotate display geometry to 3504x2439"
    );
}

#[test]
fn applies_exif_orientation_mirror_horizontal_rotate_270_cw_from_exif_item() {
    let fixture = fixture_path("tests/fixtures/7949_mirror_horizontal_rotate_270_cw.HEIC");
    let decoded = decode_path_to_rgba(&fixture).unwrap_or_else(|err| {
        panic!(
            "decode_path_to_rgba should decode {} without implicit EXIF orientation: {err}",
            fixture.display()
        )
    });

    assert_eq!(
        (decoded.width, decoded.height),
        (1209, 1547),
        "default decode should remain libheif-parity for orientation=5 source geometry"
    );

    let input = std::fs::read(&fixture).expect("fixture should be readable");
    let hint = exif_orientation_hint(&input);
    assert_eq!(hint.orientation_to_apply(), Some(5));

    let oriented = decoded
        .apply_exif_orientation(
            hint.orientation_to_apply()
                .expect("orientation should be present"),
        )
        .expect("apply_exif_orientation should transform decoded image");
    assert_eq!(
        (oriented.width, oriented.height),
        (1547, 1209),
        "orientation=5 should transform display geometry to 1547x1209"
    );
}

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}
