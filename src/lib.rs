use heic_decoder::DecodedFrame as HeicDecoderFrame;
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
use scuffle_h265::{NALUnitType, SpsNALUnit};
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
    HeicDecode(DecodeHeicError),
    PngEncoding(png::EncodingError),
    Unsupported(String),
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(err) => write!(f, "I/O error: {err}"),
            DecodeError::AvifDecode(err) => write!(f, "{err}"),
            DecodeError::HeicDecode(err) => write!(f, "{err}"),
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
            DecodeError::HeicDecode(err) => Some(err),
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

impl From<DecodeHeicError> for DecodeError {
    fn from(value: DecodeHeicError) -> Self {
        Self::HeicDecode(value)
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

/// Decoded HEIC chroma layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeicPixelLayout {
    Yuv400,
    Yuv420,
    Yuv422,
    Yuv444,
}

/// One decoded HEIC image plane in row-major order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeicPlane {
    pub width: u32,
    pub height: u32,
    pub samples: Vec<u16>,
}

/// Decoded HEIC image in planar YUV form.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedHeicImage {
    pub width: u32,
    pub height: u32,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub layout: HeicPixelLayout,
    pub y_plane: HeicPlane,
    pub u_plane: Option<HeicPlane>,
    pub v_plane: Option<HeicPlane>,
}

/// Parsed HEIC image metadata extracted from the primary HEVC SPS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedHeicImageMetadata {
    pub width: u32,
    pub height: u32,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub layout: HeicPixelLayout,
}

/// Classification of a parsed HEVC NAL unit for backend frame handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HevcNalClass {
    Vcl,
    ParameterSet,
    AccessUnitDelimiter,
    SupplementalEnhancementInfo,
    Other,
    Unknown,
}

/// One NAL unit parsed from an assembled 4-byte length-prefixed HEVC stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LengthPrefixedHevcNalUnit<'a> {
    offset: usize,
    bytes: &'a [u8],
}

impl<'a> LengthPrefixedHevcNalUnit<'a> {
    fn nal_unit_type_value(self) -> Option<u8> {
        if self.bytes.len() < 2 {
            return None;
        }

        Some((self.bytes[0] >> 1) & 0x3f)
    }

    fn nal_unit_type(self) -> Option<NALUnitType> {
        self.nal_unit_type_value().map(NALUnitType::from)
    }

    fn class(self) -> HevcNalClass {
        match self.nal_unit_type_value() {
            Some(0..=31) => HevcNalClass::Vcl,
            Some(32..=34) => HevcNalClass::ParameterSet,
            Some(35) => HevcNalClass::AccessUnitDelimiter,
            Some(39 | 40) => HevcNalClass::SupplementalEnhancementInfo,
            Some(_) => HevcNalClass::Other,
            None => HevcNalClass::Unknown,
        }
    }
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

/// Errors from HEIC primary-item bitstream assembly for decoder handoff.
#[derive(Debug)]
pub enum DecodeHeicError {
    ParsePrimaryProperties(isobmff::ParsePrimaryHeicPropertiesError),
    ExtractPrimaryPayload(isobmff::ExtractHeicItemDataError),
    BackendDecodeFailed {
        detail: String,
    },
    InvalidDecodedFrame {
        detail: String,
    },
    InvalidNalLengthSize {
        nal_length_size: u8,
    },
    TruncatedNalLengthField {
        offset: usize,
        nal_length_size: u8,
        available: usize,
    },
    TruncatedNalUnit {
        offset: usize,
        declared: usize,
        available: usize,
    },
    NalUnitTooLarge {
        nal_size: usize,
    },
    TruncatedLengthPrefixedStreamLength {
        offset: usize,
        available: usize,
    },
    TruncatedLengthPrefixedStreamNalUnit {
        offset: usize,
        declared: usize,
        available: usize,
    },
    MissingSpsNalUnit,
    SpsParseFailed {
        offset: usize,
        detail: String,
    },
    InvalidSpsGeometry {
        width: u64,
        height: u64,
    },
    UnsupportedSpsChromaArrayType {
        chroma_array_type: u8,
    },
    MissingVclNalUnit,
    DecodedGeometryMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    DecodedBitDepthMismatch {
        expected_luma: u8,
        expected_chroma: u8,
        actual_luma: u8,
        actual_chroma: u8,
    },
    DecodedLayoutMismatch {
        expected: HeicPixelLayout,
        actual: HeicPixelLayout,
    },
}

impl Display for DecodeHeicError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeHeicError::ParsePrimaryProperties(err) => write!(f, "{err}"),
            DecodeHeicError::ExtractPrimaryPayload(err) => write!(f, "{err}"),
            DecodeHeicError::BackendDecodeFailed { detail } => {
                write!(f, "pure-Rust HEVC backend failed to decode frame: {detail}")
            }
            DecodeHeicError::InvalidDecodedFrame { detail } => {
                write!(f, "decoded HEVC frame is invalid: {detail}")
            }
            DecodeHeicError::InvalidNalLengthSize { nal_length_size } => write!(
                f,
                "HEVC nal_length_size must be in 1..=4, got {nal_length_size}"
            ),
            DecodeHeicError::TruncatedNalLengthField {
                offset,
                nal_length_size,
                available,
            } => write!(
                f,
                "truncated HEVC NAL length field at payload offset {offset}: need {nal_length_size} bytes, have {available}"
            ),
            DecodeHeicError::TruncatedNalUnit {
                offset,
                declared,
                available,
            } => write!(
                f,
                "truncated HEVC NAL unit at payload offset {offset}: declared {declared} bytes, have {available}"
            ),
            DecodeHeicError::NalUnitTooLarge { nal_size } => {
                write!(f, "HEVC NAL unit size {nal_size} exceeds 32-bit length limit")
            }
            DecodeHeicError::TruncatedLengthPrefixedStreamLength { offset, available } => write!(
                f,
                "truncated length-prefixed HEVC stream at offset {offset}: need 4-byte NAL length field, have {available}"
            ),
            DecodeHeicError::TruncatedLengthPrefixedStreamNalUnit {
                offset,
                declared,
                available,
            } => write!(
                f,
                "truncated length-prefixed HEVC NAL unit at offset {offset}: declared {declared} bytes, have {available}"
            ),
            DecodeHeicError::MissingSpsNalUnit => write!(
                f,
                "length-prefixed HEVC stream does not contain an SPS NAL unit"
            ),
            DecodeHeicError::SpsParseFailed { offset, detail } => {
                write!(f, "failed to parse SPS NAL unit at stream offset {offset}: {detail}")
            }
            DecodeHeicError::InvalidSpsGeometry { width, height } => write!(
                f,
                "decoded HEVC SPS reports invalid geometry ({width}x{height})"
            ),
            DecodeHeicError::UnsupportedSpsChromaArrayType { chroma_array_type } => write!(
                f,
                "decoded HEVC SPS reports unsupported chroma_array_type {chroma_array_type}"
            ),
            DecodeHeicError::MissingVclNalUnit => write!(
                f,
                "length-prefixed HEVC stream does not contain a VCL NAL unit"
            ),
            DecodeHeicError::DecodedGeometryMismatch {
                expected_width,
                expected_height,
                actual_width,
                actual_height,
            } => write!(
                f,
                "decoded HEVC SPS geometry mismatch: expected {expected_width}x{expected_height}, got {actual_width}x{actual_height}"
            ),
            DecodeHeicError::DecodedBitDepthMismatch {
                expected_luma,
                expected_chroma,
                actual_luma,
                actual_chroma,
            } => write!(
                f,
                "decoded HEVC bit depth mismatch: expected luma/chroma {expected_luma}/{expected_chroma}, got {actual_luma}/{actual_chroma}"
            ),
            DecodeHeicError::DecodedLayoutMismatch { expected, actual } => write!(
                f,
                "decoded HEVC chroma layout mismatch: expected {expected:?}, got {actual:?}"
            ),
        }
    }
}

