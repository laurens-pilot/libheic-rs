use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};
use rav1d::Dav1dResult;
use std::error::Error;
use std::ffi::c_void;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::BufWriter;
use std::mem::MaybeUninit;
use std::path::Path;
use std::ptr::{self, NonNull};

pub mod isobmff;

/// Errors returned by the decoder entry points.
#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    AvifDecode(DecodeAvifError),
    PngEncoding(png::EncodingError),
    Unsupported(String),
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(err) => write!(f, "I/O error: {err}"),
            DecodeError::AvifDecode(err) => write!(f, "{err}"),
            DecodeError::PngEncoding(err) => write!(f, "PNG encode error: {err}"),
            DecodeError::Unsupported(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for DecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DecodeError::Io(err) => Some(err),
            DecodeError::AvifDecode(err) => Some(err),
            DecodeError::PngEncoding(err) => Some(err),
            DecodeError::Unsupported(_) => None,
        }
    }
}

impl From<std::io::Error> for DecodeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<DecodeAvifError> for DecodeError {
    fn from(value: DecodeAvifError) -> Self {
        Self::AvifDecode(value)
    }
}

impl From<png::EncodingError> for DecodeError {
    fn from(value: png::EncodingError) -> Self {
        Self::PngEncoding(value)
    }
}

/// Decoded AVIF chroma layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvifPixelLayout {
    Yuv400,
    Yuv420,
    Yuv422,
    Yuv444,
}

/// Decoded AVIF plane samples.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AvifPlaneSamples {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

/// One decoded AVIF image plane in row-major order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvifPlane {
    pub width: u32,
    pub height: u32,
    pub samples: AvifPlaneSamples,
}

/// Decoded AVIF image in planar YUV form.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedAvifImage {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub layout: AvifPixelLayout,
    pub y_plane: AvifPlane,
    pub u_plane: Option<AvifPlane>,
    pub v_plane: Option<AvifPlane>,
}

/// Errors from the AVIF decode path and internal image model conversion.
#[derive(Debug)]
pub enum DecodeAvifError {
    ParsePrimaryProperties(isobmff::ParsePrimaryAvifPropertiesError),
    ExtractPrimaryPayload(isobmff::ExtractAvifItemDataError),
    DecoderAllocationFailed {
        length: usize,
    },
    DecoderApi {
        stage: &'static str,
        code: i32,
    },
    DecoderNoFrameOutput,
    InvalidImageGeometry {
        width: i32,
        height: i32,
    },
    UnsupportedBitDepth {
        bit_depth: i32,
    },
    UnsupportedPixelLayout {
        layout: u32,
    },
    MissingPlane {
        plane: &'static str,
        layout: AvifPixelLayout,
    },
    PlaneStrideOverflow {
        plane: &'static str,
        stride: isize,
    },
    PlaneStrideTooSmall {
        plane: &'static str,
        stride: isize,
        required: usize,
    },
    PlaneSizeOverflow {
        plane: &'static str,
        width: u32,
        height: u32,
    },
    DecodedGeometryMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    PlaneSampleTypeMismatch {
        plane: &'static str,
        expected: &'static str,
        actual: &'static str,
    },
    PlaneDimensionsMismatch {
        plane: &'static str,
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    PlaneSampleCountMismatch {
        plane: &'static str,
        expected: usize,
        actual: usize,
    },
}

impl Display for DecodeAvifError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeAvifError::ParsePrimaryProperties(err) => write!(f, "{err}"),
            DecodeAvifError::ExtractPrimaryPayload(err) => write!(f, "{err}"),
            DecodeAvifError::DecoderAllocationFailed { length } => write!(
                f,
                "rav1d failed to allocate input buffer for {length} bytes"
            ),
            DecodeAvifError::DecoderApi { stage, code } => {
                write!(f, "rav1d API call {stage} failed with code {code}")
            }
            DecodeAvifError::DecoderNoFrameOutput => {
                write!(f, "rav1d did not produce a decoded frame")
            }
            DecodeAvifError::InvalidImageGeometry { width, height } => write!(
                f,
                "decoded AV1 frame has invalid geometry ({width}x{height})"
            ),
            DecodeAvifError::UnsupportedBitDepth { bit_depth } => {
                write!(f, "decoded AV1 frame has unsupported bit depth {bit_depth}")
            }
            DecodeAvifError::UnsupportedPixelLayout { layout } => {
                write!(f, "decoded AV1 frame has unsupported pixel layout value {layout}")
            }
            DecodeAvifError::MissingPlane { plane, layout } => write!(
                f,
                "decoded AV1 frame is missing {plane} plane for {layout:?} layout"
            ),
            DecodeAvifError::PlaneStrideOverflow { plane, stride } => write!(
                f,
                "decoded AV1 {plane} plane stride {stride} overflows row addressing"
            ),
            DecodeAvifError::PlaneStrideTooSmall {
                plane,
                stride,
                required,
            } => write!(
                f,
                "decoded AV1 {plane} plane stride {stride} is smaller than required row bytes {required}"
            ),
            DecodeAvifError::PlaneSizeOverflow {
                plane,
                width,
                height,
            } => write!(
                f,
                "decoded AV1 {plane} plane dimensions ({width}x{height}) are too large"
            ),
            DecodeAvifError::DecodedGeometryMismatch {
                expected_width,
                expected_height,
                actual_width,
                actual_height,
            } => write!(
                f,
                "decoded AV1 frame geometry mismatch: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
            ),
            DecodeAvifError::PlaneSampleTypeMismatch {
                plane,
                expected,
                actual,
            } => write!(
                f,
                "decoded AV1 {plane} plane has sample type {actual}, expected {expected}"
            ),
            DecodeAvifError::PlaneDimensionsMismatch {
                plane,
                expected_width,
                expected_height,
                actual_width,
                actual_height,
            } => write!(
                f,
                "decoded AV1 {plane} plane has dimensions {actual_width}x{actual_height}, expected {expected_width}x{expected_height}"
            ),
            DecodeAvifError::PlaneSampleCountMismatch {
                plane,
                expected,
                actual,
            } => write!(
                f,
                "decoded AV1 {plane} plane has {actual} samples, expected {expected}"
            ),
        }
    }
}