impl Error for DecodeHeicError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DecodeHeicError::ParsePrimaryProperties(err) => Some(err),
            DecodeHeicError::ExtractPrimaryPayload(err) => Some(err),
            _ => None,
        }
    }
}

impl From<isobmff::ParsePrimaryHeicPropertiesError> for DecodeHeicError {
    fn from(value: isobmff::ParsePrimaryHeicPropertiesError) -> Self {
        Self::ParsePrimaryProperties(value)
    }
}

impl From<isobmff::ExtractHeicItemDataError> for DecodeHeicError {
    fn from(value: isobmff::ExtractHeicItemDataError) -> Self {
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

/// Assemble primary HEIC coded data as a decoder-ready HEVC stream.
pub fn assemble_primary_heic_hevc_stream(input: &[u8]) -> Result<Vec<u8>, DecodeHeicError> {
    // Provenance: mirrors libheif's decoder input assembly flow from
    // libheif/libheif/codecs/decoder.cc:Decoder::get_compressed_data and
    // libheif/libheif/codecs/hevc_dec.cc:Decoder_HEVC::read_bitstream_configuration_data,
    // with hvcC header NAL packing semantics from
    // libheif/libheif/codecs/hevc_boxes.cc:Box_hvcC::get_header_nals.
    let properties = isobmff::parse_primary_heic_item_preflight_properties(input)?;
    let item_data = isobmff::extract_primary_heic_item_data(input)?;
    assemble_heic_hevc_stream_from_components(&properties.hvcc, &item_data.payload)
}

/// Decode the primary HEIC item into an internal planar YUV image model.
pub fn decode_primary_heic_to_image(input: &[u8]) -> Result<DecodedHeicImage, DecodeHeicError> {
    let (stream, metadata) = decode_primary_heic_stream_and_metadata(input)?;
    let decoded = decode_hevc_stream_to_image(&stream)?;
    validate_decoded_heic_image_against_metadata(&decoded, &metadata)?;
    Ok(decoded)
}

/// Parse primary HEIC stream metadata from the first SPS NAL in the assembled HEVC stream.
pub fn decode_primary_heic_to_metadata(
    input: &[u8],
) -> Result<DecodedHeicImageMetadata, DecodeHeicError> {
    let (_, metadata) = decode_primary_heic_stream_and_metadata(input)?;
    Ok(metadata)
}

fn decode_primary_heic_stream_and_metadata(
    input: &[u8],
) -> Result<(Vec<u8>, DecodedHeicImageMetadata), DecodeHeicError> {
    let properties = isobmff::parse_primary_heic_item_preflight_properties(input)?;
    let item_data = isobmff::extract_primary_heic_item_data(input)?;
    let stream = assemble_heic_hevc_stream_from_components(&properties.hvcc, &item_data.payload)?;
    let decoded = decode_hevc_stream_metadata_from_sps(&stream)?;
    validate_decoded_heic_geometry_against_ispe(
        &decoded,
        properties.ispe.width,
        properties.ispe.height,
    )?;
    Ok((stream, decoded))
}

fn validate_decoded_heic_geometry_against_ispe(
    metadata: &DecodedHeicImageMetadata,
    expected_width: u32,
    expected_height: u32,
) -> Result<(), DecodeHeicError> {
    if metadata.width != expected_width || metadata.height != expected_height {
        return Err(DecodeHeicError::DecodedGeometryMismatch {
            expected_width,
            expected_height,
            actual_width: metadata.width,
            actual_height: metadata.height,
        });
    }

    Ok(())
}

fn validate_decoded_heic_image_against_metadata(
    decoded: &DecodedHeicImage,
    metadata: &DecodedHeicImageMetadata,
) -> Result<(), DecodeHeicError> {
    // Provenance: mirrors libheif's decoder metadata expectations where HEVC
    // coded-image chroma/bit-depth metadata is exposed by
    // Decoder_HEVC::{get_coded_image_colorspace,get_luma_bits_per_pixel,get_chroma_bits_per_pixel}
    // and backend output planes are materialized in
    // plugins/decoder_libde265.cc:convert_libde265_image_to_heif_image.
    if decoded.width != metadata.width || decoded.height != metadata.height {
        return Err(DecodeHeicError::DecodedGeometryMismatch {
            expected_width: metadata.width,
            expected_height: metadata.height,
            actual_width: decoded.width,
            actual_height: decoded.height,
        });
    }

    if decoded.layout != metadata.layout {
        return Err(DecodeHeicError::DecodedLayoutMismatch {
            expected: metadata.layout,
            actual: decoded.layout,
        });
    }

    if decoded.bit_depth_luma != metadata.bit_depth_luma
        || decoded.bit_depth_chroma != metadata.bit_depth_chroma
    {
        return Err(DecodeHeicError::DecodedBitDepthMismatch {
            expected_luma: metadata.bit_depth_luma,
            expected_chroma: metadata.bit_depth_chroma,
            actual_luma: decoded.bit_depth_luma,
            actual_chroma: decoded.bit_depth_chroma,
        });
    }

    Ok(())
}

fn decode_hevc_stream_to_image(stream: &[u8]) -> Result<DecodedHeicImage, DecodeHeicError> {
    let parsed_nals = parse_length_prefixed_hevc_nal_units(stream)?;
    if !parsed_nals
        .iter()
        .any(|nal| nal.class() == HevcNalClass::Vcl)
    {
        return Err(DecodeHeicError::MissingVclNalUnit);
    }

    let mut backend_stream = Vec::with_capacity(stream.len());
    for nal_unit in parsed_nals {
        append_nal_with_u32_length_prefix(nal_unit.bytes, &mut backend_stream)?;
    }

    let decoded = heic_decoder::hevc::decode(&backend_stream).map_err(|err| {
        DecodeHeicError::BackendDecodeFailed {
            detail: err.to_string(),
        }
    })?;
    heic_decoder_frame_to_internal_image(&decoded)
}

fn heic_decoder_frame_to_internal_image(
    frame: &HeicDecoderFrame,
) -> Result<DecodedHeicImage, DecodeHeicError> {
    let width = frame.cropped_width();
    let height = frame.cropped_height();
    if width == 0 || height == 0 {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!("cropped geometry must be non-zero, got {width}x{height}"),
        });
    }

    let layout = heic_layout_from_sps_chroma_array_type(frame.chroma_format)?;
    let y_plane = extract_cropped_heic_plane(
        &frame.y_plane,
        frame.y_stride(),
        frame.crop_left,
        frame.crop_top,
        width,
        height,
        "Y",
    )?;

    let (u_plane, v_plane) = match layout {
        HeicPixelLayout::Yuv400 => (None, None),
        HeicPixelLayout::Yuv420 | HeicPixelLayout::Yuv422 | HeicPixelLayout::Yuv444 => {
            let (subsample_x, subsample_y) = heic_chroma_subsampling(layout);
            if !frame.crop_left.is_multiple_of(subsample_x)
                || !frame.crop_right.is_multiple_of(subsample_x)
                || !frame.crop_top.is_multiple_of(subsample_y)
                || !frame.crop_bottom.is_multiple_of(subsample_y)
            {
                return Err(DecodeHeicError::InvalidDecodedFrame {
                    detail: format!(
                        "chroma crop alignment mismatch for layout {layout:?}: crop=({}, {}, {}, {})",
                        frame.crop_left, frame.crop_right, frame.crop_top, frame.crop_bottom
                    ),
                });
            }

            let chroma_width = width.div_ceil(subsample_x);
            let chroma_height = height.div_ceil(subsample_y);
            let chroma_crop_left = frame.crop_left / subsample_x;
            let chroma_crop_top = frame.crop_top / subsample_y;

            let cb_plane = extract_cropped_heic_plane(
                &frame.cb_plane,
                frame.c_stride(),
                chroma_crop_left,
                chroma_crop_top,
                chroma_width,
                chroma_height,
                "U",
            )?;
            let cr_plane = extract_cropped_heic_plane(
                &frame.cr_plane,
                frame.c_stride(),
                chroma_crop_left,
                chroma_crop_top,
                chroma_width,
                chroma_height,
                "V",
            )?;
            (Some(cb_plane), Some(cr_plane))
        }
    };

    Ok(DecodedHeicImage {
        width,
        height,
        bit_depth_luma: frame.bit_depth,
        bit_depth_chroma: frame.bit_depth,
        layout,
        y_plane,
        u_plane,
        v_plane,
    })
}

fn heic_chroma_subsampling(layout: HeicPixelLayout) -> (u32, u32) {
    match layout {
        HeicPixelLayout::Yuv400 | HeicPixelLayout::Yuv444 => (1, 1),
        HeicPixelLayout::Yuv420 => (2, 2),
        HeicPixelLayout::Yuv422 => (2, 1),
    }
}

fn extract_cropped_heic_plane(
    source: &[u16],
    stride: usize,
    crop_left: u32,
    crop_top: u32,
    width: u32,
    height: u32,
    plane: &'static str,
) -> Result<HeicPlane, DecodeHeicError> {
    let width_usize = usize::try_from(width).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
        detail: format!("{plane} plane width does not fit in usize ({width})"),
    })?;
    let height_usize =
        usize::try_from(height).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("{plane} plane height does not fit in usize ({height})"),
        })?;
    let crop_left_usize =
        usize::try_from(crop_left).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("{plane} plane crop_left does not fit in usize ({crop_left})"),
        })?;
    let crop_top_usize =
        usize::try_from(crop_top).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("{plane} plane crop_top does not fit in usize ({crop_top})"),
        })?;

    let row_end = crop_left_usize
        .checked_add(width_usize)
        .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "{plane} plane row bound overflows: crop_left={crop_left_usize}, width={width_usize}"
            ),
        })?;
    if row_end > stride {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "{plane} plane stride {stride} smaller than crop+width bound {row_end}"
            ),
        });
    }

    let expected_samples = width_usize.checked_mul(height_usize).ok_or_else(|| {
        DecodeHeicError::InvalidDecodedFrame {
            detail: format!("{plane} plane sample count overflow for {width_usize}x{height_usize}"),
        }
    })?;
    let mut samples = Vec::with_capacity(expected_samples);

    for row in 0..height_usize {
        let src_row = crop_top_usize.checked_add(row).ok_or_else(|| {
            DecodeHeicError::InvalidDecodedFrame {
                detail: format!(
                    "{plane} plane row index overflow: crop_top={crop_top_usize}, row={row}"
                ),
            }
        })?;
        let src_start = src_row
            .checked_mul(stride)
            .and_then(|offset| offset.checked_add(crop_left_usize))
            .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                detail: format!(
                    "{plane} plane source index overflow at row {row} (stride={stride}, crop_left={crop_left_usize})"
                ),
            })?;
        let src_end = src_start
            .checked_add(width_usize)
            .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                detail: format!(
                    "{plane} plane source row end overflow at row {row} (start={src_start}, width={width_usize})"
                ),
            })?;
        if src_end > source.len() {
            return Err(DecodeHeicError::InvalidDecodedFrame {
                detail: format!(
                    "{plane} plane row {row} exceeds decoded buffer: end={src_end}, available={}",
                    source.len()
                ),
            });
        }
        samples.extend_from_slice(&source[src_start..src_end]);
    }

    Ok(HeicPlane {
        width,
        height,
        samples,
    })
}

fn assemble_heic_hevc_stream_from_components(
    hvcc: &isobmff::HevcDecoderConfigurationBox,
    payload: &[u8],
) -> Result<Vec<u8>, DecodeHeicError> {
    let nal_length_size = hvcc.nal_length_size;
    if !(1..=4).contains(&nal_length_size) {
        return Err(DecodeHeicError::InvalidNalLengthSize { nal_length_size });
    }

    let mut stream = Vec::new();
    append_hvcc_header_nals(&hvcc.nal_arrays, &mut stream)?;
    append_normalized_hevc_payload_nals(payload, usize::from(nal_length_size), &mut stream)?;
    Ok(stream)
}