impl Error for DecodeAvifError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DecodeAvifError::ParsePrimaryProperties(err) => Some(err),
            DecodeAvifError::ExtractPrimaryPayload(err) => Some(err),
            _ => None,
        }
    }
}

impl From<isobmff::ParsePrimaryAvifPropertiesError> for DecodeAvifError {
    fn from(value: isobmff::ParsePrimaryAvifPropertiesError) -> Self {
        Self::ParsePrimaryProperties(value)
    }
}

impl From<isobmff::ExtractAvifItemDataError> for DecodeAvifError {
    fn from(value: isobmff::ExtractAvifItemDataError) -> Self {
        Self::ExtractPrimaryPayload(value)
    }
}

/// Decode the primary AVIF item into an internal planar YUV image model.
pub fn decode_primary_avif_to_image(input: &[u8]) -> Result<DecodedAvifImage, DecodeAvifError> {
    // Provenance: mirrors libheif configuration+payload bitstream assembly in
    // libheif/libheif/codecs/decoder.cc:Decoder::get_compressed_data and
    // AVIF configuration extraction in
    // libheif/libheif/codecs/avif_dec.cc:Decoder_AVIF::read_bitstream_configuration_data.
    let item_data = isobmff::extract_primary_avif_item_data(input)?;
    let payload = item_data.payload;
    let (elementary_stream, expected_geometry) =
        match isobmff::parse_primary_avif_item_properties(input) {
            Ok(properties) => {
                let mut stream = properties.av1c.config_obus.clone();
                stream.extend_from_slice(&payload);
                (
                    stream,
                    Some((properties.ispe.width, properties.ispe.height)),
                )
            }
            Err(isobmff::ParsePrimaryAvifPropertiesError::MissingRequiredProperty {
                property_type,
                ..
            }) if property_type.as_bytes() == *b"pixi" => {
                // Some valid AVIF assets (including libheif/examples/example.avif)
                // omit pixi; keep decode progress by feeding the coded payload
                // directly to the AV1 decoder and deriving geometry from the frame.
                (payload, None)
            }
            Err(err) => return Err(DecodeAvifError::ParsePrimaryProperties(err)),
        };

    let decoded = decode_av1_bitstream_to_image(&elementary_stream)?;
    if let Some((expected_width, expected_height)) = expected_geometry {
        if decoded.width != expected_width || decoded.height != expected_height {
            return Err(DecodeAvifError::DecodedGeometryMismatch {
                expected_width,
                expected_height,
                actual_width: decoded.width,
                actual_height: decoded.height,
            });
        }
    }

    Ok(decoded)
}

/// Decode a HEIF/HEIC/AVIF image from `input_path` and write a PNG to `output_path`.
pub fn decode_file_to_png(input_path: &Path, output_path: &Path) -> Result<(), DecodeError> {
    if !input_path.exists() {
        return Err(DecodeError::Unsupported(format!(
            "Input file does not exist: {}",
            input_path.display()
        )));
    }

    if matches!(
        input_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("avif")),
        Some(true)
    ) {
        let input = std::fs::read(input_path)?;
        let decoded = decode_primary_avif_to_image(&input)?;
        write_decoded_avif_to_png(&decoded, output_path)?;
        return Ok(());
    }

    let _ = output_path;
    Err(DecodeError::Unsupported(
        "Decoder not implemented yet.".to_string(),
    ))
}

// Provenance: conversion constants/mapping align with libheif's full-range
// YCbCr->RGB defaults in libheif/libheif/color-conversion/yuv2rgb.cc
// (Op_YCbCr420_to_RGB32::convert_colorspace) and libheif/libheif/nclx.cc
// (YCbCr_to_RGB_coefficients::defaults).
const YCBCR_TO_RGB_R_CR_COEFF_FP8: i32 = 359;
const YCBCR_TO_RGB_G_CB_COEFF_FP8: i32 = -88;
const YCBCR_TO_RGB_G_CR_COEFF_FP8: i32 = -183;
const YCBCR_TO_RGB_B_CB_COEFF_FP8: i32 = 454;

fn write_decoded_avif_to_png(
    decoded: &DecodedAvifImage,
    output_path: &Path,
) -> Result<(), DecodeError> {
    if decoded.bit_depth <= 8 {
        let pixels = convert_avif_to_rgba8(decoded)?;
        return write_rgba8_png(decoded.width, decoded.height, &pixels, output_path);
    }

    let pixels = convert_avif_to_rgba16(decoded)?;
    write_rgba16_png(decoded.width, decoded.height, &pixels, output_path)
}

fn write_rgba8_png(
    width: u32,
    height: u32,
    pixels: &[u8],
    output_path: &Path,
) -> Result<(), DecodeError> {
    let file = File::create(output_path)?;
    let writer = BufWriter::new(file);

    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder.write_header()?;
    png_writer.write_image_data(pixels)?;

    Ok(())
}

fn write_rgba16_png(
    width: u32,
    height: u32,
    pixels: &[u16],
    output_path: &Path,
) -> Result<(), DecodeError> {
    let file = File::create(output_path)?;
    let writer = BufWriter::new(file);

    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Sixteen);
    let mut png_writer = encoder.write_header()?;

    let byte_len = pixels
        .len()
        .checked_mul(2)
        .ok_or_else(|| DecodeError::Unsupported("RGBA16 byte buffer size overflow".to_string()))?;
    let mut bytes = Vec::with_capacity(byte_len);
    for sample in pixels {
        bytes.extend_from_slice(&sample.to_be_bytes());
    }
    png_writer.write_image_data(&bytes)?;

    Ok(())
}

fn convert_avif_to_rgba8(decoded: &DecodedAvifImage) -> Result<Vec<u8>, DecodeAvifError> {
    validate_plane_dimensions(&decoded.y_plane, decoded.width, decoded.height, "Y")?;
    let y_samples = plane_samples_u8(&decoded.y_plane, "Y")?;
    let expected_y_samples = sample_count(decoded.width, decoded.height, "Y")?;
    if y_samples.len() != expected_y_samples {
        return Err(DecodeAvifError::PlaneSampleCountMismatch {
            plane: "Y",
            expected: expected_y_samples,
            actual: y_samples.len(),
        });
    }

    let width = usize::try_from(decoded.width).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
        plane: "RGBA",
        width: decoded.width,
        height: decoded.height,
    })?;
    let height =
        usize::try_from(decoded.height).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
            plane: "RGBA",
            width: decoded.width,
            height: decoded.height,
        })?;
    let output_len =
        expected_y_samples
            .checked_mul(4)
            .ok_or(DecodeAvifError::PlaneSizeOverflow {
                plane: "RGBA",
                width: decoded.width,
                height: decoded.height,
            })?;
    let mut out = Vec::with_capacity(output_len);

    let chroma = prepare_chroma_u8(decoded)?;
    let half_range = chroma_half_range(decoded.bit_depth);

    for y in 0..height {
        let row_start = y
            .checked_mul(width)
            .ok_or(DecodeAvifError::PlaneSizeOverflow {
                plane: "RGBA",
                width: decoded.width,
                height: decoded.height,
            })?;

        for x in 0..width {
            let y_index = row_start
                .checked_add(x)
                .ok_or(DecodeAvifError::PlaneSizeOverflow {
                    plane: "RGBA",
                    width: decoded.width,
                    height: decoded.height,
                })?;
            let y_sample = i32::from(y_samples[y_index]);

            let (cb_centered, cr_centered) = match &chroma {
                ChromaPlanesU8::Monochrome => (0, 0),
                ChromaPlanesU8::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]) - half_range,
                        i32::from(v_samples[chroma_index]) - half_range,
                    )
                }
            };

            let (r, g, b) =
                ycbcr_to_rgb_components(y_sample, cb_centered, cr_centered, decoded.bit_depth);
            out.push(scale_sample_to_u8(r, decoded.bit_depth));
            out.push(scale_sample_to_u8(g, decoded.bit_depth));
            out.push(scale_sample_to_u8(b, decoded.bit_depth));
            out.push(u8::MAX);
        }
    }

    Ok(out)
}