fn parse_length_prefixed_hevc_nal_units(
    stream: &[u8],
) -> Result<Vec<LengthPrefixedHevcNalUnit<'_>>, DecodeHeicError> {
    let mut units = Vec::new();
    let mut cursor = 0usize;
    while cursor < stream.len() {
        let length_offset = cursor;
        let remaining = stream.len() - cursor;
        if remaining < 4 {
            return Err(DecodeHeicError::TruncatedLengthPrefixedStreamLength {
                offset: length_offset,
                available: remaining,
            });
        }

        let nal_size = u32::from_be_bytes([
            stream[cursor],
            stream[cursor + 1],
            stream[cursor + 2],
            stream[cursor + 3],
        ]) as usize;
        cursor += 4;

        let available = stream.len() - cursor;
        if available < nal_size {
            return Err(DecodeHeicError::TruncatedLengthPrefixedStreamNalUnit {
                offset: cursor,
                declared: nal_size,
                available,
            });
        }

        let nal_offset = cursor;
        let nal_end = cursor + nal_size;
        units.push(LengthPrefixedHevcNalUnit {
            offset: nal_offset,
            bytes: &stream[nal_offset..nal_end],
        });
        cursor = nal_end;
    }

    Ok(units)
}

fn decode_hevc_stream_metadata_from_sps(
    stream: &[u8],
) -> Result<DecodedHeicImageMetadata, DecodeHeicError> {
    // Provenance: length-prefixed NAL iteration mirrors libheif's decoder
    // plugin handoff loop in libheif/libheif/plugins/decoder_libde265.cc
    // (libde265_v2_push_data/libde265_v1_push_data2), while SPS parsing is
    // delegated to the pure-Rust scuffle-h265 backend.
    for nal_unit in parse_length_prefixed_hevc_nal_units(stream)? {
        if nal_unit.class() != HevcNalClass::ParameterSet {
            continue;
        }
        if nal_unit.nal_unit_type() != Some(NALUnitType::SpsNut) {
            continue;
        }
        let nal_offset = nal_unit.offset;

        let sps_nal = SpsNALUnit::parse(std::io::Cursor::new(nal_unit.bytes)).map_err(|err| {
            DecodeHeicError::SpsParseFailed {
                offset: nal_offset,
                detail: err.to_string(),
            }
        })?;
        let width_raw = sps_nal.rbsp.cropped_width();
        let height_raw = sps_nal.rbsp.cropped_height();
        if width_raw == 0 || height_raw == 0 {
            return Err(DecodeHeicError::InvalidSpsGeometry {
                width: width_raw,
                height: height_raw,
            });
        }

        let width = u32::try_from(width_raw).map_err(|_| DecodeHeicError::InvalidSpsGeometry {
            width: width_raw,
            height: height_raw,
        })?;
        let height =
            u32::try_from(height_raw).map_err(|_| DecodeHeicError::InvalidSpsGeometry {
                width: width_raw,
                height: height_raw,
            })?;
        let layout = heic_layout_from_sps_chroma_array_type(sps_nal.rbsp.chroma_array_type())?;

        return Ok(DecodedHeicImageMetadata {
            width,
            height,
            bit_depth_luma: sps_nal.rbsp.bit_depth_y(),
            bit_depth_chroma: sps_nal.rbsp.bit_depth_c(),
            layout,
        });
    }

    Err(DecodeHeicError::MissingSpsNalUnit)
}

fn heic_layout_from_sps_chroma_array_type(
    chroma_array_type: u8,
) -> Result<HeicPixelLayout, DecodeHeicError> {
    match chroma_array_type {
        0 => Ok(HeicPixelLayout::Yuv400),
        1 => Ok(HeicPixelLayout::Yuv420),
        2 => Ok(HeicPixelLayout::Yuv422),
        3 => Ok(HeicPixelLayout::Yuv444),
        _ => Err(DecodeHeicError::UnsupportedSpsChromaArrayType { chroma_array_type }),
    }
}