fn convert_avif_to_rgba16(decoded: &DecodedAvifImage) -> Result<Vec<u16>, DecodeAvifError> {
    validate_plane_dimensions(&decoded.y_plane, decoded.width, decoded.height, "Y")?;
    let y_samples = plane_samples_u16(&decoded.y_plane, "Y")?;
    let expected_y_samples = sample_count(decoded.width, decoded.height, "Y")?;
    if y_samples.len() != expected_y_samples {
        return Err(DecodeAvifError::PlaneSampleCountMismatch {
            plane: "Y",
            expected: expected_y_samples,
            actual: y_samples.len(),
        });
    }

    let width = usize::try_from(decoded.width).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
        plane: "RGBA",
        width: decoded.width,
        height: decoded.height,
    })?;
    let height =
        usize::try_from(decoded.height).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
            plane: "RGBA",
            width: decoded.width,
            height: decoded.height,
        })?;
    let output_len =
        expected_y_samples
            .checked_mul(4)
            .ok_or(DecodeAvifError::PlaneSizeOverflow {
                plane: "RGBA",
                width: decoded.width,
                height: decoded.height,
            })?;
    let mut out = Vec::with_capacity(output_len);

    let chroma = prepare_chroma_u16(decoded)?;
    let half_range = chroma_half_range(decoded.bit_depth);

    for y in 0..height {
        let row_start = y
            .checked_mul(width)
            .ok_or(DecodeAvifError::PlaneSizeOverflow {
                plane: "RGBA",
                width: decoded.width,
                height: decoded.height,
            })?;

        for x in 0..width {
            let y_index = row_start
                .checked_add(x)
                .ok_or(DecodeAvifError::PlaneSizeOverflow {
                    plane: "RGBA",
                    width: decoded.width,
                    height: decoded.height,
                })?;
            let y_sample = i32::from(y_samples[y_index]);

            let (cb_centered, cr_centered) = match &chroma {
                ChromaPlanesU16::Monochrome => (0, 0),
                ChromaPlanesU16::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]) - half_range,
                        i32::from(v_samples[chroma_index]) - half_range,
                    )
                }
            };

            let (r, g, b) =
                ycbcr_to_rgb_components(y_sample, cb_centered, cr_centered, decoded.bit_depth);
            out.push(scale_sample_to_u16(r, decoded.bit_depth));
            out.push(scale_sample_to_u16(g, decoded.bit_depth));
            out.push(scale_sample_to_u16(b, decoded.bit_depth));
            out.push(u16::MAX);
        }
    }

    Ok(out)
}

enum ChromaPlanesU8<'a> {
    Monochrome,
    Color {
        u_samples: &'a [u8],
        v_samples: &'a [u8],
        chroma_width: usize,
        layout: AvifPixelLayout,
    },
}

enum ChromaPlanesU16<'a> {
    Monochrome,
    Color {
        u_samples: &'a [u16],
        v_samples: &'a [u16],
        chroma_width: usize,
        layout: AvifPixelLayout,
    },
}

fn prepare_chroma_u8(decoded: &DecodedAvifImage) -> Result<ChromaPlanesU8<'_>, DecodeAvifError> {
    if decoded.layout == AvifPixelLayout::Yuv400 {
        return Ok(ChromaPlanesU8::Monochrome);
    }

    let (u_plane, v_plane, expected_width, expected_height) = require_chroma_planes(decoded)?;
    validate_plane_dimensions(u_plane, expected_width, expected_height, "U")?;
    validate_plane_dimensions(v_plane, expected_width, expected_height, "V")?;

    let u_samples = plane_samples_u8(u_plane, "U")?;
    let v_samples = plane_samples_u8(v_plane, "V")?;
    let expected_samples = sample_count(expected_width, expected_height, "U/V")?;
    if u_samples.len() != expected_samples {
        return Err(DecodeAvifError::PlaneSampleCountMismatch {
            plane: "U",
            expected: expected_samples,
            actual: u_samples.len(),
        });
    }
    if v_samples.len() != expected_samples {
        return Err(DecodeAvifError::PlaneSampleCountMismatch {
            plane: "V",
            expected: expected_samples,
            actual: v_samples.len(),
        });
    }

    let chroma_width =
        usize::try_from(expected_width).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
            plane: "U",
            width: expected_width,
            height: expected_height,
        })?;
    Ok(ChromaPlanesU8::Color {
        u_samples,
        v_samples,
        chroma_width,
        layout: decoded.layout,
    })
}

fn prepare_chroma_u16(decoded: &DecodedAvifImage) -> Result<ChromaPlanesU16<'_>, DecodeAvifError> {
    if decoded.layout == AvifPixelLayout::Yuv400 {
        return Ok(ChromaPlanesU16::Monochrome);
    }

    let (u_plane, v_plane, expected_width, expected_height) = require_chroma_planes(decoded)?;
    validate_plane_dimensions(u_plane, expected_width, expected_height, "U")?;
    validate_plane_dimensions(v_plane, expected_width, expected_height, "V")?;

    let u_samples = plane_samples_u16(u_plane, "U")?;
    let v_samples = plane_samples_u16(v_plane, "V")?;
    let expected_samples = sample_count(expected_width, expected_height, "U/V")?;
    if u_samples.len() != expected_samples {
        return Err(DecodeAvifError::PlaneSampleCountMismatch {
            plane: "U",
            expected: expected_samples,
            actual: u_samples.len(),
        });
    }
    if v_samples.len() != expected_samples {
        return Err(DecodeAvifError::PlaneSampleCountMismatch {
            plane: "V",
            expected: expected_samples,
            actual: v_samples.len(),
        });
    }

    let chroma_width =
        usize::try_from(expected_width).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
            plane: "U",
            width: expected_width,
            height: expected_height,
        })?;
    Ok(ChromaPlanesU16::Color {
        u_samples,
        v_samples,
        chroma_width,
        layout: decoded.layout,
    })
}