/// Decode a HEIF/HEIC/AVIF image from `input_path` and write a PNG to `output_path`.
pub fn decode_file_to_png(input_path: &Path, output_path: &Path) -> Result<(), DecodeError> {
    if !input_path.exists() {
        return Err(DecodeError::Unsupported(format!(
            "Input file does not exist: {}",
            input_path.display()
        )));
    }

    let extension = input_path.extension().and_then(|ext| ext.to_str());
    if matches!(extension, Some(ext) if ext.eq_ignore_ascii_case("avif")) {
        let input = std::fs::read(input_path)?;
        let decoded = decode_primary_avif_to_image(&input)?;
        write_decoded_avif_to_png(&decoded, output_path)?;
        return Ok(());
    }

    if matches!(extension, Some(ext) if ext.eq_ignore_ascii_case("heic") || ext.eq_ignore_ascii_case("heif"))
    {
        let input = std::fs::read(input_path)?;
        let decoded = decode_primary_heic_to_image(&input)?;
        write_decoded_heic_to_png(&decoded, output_path)?;
        return Ok(());
    }

    Err(DecodeError::Unsupported(format!(
        "Unsupported file extension for input: {}",
        input_path.display()
    )))
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

fn write_decoded_heic_to_png(
    decoded: &DecodedHeicImage,
    output_path: &Path,
) -> Result<(), DecodeError> {
    if decoded.bit_depth_luma <= 8 {
        let pixels = convert_heic_to_rgba8(decoded)?;
        return write_rgba8_png(decoded.width, decoded.height, &pixels, output_path);
    }

    let pixels = convert_heic_to_rgba16(decoded)?;
    write_rgba16_png(decoded.width, decoded.height, &pixels, output_path)
}

fn append_hvcc_header_nals(
    nal_arrays: &[isobmff::HevcNalArray],
    stream: &mut Vec<u8>,
) -> Result<(), DecodeHeicError> {
    for nal_array in nal_arrays {
        for nal_unit in &nal_array.nal_units {
            append_nal_with_u32_length_prefix(nal_unit, stream)?;
        }
    }

    Ok(())
}

fn append_normalized_hevc_payload_nals(
    payload: &[u8],
    nal_length_size: usize,
    stream: &mut Vec<u8>,
) -> Result<(), DecodeHeicError> {
    let mut cursor = 0usize;
    while cursor < payload.len() {
        let length_field_start = cursor;
        let remaining = payload.len() - cursor;
        if remaining < nal_length_size {
            return Err(DecodeHeicError::TruncatedNalLengthField {
                offset: length_field_start,
                nal_length_size: nal_length_size as u8,
                available: remaining,
            });
        }

        let mut nal_size: usize = 0;
        for byte in &payload[cursor..cursor + nal_length_size] {
            nal_size = (nal_size << 8) | usize::from(*byte);
        }
        cursor += nal_length_size;

        let available = payload.len() - cursor;
        if available < nal_size {
            return Err(DecodeHeicError::TruncatedNalUnit {
                offset: cursor,
                declared: nal_size,
                available,
            });
        }

        let nal_end = cursor + nal_size;
        append_nal_with_u32_length_prefix(&payload[cursor..nal_end], stream)?;
        cursor = nal_end;
    }

    Ok(())
}

fn append_nal_with_u32_length_prefix(
    nal_unit: &[u8],
    stream: &mut Vec<u8>,
) -> Result<(), DecodeHeicError> {
    let nal_size = nal_unit.len();
    let nal_size_u32 =
        u32::try_from(nal_size).map_err(|_| DecodeHeicError::NalUnitTooLarge { nal_size })?;
    stream.extend_from_slice(&nal_size_u32.to_be_bytes());
    stream.extend_from_slice(nal_unit);
    Ok(())
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

fn convert_heic_to_rgba8(decoded: &DecodedHeicImage) -> Result<Vec<u8>, DecodeHeicError> {
    let bit_depth = heic_bit_depth_for_png_conversion(decoded)?;

    validate_heic_plane_dimensions(&decoded.y_plane, decoded.width, decoded.height, "Y")?;
    let expected_y_samples = heic_sample_count(decoded.width, decoded.height, "Y")?;
    if decoded.y_plane.samples.len() != expected_y_samples {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "Y plane has {} samples, expected {expected_y_samples}",
                decoded.y_plane.samples.len()
            ),
        });
    }

    let width =
        usize::try_from(decoded.width).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("HEIC width does not fit in usize ({})", decoded.width),
        })?;
    let height =
        usize::try_from(decoded.height).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("HEIC height does not fit in usize ({})", decoded.height),
        })?;
    let output_len =
        expected_y_samples
            .checked_mul(4)
            .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                detail: format!(
                    "RGBA output sample count overflow for {}x{}",
                    decoded.width, decoded.height
                ),
            })?;
    let mut out = Vec::with_capacity(output_len);

    let chroma = prepare_heic_chroma(decoded)?;
    let half_range = chroma_half_range(bit_depth);

    for y in 0..height {
        let row_start =
            y.checked_mul(width)
                .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                    detail: format!(
                        "row offset overflow while converting HEIC at row {y} (width={width})"
                    ),
                })?;

        for x in 0..width {
            let y_index =
                row_start
                    .checked_add(x)
                    .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                        detail: format!("luma index overflow while converting HEIC at ({x}, {y})"),
                    })?;
            let y_sample = i32::from(decoded.y_plane.samples[y_index]);

            let (cb_centered, cr_centered) = match &chroma {
                HeicChromaPlanes::Monochrome => (0, 0),
                HeicChromaPlanes::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = heic_chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]) - half_range,
                        i32::from(v_samples[chroma_index]) - half_range,
                    )
                }
            };

            let (r, g, b) = ycbcr_to_rgb_components(y_sample, cb_centered, cr_centered, bit_depth);
            out.push(scale_sample_to_u8(r, bit_depth));
            out.push(scale_sample_to_u8(g, bit_depth));
            out.push(scale_sample_to_u8(b, bit_depth));
            out.push(u8::MAX);
        }
    }

    Ok(out)
}

fn convert_heic_to_rgba16(decoded: &DecodedHeicImage) -> Result<Vec<u16>, DecodeHeicError> {
    let bit_depth = heic_bit_depth_for_png_conversion(decoded)?;

    validate_heic_plane_dimensions(&decoded.y_plane, decoded.width, decoded.height, "Y")?;
    let expected_y_samples = heic_sample_count(decoded.width, decoded.height, "Y")?;
    if decoded.y_plane.samples.len() != expected_y_samples {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "Y plane has {} samples, expected {expected_y_samples}",
                decoded.y_plane.samples.len()
            ),
        });
    }

    let width =
        usize::try_from(decoded.width).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("HEIC width does not fit in usize ({})", decoded.width),
        })?;
    let height =
        usize::try_from(decoded.height).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("HEIC height does not fit in usize ({})", decoded.height),
        })?;
    let output_len =
        expected_y_samples
            .checked_mul(4)
            .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                detail: format!(
                    "RGBA output sample count overflow for {}x{}",
                    decoded.width, decoded.height
                ),
            })?;
    let mut out = Vec::with_capacity(output_len);

    let chroma = prepare_heic_chroma(decoded)?;
    let half_range = chroma_half_range(bit_depth);

    for y in 0..height {
        let row_start =
            y.checked_mul(width)
                .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                    detail: format!(
                        "row offset overflow while converting HEIC at row {y} (width={width})"
                    ),
                })?;

        for x in 0..width {
            let y_index =
                row_start
                    .checked_add(x)
                    .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
                        detail: format!("luma index overflow while converting HEIC at ({x}, {y})"),
                    })?;
            let y_sample = i32::from(decoded.y_plane.samples[y_index]);

            let (cb_centered, cr_centered) = match &chroma {
                HeicChromaPlanes::Monochrome => (0, 0),
                HeicChromaPlanes::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = heic_chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]) - half_range,
                        i32::from(v_samples[chroma_index]) - half_range,
                    )
                }
            };

            let (r, g, b) = ycbcr_to_rgb_components(y_sample, cb_centered, cr_centered, bit_depth);
            out.push(scale_sample_to_u16(r, bit_depth));
            out.push(scale_sample_to_u16(g, bit_depth));
            out.push(scale_sample_to_u16(b, bit_depth));
            out.push(u16::MAX);
        }
    }

    Ok(out)
}

enum HeicChromaPlanes<'a> {
    Monochrome,
    Color {
        u_samples: &'a [u16],
        v_samples: &'a [u16],
        chroma_width: usize,
        layout: HeicPixelLayout,
    },
}

fn heic_bit_depth_for_png_conversion(decoded: &DecodedHeicImage) -> Result<u8, DecodeHeicError> {
    if decoded.bit_depth_luma != decoded.bit_depth_chroma {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "HEIC luma/chroma bit-depth mismatch during PNG conversion: {}/{}",
                decoded.bit_depth_luma, decoded.bit_depth_chroma
            ),
        });
    }

    if decoded.bit_depth_luma == 0 || decoded.bit_depth_luma > 16 {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "HEIC bit depth {} is outside supported PNG conversion range 1..=16",
                decoded.bit_depth_luma
            ),
        });
    }

    Ok(decoded.bit_depth_luma)
}