fn require_chroma_planes(
    decoded: &DecodedAvifImage,
) -> Result<(&AvifPlane, &AvifPlane, u32, u32), DecodeAvifError> {
    let (expected_width, expected_height) =
        chroma_dimensions(decoded.width, decoded.height, decoded.layout);
    let u_plane = decoded
        .u_plane
        .as_ref()
        .ok_or(DecodeAvifError::MissingPlane {
            plane: "U",
            layout: decoded.layout,
        })?;
    let v_plane = decoded
        .v_plane
        .as_ref()
        .ok_or(DecodeAvifError::MissingPlane {
            plane: "V",
            layout: decoded.layout,
        })?;
    Ok((u_plane, v_plane, expected_width, expected_height))
}

fn plane_samples_u8<'a>(
    plane: &'a AvifPlane,
    plane_name: &'static str,
) -> Result<&'a [u8], DecodeAvifError> {
    match &plane.samples {
        AvifPlaneSamples::U8(samples) => Ok(samples),
        AvifPlaneSamples::U16(_) => Err(DecodeAvifError::PlaneSampleTypeMismatch {
            plane: plane_name,
            expected: "u8",
            actual: "u16",
        }),
    }
}

fn plane_samples_u16<'a>(
    plane: &'a AvifPlane,
    plane_name: &'static str,
) -> Result<&'a [u16], DecodeAvifError> {
    match &plane.samples {
        AvifPlaneSamples::U8(_) => Err(DecodeAvifError::PlaneSampleTypeMismatch {
            plane: plane_name,
            expected: "u16",
            actual: "u8",
        }),
        AvifPlaneSamples::U16(samples) => Ok(samples),
    }
}

fn validate_plane_dimensions(
    plane: &AvifPlane,
    expected_width: u32,
    expected_height: u32,
    plane_name: &'static str,
) -> Result<(), DecodeAvifError> {
    if plane.width != expected_width || plane.height != expected_height {
        return Err(DecodeAvifError::PlaneDimensionsMismatch {
            plane: plane_name,
            expected_width,
            expected_height,
            actual_width: plane.width,
            actual_height: plane.height,
        });
    }

    Ok(())
}

fn sample_count(
    width: u32,
    height: u32,
    plane_name: &'static str,
) -> Result<usize, DecodeAvifError> {
    let width_usize = usize::try_from(width).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
        plane: plane_name,
        width,
        height,
    })?;
    let height_usize = usize::try_from(height).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
        plane: plane_name,
        width,
        height,
    })?;
    width_usize
        .checked_mul(height_usize)
        .ok_or(DecodeAvifError::PlaneSizeOverflow {
            plane: plane_name,
            width,
            height,
        })
}

fn chroma_sample_index(x: usize, y: usize, chroma_width: usize, layout: AvifPixelLayout) -> usize {
    match layout {
        AvifPixelLayout::Yuv400 => 0,
        AvifPixelLayout::Yuv420 => (y / 2) * chroma_width + (x / 2),
        AvifPixelLayout::Yuv422 => y * chroma_width + (x / 2),
        AvifPixelLayout::Yuv444 => y * chroma_width + x,
    }
}

fn ycbcr_to_rgb_components(
    y: i32,
    cb_centered: i32,
    cr_centered: i32,
    bit_depth: u8,
) -> (u16, u16, u16) {
    let r = y + ((YCBCR_TO_RGB_R_CR_COEFF_FP8 * cr_centered + 128) >> 8);
    let g = y
        + ((YCBCR_TO_RGB_G_CB_COEFF_FP8 * cb_centered
            + YCBCR_TO_RGB_G_CR_COEFF_FP8 * cr_centered
            + 128)
            >> 8);
    let b = y + ((YCBCR_TO_RGB_B_CB_COEFF_FP8 * cb_centered + 128) >> 8);

    (
        clip_to_bit_depth(r, bit_depth),
        clip_to_bit_depth(g, bit_depth),
        clip_to_bit_depth(b, bit_depth),
    )
}

fn chroma_half_range(bit_depth: u8) -> i32 {
    1_i32 << u32::from(bit_depth.saturating_sub(1))
}

fn clip_to_bit_depth(value: i32, bit_depth: u8) -> u16 {
    let max_value = ((1_i32 << bit_depth) - 1).max(0);
    value.clamp(0, max_value) as u16
}

fn scale_sample_to_u8(sample: u16, bit_depth: u8) -> u8 {
    if bit_depth == 8 {
        return sample as u8;
    }

    let max_value = (1_u32 << bit_depth) - 1;
    let scaled = (u32::from(sample) * u32::from(u8::MAX) + (max_value / 2)) / max_value;
    scaled as u8
}

fn scale_sample_to_u16(sample: u16, bit_depth: u8) -> u16 {
    if bit_depth == 16 {
        return sample;
    }

    let max_value = (1_u32 << bit_depth) - 1;
    let scaled = (u32::from(sample) * u32::from(u16::MAX) + (max_value / 2)) / max_value;
    scaled as u16
}