fn prepare_heic_chroma(
    decoded: &DecodedHeicImage,
) -> Result<HeicChromaPlanes<'_>, DecodeHeicError> {
    if decoded.layout == HeicPixelLayout::Yuv400 {
        return Ok(HeicChromaPlanes::Monochrome);
    }

    let (u_plane, v_plane, expected_width, expected_height) = require_heic_chroma_planes(decoded)?;
    validate_heic_plane_dimensions(u_plane, expected_width, expected_height, "U")?;
    validate_heic_plane_dimensions(v_plane, expected_width, expected_height, "V")?;

    let expected_samples = heic_sample_count(expected_width, expected_height, "U/V")?;
    if u_plane.samples.len() != expected_samples {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "U plane has {} samples, expected {expected_samples}",
                u_plane.samples.len()
            ),
        });
    }
    if v_plane.samples.len() != expected_samples {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "V plane has {} samples, expected {expected_samples}",
                v_plane.samples.len()
            ),
        });
    }

    let chroma_width =
        usize::try_from(expected_width).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("HEIC chroma width does not fit in usize ({expected_width})"),
        })?;
    Ok(HeicChromaPlanes::Color {
        u_samples: &u_plane.samples,
        v_samples: &v_plane.samples,
        chroma_width,
        layout: decoded.layout,
    })
}

fn require_heic_chroma_planes(
    decoded: &DecodedHeicImage,
) -> Result<(&HeicPlane, &HeicPlane, u32, u32), DecodeHeicError> {
    let (expected_width, expected_height) =
        heic_chroma_dimensions(decoded.width, decoded.height, decoded.layout);
    let u_plane = decoded
        .u_plane
        .as_ref()
        .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "decoded HEIC frame is missing U plane for {:?}",
                decoded.layout
            ),
        })?;
    let v_plane = decoded
        .v_plane
        .as_ref()
        .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "decoded HEIC frame is missing V plane for {:?}",
                decoded.layout
            ),
        })?;
    Ok((u_plane, v_plane, expected_width, expected_height))
}

fn validate_heic_plane_dimensions(
    plane: &HeicPlane,
    expected_width: u32,
    expected_height: u32,
    plane_name: &'static str,
) -> Result<(), DecodeHeicError> {
    if plane.width != expected_width || plane.height != expected_height {
        return Err(DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "{plane_name} plane has dimensions {}x{}, expected {expected_width}x{expected_height}",
                plane.width, plane.height
            ),
        });
    }

    Ok(())
}

fn heic_sample_count(
    width: u32,
    height: u32,
    plane_name: &'static str,
) -> Result<usize, DecodeHeicError> {
    let width_usize = usize::try_from(width).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
        detail: format!("{plane_name} plane width does not fit in usize ({width})"),
    })?;
    let height_usize =
        usize::try_from(height).map_err(|_| DecodeHeicError::InvalidDecodedFrame {
            detail: format!("{plane_name} plane height does not fit in usize ({height})"),
        })?;
    width_usize
        .checked_mul(height_usize)
        .ok_or_else(|| DecodeHeicError::InvalidDecodedFrame {
            detail: format!(
                "{plane_name} plane sample count overflow for {width_usize}x{height_usize}"
            ),
        })
}

fn heic_chroma_dimensions(width: u32, height: u32, layout: HeicPixelLayout) -> (u32, u32) {
    if layout == HeicPixelLayout::Yuv400 {
        return (0, 0);
    }

    let (subsample_x, subsample_y) = heic_chroma_subsampling(layout);
    (width.div_ceil(subsample_x), height.div_ceil(subsample_y))
}

fn heic_chroma_sample_index(
    x: usize,
    y: usize,
    chroma_width: usize,
    layout: HeicPixelLayout,
) -> usize {
    match layout {
        HeicPixelLayout::Yuv400 => 0,
        HeicPixelLayout::Yuv420 => (y / 2) * chroma_width + (x / 2),
        HeicPixelLayout::Yuv422 => y * chroma_width + (x / 2),
        HeicPixelLayout::Yuv444 => y * chroma_width + x,
    }
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
        append_normalized_hevc_payload_nals, assemble_primary_heic_hevc_stream,
        convert_avif_to_rgba8, decode_file_to_png, decode_hevc_stream_metadata_from_sps,
        decode_hevc_stream_to_image, decode_primary_avif_to_image, decode_primary_heic_to_image,
        decode_primary_heic_to_metadata, parse_length_prefixed_hevc_nal_units,
        validate_decoded_heic_image_against_metadata, AvifPixelLayout, AvifPlane, AvifPlaneSamples,
        DecodeHeicError, DecodedAvifImage, DecodedHeicImage, DecodedHeicImageMetadata,
        HeicPixelLayout, HeicPlane, HevcNalClass,
    };
    use scuffle_h265::NALUnitType;
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
    fn decodes_example_heic_to_png() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libheif/examples/example.heic");
        let input = std::fs::read(&fixture).expect("example.heic fixture must be readable");
        let decoded =
            decode_primary_heic_to_image(&input).expect("example HEIC should decode into planes");

        let output = test_output_png_path("example-heic");
        let _guard = TempFileGuard(output.clone());
        decode_file_to_png(&fixture, &output).expect("HEIC decode should write PNG");

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
        let expected_bit_depth = if decoded.bit_depth_luma <= 8 {
            png::BitDepth::Eight
        } else {
            png::BitDepth::Sixteen
        };
        assert_eq!(frame_info.bit_depth, expected_bit_depth);
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

    #[test]
    fn assembles_primary_heic_stream_for_fixture_without_pixi_property() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../libheif/fuzzing/data/corpus/colors-no-alpha.heic");
        let input = std::fs::read(&fixture).expect("HEIC fixture must be readable");

        let stream = assemble_primary_heic_hevc_stream(&input)
            .expect("fixture without pixi should still assemble a decoder stream");
        assert!(
            !stream.is_empty(),
            "assembled HEIC stream should not be empty"
        );
        let metadata = decode_hevc_stream_metadata_from_sps(&stream)
            .expect("assembled stream should expose SPS metadata");
        assert!(metadata.width > 0);
        assert!(metadata.height > 0);
    }

    #[test]
    fn parses_heic_sps_metadata_from_length_prefixed_stream() {
        let sps_nal = b"\x42\x01\x01\x01\x40\x00\x00\x03\x00\x90\x00\x00\x03\x00\x00\x03\x00\x78\xa0\x03\xc0\x80\x11\x07\xcb\x96\xb4\xa4\x25\x92\xe3\x01\x6a\x02\x02\x02\x08\x00\x00\x03\x00\x08\x00\x00\x03\x00\xf3\x00\x2e\xf2\x88\x00\x02\x62\x5a\x00\x00\x13\x12\xd0\x20";
        let mut stream = Vec::new();
        stream.extend_from_slice(&(sps_nal.len() as u32).to_be_bytes());
        stream.extend_from_slice(sps_nal);

        let metadata = decode_hevc_stream_metadata_from_sps(&stream)
            .expect("valid SPS NAL should decode metadata");
        assert_eq!(metadata.width, 1920);
        assert_eq!(metadata.height, 1080);
        assert_eq!(metadata.bit_depth_luma, 8);
        assert_eq!(metadata.bit_depth_chroma, 8);
        assert_eq!(metadata.layout, HeicPixelLayout::Yuv420);
    }

    #[test]
    fn parses_length_prefixed_hevc_nal_units_and_classifies_for_backend_handoff() {
        let vps_nal = [0x40, 0x01, 0x01];
        let sps_nal = [0x42, 0x01, 0x01];
        let vcl_nal = [0x26, 0x01, 0x01];
        let unknown_nal = [0x7E];
        let mut stream = Vec::new();
        for nal in [&vps_nal[..], &sps_nal[..], &vcl_nal[..], &unknown_nal[..]] {
            stream.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            stream.extend_from_slice(nal);
        }

        let parsed = parse_length_prefixed_hevc_nal_units(&stream)
            .expect("well-formed stream should parse into NAL units");
        assert_eq!(parsed.len(), 4);

        assert_eq!(parsed[0].offset, 4);
        assert_eq!(parsed[0].nal_unit_type(), Some(NALUnitType::VpsNut));
        assert_eq!(parsed[0].class(), HevcNalClass::ParameterSet);

        assert_eq!(parsed[1].offset, 11);
        assert_eq!(parsed[1].nal_unit_type(), Some(NALUnitType::SpsNut));
        assert_eq!(parsed[1].class(), HevcNalClass::ParameterSet);

        assert_eq!(parsed[2].offset, 18);
        assert_eq!(parsed[2].class(), HevcNalClass::Vcl);

        assert_eq!(parsed[3].offset, 25);
        assert_eq!(parsed[3].nal_unit_type(), None);
        assert_eq!(parsed[3].class(), HevcNalClass::Unknown);
    }

    #[test]
    fn rejects_truncated_length_prefixed_hevc_stream_length_field_during_nal_parsing() {
        let stream = vec![0x00, 0x00, 0x00];
        let err = parse_length_prefixed_hevc_nal_units(&stream)
            .expect_err("stream with short length prefix must fail");
        assert!(matches!(
            err,
            DecodeHeicError::TruncatedLengthPrefixedStreamLength {
                offset: 0,
                available: 3,
            }
        ));
    }

    #[test]
    fn decodes_primary_heic_metadata_for_fixture_without_pixi_property() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libheif/examples/example.heic");
        let input = std::fs::read(&fixture).expect("HEIC fixture must be readable");
        let preflight = crate::isobmff::parse_primary_heic_item_preflight_properties(&input)
            .expect("HEIC preflight should parse fixture even without pixi");
        assert!(
            preflight.pixi.is_none(),
            "fixture should exercise missing-pixi preflight path"
        );

        let metadata = decode_primary_heic_to_metadata(&input)
            .expect("fixture without pixi should still decode HEIC SPS metadata");
        assert_eq!(metadata.width, preflight.ispe.width);
        assert_eq!(metadata.height, preflight.ispe.height);
        assert!(metadata.bit_depth_luma >= 8);
        assert!(metadata.bit_depth_chroma >= 8);
    }

    #[test]
    fn decodes_primary_heic_image_for_fixture_without_pixi_property() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libheif/examples/example.heic");
        let input = std::fs::read(&fixture).expect("HEIC fixture must be readable");
        let metadata = decode_primary_heic_to_metadata(&input)
            .expect("fixture metadata should decode before image-model validation");

        let decoded = decode_primary_heic_to_image(&input)
            .expect("fixture without pixi should decode to planar HEIC image");
        assert_eq!(decoded.width, metadata.width);
        assert_eq!(decoded.height, metadata.height);
        assert_eq!(decoded.bit_depth_luma, metadata.bit_depth_luma);
        assert_eq!(decoded.bit_depth_chroma, metadata.bit_depth_chroma);
        assert_eq!(decoded.layout, metadata.layout);
        assert_heic_plane_shapes(&decoded);
    }

    #[test]
    fn decodes_assembled_heic_stream_for_fixture_into_image_model() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libheif/examples/example.heic");
        let input = std::fs::read(&fixture).expect("HEIC fixture must be readable");
        let stream = assemble_primary_heic_hevc_stream(&input)
            .expect("fixture should assemble into backend-ready HEVC stream");

        let decoded = decode_hevc_stream_to_image(&stream)
            .expect("assembled fixture stream should decode through HEVC backend");
        assert!(decoded.width > 0);
        assert!(decoded.height > 0);
        assert!(decoded.bit_depth_luma >= 8);
        assert!(decoded.bit_depth_chroma >= 8);
        assert_heic_plane_shapes(&decoded);
    }

    #[test]
    fn rejects_decoded_heic_image_with_layout_mismatch_against_metadata() {
        let decoded = DecodedHeicImage {
            width: 2,
            height: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            layout: HeicPixelLayout::Yuv400,
            y_plane: HeicPlane {
                width: 2,
                height: 1,
                samples: vec![0, 0],
            },
            u_plane: None,
            v_plane: None,
        };
        let metadata = DecodedHeicImageMetadata {
            width: 2,
            height: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            layout: HeicPixelLayout::Yuv420,
        };

        let err = validate_decoded_heic_image_against_metadata(&decoded, &metadata)
            .expect_err("layout mismatch should fail deterministic metadata validation");
        assert!(matches!(
            err,
            DecodeHeicError::DecodedLayoutMismatch {
                expected: HeicPixelLayout::Yuv420,
                actual: HeicPixelLayout::Yuv400,
            }
        ));
    }

    #[test]
    fn rejects_decoded_heic_image_with_bit_depth_mismatch_against_metadata() {
        let decoded = DecodedHeicImage {
            width: 2,
            height: 1,
            bit_depth_luma: 10,
            bit_depth_chroma: 10,
            layout: HeicPixelLayout::Yuv400,
            y_plane: HeicPlane {
                width: 2,
                height: 1,
                samples: vec![0, 0],
            },
            u_plane: None,
            v_plane: None,
        };
        let metadata = DecodedHeicImageMetadata {
            width: 2,
            height: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            layout: HeicPixelLayout::Yuv400,
        };

        let err = validate_decoded_heic_image_against_metadata(&decoded, &metadata)
            .expect_err("bit-depth mismatch should fail deterministic metadata validation");
        assert!(matches!(
            err,
            DecodeHeicError::DecodedBitDepthMismatch {
                expected_luma: 8,
                expected_chroma: 8,
                actual_luma: 10,
                actual_chroma: 10,
            }
        ));
    }

    #[test]
    fn reports_missing_sps_for_length_prefixed_stream_without_sps_nal() {
        // NAL header with nal_unit_type=34 (PPS), layer_id=0, temporal_id_plus1=1.
        let pps_nal = [0x44, 0x01, 0x80];
        let mut stream = Vec::new();
        stream.extend_from_slice(&(pps_nal.len() as u32).to_be_bytes());
        stream.extend_from_slice(&pps_nal);

        let err = decode_hevc_stream_metadata_from_sps(&stream)
            .expect_err("stream without SPS NAL should fail metadata decode");
        assert!(matches!(err, DecodeHeicError::MissingSpsNalUnit));
    }

    #[test]
    fn rejects_truncated_length_prefixed_hevc_stream_nal_unit_during_nal_parsing() {
        let stream = vec![0x00, 0x00, 0x00, 0x03, 0x42, 0x01];
        let err = parse_length_prefixed_hevc_nal_units(&stream)
            .expect_err("stream with short NAL payload must fail");
        assert!(matches!(
            err,
            DecodeHeicError::TruncatedLengthPrefixedStreamNalUnit {
                offset: 4,
                declared: 3,
                available: 2,
            }
        ));
    }

    #[test]
    fn reports_missing_vcl_when_decoding_heic_stream_image() {
        let sps_nal = [0x42, 0x01, 0x01];
        let mut stream = Vec::new();
        stream.extend_from_slice(&(sps_nal.len() as u32).to_be_bytes());
        stream.extend_from_slice(&sps_nal);

        let err = decode_hevc_stream_to_image(&stream)
            .expect_err("stream without VCL NAL must fail backend image decode");
        assert!(matches!(err, DecodeHeicError::MissingVclNalUnit));
    }

    #[test]
    fn reports_backend_decode_failure_for_malformed_vcl_stream() {
        let sps_nal = [0x42, 0x01, 0x01];
        let vcl_nal = [0x26, 0x01, 0x01];
        let mut stream = Vec::new();
        for nal in [&sps_nal[..], &vcl_nal[..]] {
            stream.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            stream.extend_from_slice(nal);
        }

        let err = decode_hevc_stream_to_image(&stream)
            .expect_err("malformed stream with VCL should fail inside backend decode");
        assert!(matches!(err, DecodeHeicError::BackendDecodeFailed { .. }));
    }

    #[test]
    fn normalizes_two_byte_nal_lengths_to_four_byte_stream() {
        let payload = vec![0x00, 0x03, 0x41, 0x42, 0x43, 0x00, 0x01, 0x99];
        let mut stream = Vec::new();
        append_normalized_hevc_payload_nals(&payload, 2, &mut stream)
            .expect("2-byte HEVC NAL lengths should normalize to 4-byte lengths");

        assert_eq!(
            stream,
            vec![0x00, 0x00, 0x00, 0x03, 0x41, 0x42, 0x43, 0x00, 0x00, 0x00, 0x01, 0x99,]
        );
    }

    #[test]
    fn rejects_truncated_normalized_hevc_payload_nal_length_field() {
        let payload = vec![0x00];
        let mut stream = Vec::new();
        let err = append_normalized_hevc_payload_nals(&payload, 2, &mut stream)
            .expect_err("truncated NAL length field must fail");
        assert!(matches!(
            err,
            DecodeHeicError::TruncatedNalLengthField {
                offset: 0,
                nal_length_size: 2,
                available: 1,
            }
        ));
    }

    #[test]
    fn rejects_truncated_normalized_hevc_payload_nal_unit() {
        let payload = vec![0x00, 0x02, 0xAA];
        let mut stream = Vec::new();
        let err = append_normalized_hevc_payload_nals(&payload, 2, &mut stream)
            .expect_err("truncated NAL payload must fail");
        assert!(matches!(
            err,
            DecodeHeicError::TruncatedNalUnit {
                offset: 2,
                declared: 2,
                available: 1,
            }
        ));
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

    fn assert_heic_plane_shapes(decoded: &DecodedHeicImage) {
        let y_expected = decoded.width as usize * decoded.height as usize;
        assert_eq!(decoded.y_plane.width, decoded.width);
        assert_eq!(decoded.y_plane.height, decoded.height);
        assert_eq!(decoded.y_plane.samples.len(), y_expected);

        match decoded.layout {
            HeicPixelLayout::Yuv400 => {
                assert!(decoded.u_plane.is_none());
                assert!(decoded.v_plane.is_none());
            }
            HeicPixelLayout::Yuv420 | HeicPixelLayout::Yuv422 | HeicPixelLayout::Yuv444 => {
                let (chroma_width, chroma_height) =
                    heic_chroma_dimensions(decoded.width, decoded.height, decoded.layout);
                let expected_chroma_samples = chroma_width as usize * chroma_height as usize;
                let u_plane = decoded.u_plane.as_ref().expect("U plane should exist");
                let v_plane = decoded.v_plane.as_ref().expect("V plane should exist");
                assert_eq!(u_plane.width, chroma_width);
                assert_eq!(u_plane.height, chroma_height);
                assert_eq!(v_plane.width, chroma_width);
                assert_eq!(v_plane.height, chroma_height);
                assert_eq!(u_plane.samples.len(), expected_chroma_samples);
                assert_eq!(v_plane.samples.len(), expected_chroma_samples);
            }
        }
    }

    fn heic_chroma_dimensions(width: u32, height: u32, layout: HeicPixelLayout) -> (u32, u32) {
        match layout {
            HeicPixelLayout::Yuv400 => (0, 0),
            HeicPixelLayout::Yuv420 => (width.div_ceil(2), height.div_ceil(2)),
            HeicPixelLayout::Yuv422 => (width.div_ceil(2), height),
            HeicPixelLayout::Yuv444 => (width, height),
        }
    }
}