#[derive(Default)]
struct DecoderContextGuard(Option<Dav1dContext>);

impl Drop for DecoderContextGuard {
    fn drop(&mut self) {
        // SAFETY: `dav1d_close` accepts a pointer to optional context and
        // safely handles `None` by doing nothing.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.0))) };
    }
}

#[derive(Default)]
struct DecoderDataGuard(Dav1dData);

impl Drop for DecoderDataGuard {
    fn drop(&mut self) {
        // SAFETY: `dav1d_data_unref` accepts initialized/default `Dav1dData`
        // and clears associated references if present.
        unsafe { dav1d_data_unref(Some(NonNull::from(&mut self.0))) };
    }
}

#[derive(Default)]
struct DecoderPictureGuard(Dav1dPicture);

impl Drop for DecoderPictureGuard {
    fn drop(&mut self) {
        // SAFETY: `dav1d_picture_unref` accepts initialized/default
        // `Dav1dPicture` and clears associated references if present.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut self.0))) };
    }
}

fn decode_av1_bitstream_to_image(bitstream: &[u8]) -> Result<DecodedAvifImage, DecodeAvifError> {
    let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
    // SAFETY: `dav1d_default_settings` writes a full valid `Dav1dSettings`.
    unsafe { dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr())) };
    // SAFETY: initialized by `dav1d_default_settings`.
    let mut settings = unsafe { settings.assume_init() };
    settings.n_threads = 1;
    settings.max_frame_delay = 1;

    let mut context = DecoderContextGuard::default();
    let open_result = unsafe {
        // SAFETY: pointers point to valid initialized storage.
        dav1d_open(
            Some(NonNull::from(&mut context.0)),
            Some(NonNull::from(&mut settings)),
        )
    };
    ensure_dav1d_ok("dav1d_open", open_result)?;

    let mut data = DecoderDataGuard::default();
    let input_ptr = unsafe {
        // SAFETY: `data.0` points to valid storage for output data wrapper.
        dav1d_data_create(Some(NonNull::from(&mut data.0)), bitstream.len())
    };
    if input_ptr.is_null() {
        return Err(DecodeAvifError::DecoderAllocationFailed {
            length: bitstream.len(),
        });
    }
    // SAFETY: `dav1d_data_create` allocated `bitstream.len()` bytes at
    // `input_ptr` and `bitstream` has exactly that length.
    unsafe {
        ptr::copy_nonoverlapping(bitstream.as_ptr(), input_ptr, bitstream.len());
    }

    let send_result = unsafe {
        // SAFETY: context was opened successfully and data pointer is valid.
        dav1d_send_data(context.0, Some(NonNull::from(&mut data.0)))
    };
    ensure_dav1d_ok("dav1d_send_data", send_result)?;

    let mut picture = DecoderPictureGuard::default();
    for _ in 0..16 {
        let result = unsafe {
            // SAFETY: context remains valid until guard drop and picture points
            // to valid writable storage.
            dav1d_get_picture(context.0, Some(NonNull::from(&mut picture.0)))
        };
        if result.0 == 0 {
            return picture_to_internal_image(&picture.0);
        }
        if result.0 != -libc::EAGAIN {
            return Err(DecodeAvifError::DecoderApi {
                stage: "dav1d_get_picture",
                code: result.0,
            });
        }
    }

    Err(DecodeAvifError::DecoderNoFrameOutput)
}

fn ensure_dav1d_ok(stage: &'static str, result: Dav1dResult) -> Result<(), DecodeAvifError> {
    if result.0 == 0 {
        Ok(())
    } else {
        Err(DecodeAvifError::DecoderApi {
            stage,
            code: result.0,
        })
    }
}

fn picture_to_internal_image(picture: &Dav1dPicture) -> Result<DecodedAvifImage, DecodeAvifError> {
    let width = u32::try_from(picture.p.w).map_err(|_| DecodeAvifError::InvalidImageGeometry {
        width: picture.p.w,
        height: picture.p.h,
    })?;
    let height = u32::try_from(picture.p.h).map_err(|_| DecodeAvifError::InvalidImageGeometry {
        width: picture.p.w,
        height: picture.p.h,
    })?;
    if width == 0 || height == 0 {
        return Err(DecodeAvifError::InvalidImageGeometry {
            width: picture.p.w,
            height: picture.p.h,
        });
    }

    let bit_depth_i32 = picture.p.bpc;
    let bit_depth =
        u8::try_from(bit_depth_i32).map_err(|_| DecodeAvifError::UnsupportedBitDepth {
            bit_depth: bit_depth_i32,
        })?;
    let bytes_per_sample = match bit_depth {
        1..=8 => 1,
        9..=16 => 2,
        _ => {
            return Err(DecodeAvifError::UnsupportedBitDepth {
                bit_depth: bit_depth_i32,
            })
        }
    };

    let layout = decode_layout_from_dav1d(picture.p.layout)?;
    let y_ptr = picture.data[0].ok_or(DecodeAvifError::MissingPlane { plane: "Y", layout })?;
    let y_plane = AvifPlane {
        width,
        height,
        samples: copy_plane_samples(
            y_ptr,
            picture.stride[0],
            width,
            height,
            bytes_per_sample,
            "Y",
        )?,
    };

    let (u_plane, v_plane) = match layout {
        AvifPixelLayout::Yuv400 => (None, None),
        AvifPixelLayout::Yuv420 | AvifPixelLayout::Yuv422 | AvifPixelLayout::Yuv444 => {
            let (chroma_width, chroma_height) = chroma_dimensions(width, height, layout);
            let u_ptr =
                picture.data[1].ok_or(DecodeAvifError::MissingPlane { plane: "U", layout })?;
            let v_ptr =
                picture.data[2].ok_or(DecodeAvifError::MissingPlane { plane: "V", layout })?;

            let u_plane = AvifPlane {
                width: chroma_width,
                height: chroma_height,
                samples: copy_plane_samples(
                    u_ptr,
                    picture.stride[1],
                    chroma_width,
                    chroma_height,
                    bytes_per_sample,
                    "U",
                )?,
            };
            let v_plane = AvifPlane {
                width: chroma_width,
                height: chroma_height,
                samples: copy_plane_samples(
                    v_ptr,
                    picture.stride[1],
                    chroma_width,
                    chroma_height,
                    bytes_per_sample,
                    "V",
                )?,
            };
            (Some(u_plane), Some(v_plane))
        }
    };

    Ok(DecodedAvifImage {
        width,
        height,
        bit_depth,
        layout,
        y_plane,
        u_plane,
        v_plane,
    })
}

fn decode_layout_from_dav1d(layout: u32) -> Result<AvifPixelLayout, DecodeAvifError> {
    if layout == DAV1D_PIXEL_LAYOUT_I400 {
        Ok(AvifPixelLayout::Yuv400)
    } else if layout == DAV1D_PIXEL_LAYOUT_I420 {
        Ok(AvifPixelLayout::Yuv420)
    } else if layout == DAV1D_PIXEL_LAYOUT_I422 {
        Ok(AvifPixelLayout::Yuv422)
    } else if layout == DAV1D_PIXEL_LAYOUT_I444 {
        Ok(AvifPixelLayout::Yuv444)
    } else {
        Err(DecodeAvifError::UnsupportedPixelLayout { layout })
    }
}

fn chroma_dimensions(width: u32, height: u32, layout: AvifPixelLayout) -> (u32, u32) {
    match layout {
        AvifPixelLayout::Yuv400 => (0, 0),
        AvifPixelLayout::Yuv420 => (width.div_ceil(2), height.div_ceil(2)),
        AvifPixelLayout::Yuv422 => (width.div_ceil(2), height),
        AvifPixelLayout::Yuv444 => (width, height),
    }
}

fn copy_plane_samples(
    plane_ptr: NonNull<c_void>,
    stride: isize,
    width: u32,
    height: u32,
    bytes_per_sample: usize,
    plane: &'static str,
) -> Result<AvifPlaneSamples, DecodeAvifError> {
    let width_usize = usize::try_from(width).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
        plane,
        width,
        height,
    })?;
    let height_usize = usize::try_from(height).map_err(|_| DecodeAvifError::PlaneSizeOverflow {
        plane,
        width,
        height,
    })?;
    let row_bytes =
        width_usize
            .checked_mul(bytes_per_sample)
            .ok_or(DecodeAvifError::PlaneSizeOverflow {
                plane,
                width,
                height,
            })?;

    let stride_abs = stride.unsigned_abs();
    if stride_abs < row_bytes {
        return Err(DecodeAvifError::PlaneStrideTooSmall {
            plane,
            stride,
            required: row_bytes,
        });
    }

    let sample_count =
        width_usize
            .checked_mul(height_usize)
            .ok_or(DecodeAvifError::PlaneSizeOverflow {
                plane,
                width,
                height,
            })?;
    let src_base = plane_ptr.cast::<u8>().as_ptr();

    if bytes_per_sample == 1 {
        let mut out = vec![0_u8; sample_count];
        for row in 0..height_usize {
            let row_offset = (row as isize)
                .checked_mul(stride)
                .ok_or(DecodeAvifError::PlaneStrideOverflow { plane, stride })?;
            // SAFETY: rav1d guarantees decoded plane buffers are valid for the
            // frame dimensions and stride. Bounds are validated by row_bytes.
            let src_row = unsafe { src_base.offset(row_offset) };
            // SAFETY: row pointer and length are validated by decoder contract
            // and stride checks above.
            let src_slice = unsafe { std::slice::from_raw_parts(src_row, row_bytes) };
            let dst_offset =
                row.checked_mul(width_usize)
                    .ok_or(DecodeAvifError::PlaneSizeOverflow {
                        plane,
                        width,
                        height,
                    })?;
            let dst_end =
                dst_offset
                    .checked_add(width_usize)
                    .ok_or(DecodeAvifError::PlaneSizeOverflow {
                        plane,
                        width,
                        height,
                    })?;
            out[dst_offset..dst_end].copy_from_slice(src_slice);
        }
        return Ok(AvifPlaneSamples::U8(out));
    }

    let mut out = vec![0_u16; sample_count];
    for row in 0..height_usize {
        let row_offset = (row as isize)
            .checked_mul(stride)
            .ok_or(DecodeAvifError::PlaneStrideOverflow { plane, stride })?;
        // SAFETY: rav1d guarantees decoded plane buffers are valid for the
        // frame dimensions and stride. Bounds are validated by row_bytes.
        let src_row = unsafe { src_base.offset(row_offset) };
        // SAFETY: row pointer and length are validated by decoder contract and
        // stride checks above.
        let src_slice = unsafe { std::slice::from_raw_parts(src_row, row_bytes) };

        let dst_offset =
            row.checked_mul(width_usize)
                .ok_or(DecodeAvifError::PlaneSizeOverflow {
                    plane,
                    width,
                    height,
                })?;
        for (col, bytes) in src_slice.chunks_exact(2).enumerate() {
            out[dst_offset + col] = u16::from_ne_bytes([bytes[0], bytes[1]]);
        }
    }

    Ok(AvifPlaneSamples::U16(out))
}

#[cfg(test)]
mod tests {
    use super::{
        convert_avif_to_rgba8, decode_file_to_png, decode_primary_avif_to_image, AvifPixelLayout,
        AvifPlane, AvifPlaneSamples, DecodedAvifImage,
    };
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn decodes_example_avif_into_internal_plane_model() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libheif/examples/example.avif");
        let input = std::fs::read(&fixture).expect("example.avif fixture must be readable");
        let decoded =
            decode_primary_avif_to_image(&input).expect("example AVIF should decode into planes");

        assert!(decoded.width > 0);
        assert!(decoded.height > 0);
        assert!((8..=12).contains(&decoded.bit_depth));
        assert_plane_len(
            &decoded.y_plane,
            decoded.width as usize * decoded.height as usize,
        );

        match decoded.layout {
            AvifPixelLayout::Yuv400 => {
                assert!(decoded.u_plane.is_none());
                assert!(decoded.v_plane.is_none());
            }
            AvifPixelLayout::Yuv420 | AvifPixelLayout::Yuv422 | AvifPixelLayout::Yuv444 => {
                let u_plane = decoded.u_plane.as_ref().expect("U plane should exist");
                let v_plane = decoded.v_plane.as_ref().expect("V plane should exist");
                let chroma_len = u_plane.width as usize * u_plane.height as usize;
                assert_plane_len(u_plane, chroma_len);
                assert_plane_len(v_plane, chroma_len);
                assert_eq!(u_plane.width, v_plane.width);
                assert_eq!(u_plane.height, v_plane.height);
            }
        }
    }

    #[test]
    fn decodes_example_avif_to_png() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libheif/examples/example.avif");
        let input = std::fs::read(&fixture).expect("example.avif fixture must be readable");
        let decoded =
            decode_primary_avif_to_image(&input).expect("example AVIF should decode into planes");

        let output = test_output_png_path("example-avif");
        let _guard = TempFileGuard(output.clone());
        decode_file_to_png(&fixture, &output).expect("AVIF decode should write PNG");

        let png_data = std::fs::read(&output).expect("decoded PNG should be readable");
        let decoder = png::Decoder::new(Cursor::new(png_data));
        let mut reader = decoder.read_info().expect("PNG info should decode");
        let frame_len = reader
            .output_buffer_size()
            .expect("output buffer size should be known after read_info");
        let mut frame = vec![0; frame_len];
        let frame_info = reader
            .next_frame(&mut frame)
            .expect("PNG frame should decode");

        assert_eq!(frame_info.width, decoded.width);
        assert_eq!(frame_info.height, decoded.height);
        assert_eq!(frame_info.color_type, png::ColorType::Rgba);
        assert!(matches!(
            frame_info.bit_depth,
            png::BitDepth::Eight | png::BitDepth::Sixteen
        ));
    }

    #[test]
    fn converts_monochrome_u8_planes_to_rgba8() {
        let image = DecodedAvifImage {
            width: 2,
            height: 1,
            bit_depth: 8,
            layout: AvifPixelLayout::Yuv400,
            y_plane: AvifPlane {
                width: 2,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![32, 200]),
            },
            u_plane: None,
            v_plane: None,
        };

        let rgba = convert_avif_to_rgba8(&image).expect("YUV400 should convert to RGBA8");
        assert_eq!(rgba, vec![32, 32, 32, 255, 200, 200, 200, 255]);
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
        let nanos = since_epoch.as_nanos();
        std::env::temp_dir().join(format!(
            "libheic-rs-{label}-{}-{nanos}.png",
            std::process::id()
        ))
    }

    fn assert_plane_len(plane: &AvifPlane, expected_samples: usize) {
        match &plane.samples {
            AvifPlaneSamples::U8(samples) => assert_eq!(samples.len(), expected_samples),
            AvifPlaneSamples::U16(samples) => assert_eq!(samples.len(), expected_samples),
        }
    }
}
