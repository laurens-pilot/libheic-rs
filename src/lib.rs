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
use std::borrow::Cow;
use std::error::Error;
use std::ffi::c_void;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::BufWriter;
use std::mem::MaybeUninit;
use std::path::Path;
use std::ptr::{self, NonNull};

pub mod isobmff;

/// Stable high-level decoder error categories for callers and tooling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeErrorCategory {
    Io,
    Parse,
    MalformedInput,
    UnsupportedFeature,
    DecoderBackend,
    OutputEncoding,
}

/// Errors returned by the decoder entry points.
#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    AvifDecode(DecodeAvifError),
    HeicDecode(DecodeHeicError),
    PngEncoding(png::EncodingError),
    TransformGuard(TransformGuardError),
    OutputBufferOverflow {
        buffer_name: &'static str,
        element_count: usize,
        element_size_bytes: usize,
    },
    Unsupported(String),
}

/// Structured transform/input validation failures in the RGBA output path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransformGuardError {
    RgbaSampleCountMismatch {
        stage: &'static str,
        actual: usize,
        expected: usize,
        width: u32,
        height: u32,
    },
    PixelCountOverflow {
        width: u32,
        height: u32,
    },
    SampleCountOverflow {
        width: u32,
        height: u32,
    },
    SampleCountExceedsAddressSpace {
        width: u32,
        height: u32,
    },
    UnsupportedRotation {
        rotation_ccw_degrees: u16,
    },
    DimensionTooLargeForPlatform {
        stage: &'static str,
        dimension: &'static str,
        value: u64,
    },
    PixelIndexOverflow {
        stage: &'static str,
        x: usize,
        y: usize,
        width: u32,
        height: u32,
    },
    EmptyImageGeometry {
        width: u32,
        height: u32,
    },
    InvalidCleanApertureBounds {
        width: u32,
        height: u32,
        left: i128,
        right: i128,
        top: i128,
        bottom: i128,
    },
    CleanApertureCropDimensionOutOfRange {
        dimension: &'static str,
        value: i128,
    },
    CleanApertureBoundOutOfRange {
        bound: &'static str,
        value: i128,
    },
    CleanApertureRowOffsetOverflow {
        stage: &'static str,
        y: usize,
        width: u32,
        height: u32,
    },
}

impl TransformGuardError {
    fn category(&self) -> DecodeErrorCategory {
        match self {
            TransformGuardError::UnsupportedRotation { .. } => {
                DecodeErrorCategory::UnsupportedFeature
            }
            _ => DecodeErrorCategory::MalformedInput,
        }
    }
}

impl Display for TransformGuardError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformGuardError::RgbaSampleCountMismatch {
                stage,
                actual,
                expected,
                width,
                height,
            } => write!(
                f,
                "RGBA sample count mismatch for {stage}: got {actual}, expected {expected} for {width}x{height}"
            ),
            TransformGuardError::PixelCountOverflow { width, height } => write!(
                f,
                "RGBA pixel count overflow for dimensions {width}x{height}"
            ),
            TransformGuardError::SampleCountOverflow { width, height } => write!(
                f,
                "RGBA sample count overflow for dimensions {width}x{height}"
            ),
            TransformGuardError::SampleCountExceedsAddressSpace { width, height } => write!(
                f,
                "RGBA sample count does not fit in memory on this platform for dimensions {width}x{height}"
            ),
            TransformGuardError::UnsupportedRotation {
                rotation_ccw_degrees,
            } => write!(
                f,
                "unsupported irot rotation angle {rotation_ccw_degrees} degrees"
            ),
            TransformGuardError::DimensionTooLargeForPlatform {
                stage,
                dimension,
                value,
            } => write!(
                f,
                "{stage} {dimension} does not fit in usize ({value}) while applying transform"
            ),
            TransformGuardError::PixelIndexOverflow {
                stage,
                x,
                y,
                width,
                height,
            } => write!(
                f,
                "{stage} pixel index overflow at ({x}, {y}) for {width}x{height} image"
            ),
            TransformGuardError::EmptyImageGeometry { width, height } => write!(
                f,
                "cannot apply clean aperture to empty image geometry {width}x{height}"
            ),
            TransformGuardError::InvalidCleanApertureBounds {
                width,
                height,
                left,
                right,
                top,
                bottom,
            } => write!(
                f,
                "invalid clean aperture crop bounds after clamping for {width}x{height} image: left={left}, right={right}, top={top}, bottom={bottom}"
            ),
            TransformGuardError::CleanApertureCropDimensionOutOfRange { dimension, value } => {
                write!(
                    f,
                    "clean aperture crop {dimension} does not fit in u32 ({value})"
                )
            }
            TransformGuardError::CleanApertureBoundOutOfRange { bound, value } => write!(
                f,
                "clean aperture {bound} bound does not fit in usize ({value})"
            ),
            TransformGuardError::CleanApertureRowOffsetOverflow {
                stage,
                y,
                width,
                height,
            } => write!(
                f,
                "clean aperture {stage} overflow at y={y} for {width}x{height} image"
            ),
        }
    }
}

impl DecodeError {
    /// Return the stable high-level category for this decode failure.
    pub fn category(&self) -> DecodeErrorCategory {
        match self {
            DecodeError::Io(_) => DecodeErrorCategory::Io,
            DecodeError::AvifDecode(err) => err.category(),
            DecodeError::HeicDecode(err) => err.category(),
            DecodeError::PngEncoding(_) => DecodeErrorCategory::OutputEncoding,
            DecodeError::TransformGuard(err) => err.category(),
            DecodeError::OutputBufferOverflow { .. } => DecodeErrorCategory::OutputEncoding,
            DecodeError::Unsupported(_) => DecodeErrorCategory::UnsupportedFeature,
        }
    }
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(err) => write!(f, "I/O error: {err}"),
            DecodeError::AvifDecode(err) => write!(f, "{err}"),
            DecodeError::HeicDecode(err) => write!(f, "{err}"),
            DecodeError::PngEncoding(err) => write!(f, "PNG encode error: {err}"),
            DecodeError::TransformGuard(err) => write!(f, "{err}"),
            DecodeError::OutputBufferOverflow {
                buffer_name,
                element_count,
                element_size_bytes,
            } => write!(
                f,
                "output buffer size overflow for {buffer_name}: {element_count} elements x {element_size_bytes} bytes"
            ),
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
            DecodeError::TransformGuard(_) => None,
            DecodeError::OutputBufferOverflow { .. } => None,
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

/// Decoded YCbCr sample range derived from nclx signalling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YCbCrRange {
    Full,
    Limited,
}

/// Decoded matrix metadata derived from nclx signalling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct YCbCrMatrixCoefficients {
    pub matrix_coefficients: u16,
    pub colour_primaries: u16,
}

impl Default for YCbCrMatrixCoefficients {
    fn default() -> Self {
        // Provenance: matches libheif undefined-profile defaults from
        // libheif/libheif/nclx.cc:nclx_profile::set_undefined.
        Self {
            matrix_coefficients: 2,
            colour_primaries: 2,
        }
    }
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
    pub ycbcr_range: YCbCrRange,
    pub ycbcr_matrix: YCbCrMatrixCoefficients,
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
    pub ycbcr_range: YCbCrRange,
    pub ycbcr_matrix: YCbCrMatrixCoefficients,
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
    ParsePrimaryTransforms(isobmff::ParsePrimaryItemTransformPropertiesError),
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
    UnsupportedMatrixCoefficients {
        matrix_coefficients: u16,
    },
}

impl DecodeAvifError {
    /// Return the stable high-level category for this AVIF decode failure.
    pub fn category(&self) -> DecodeErrorCategory {
        match self {
            DecodeAvifError::ParsePrimaryProperties(_)
            | DecodeAvifError::ParsePrimaryTransforms(_)
            | DecodeAvifError::ExtractPrimaryPayload(_) => DecodeErrorCategory::Parse,
            DecodeAvifError::DecoderAllocationFailed { .. }
            | DecodeAvifError::DecoderApi { .. }
            | DecodeAvifError::DecoderNoFrameOutput => DecodeErrorCategory::DecoderBackend,
            DecodeAvifError::UnsupportedBitDepth { .. }
            | DecodeAvifError::UnsupportedPixelLayout { .. }
            | DecodeAvifError::UnsupportedMatrixCoefficients { .. } => {
                DecodeErrorCategory::UnsupportedFeature
            }
            DecodeAvifError::InvalidImageGeometry { .. }
            | DecodeAvifError::MissingPlane { .. }
            | DecodeAvifError::PlaneStrideOverflow { .. }
            | DecodeAvifError::PlaneStrideTooSmall { .. }
            | DecodeAvifError::PlaneSizeOverflow { .. }
            | DecodeAvifError::DecodedGeometryMismatch { .. }
            | DecodeAvifError::PlaneSampleTypeMismatch { .. }
            | DecodeAvifError::PlaneDimensionsMismatch { .. }
            | DecodeAvifError::PlaneSampleCountMismatch { .. } => {
                DecodeErrorCategory::MalformedInput
            }
        }
    }
}

impl Display for DecodeAvifError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeAvifError::ParsePrimaryProperties(err) => write!(f, "{err}"),
            DecodeAvifError::ParsePrimaryTransforms(err) => write!(f, "{err}"),
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
            DecodeAvifError::UnsupportedMatrixCoefficients {
                matrix_coefficients,
            } => write!(
                f,
                "AVIF nclx matrix_coefficients {matrix_coefficients} is not supported for YCbCr->RGB conversion"
            ),
        }
    }
}

impl Error for DecodeAvifError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DecodeAvifError::ParsePrimaryProperties(err) => Some(err),
            DecodeAvifError::ParsePrimaryTransforms(err) => Some(err),
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
    ParsePrimaryTransforms(isobmff::ParsePrimaryItemTransformPropertiesError),
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
    UnsupportedMatrixCoefficients {
        matrix_coefficients: u16,
    },
}

impl DecodeHeicError {
    /// Return the stable high-level category for this HEIC decode failure.
    pub fn category(&self) -> DecodeErrorCategory {
        match self {
            DecodeHeicError::ParsePrimaryProperties(_)
            | DecodeHeicError::ParsePrimaryTransforms(_)
            | DecodeHeicError::ExtractPrimaryPayload(_) => DecodeErrorCategory::Parse,
            DecodeHeicError::BackendDecodeFailed { .. } => DecodeErrorCategory::DecoderBackend,
            DecodeHeicError::UnsupportedMatrixCoefficients { .. } => {
                DecodeErrorCategory::UnsupportedFeature
            }
            DecodeHeicError::InvalidDecodedFrame { .. }
            | DecodeHeicError::InvalidNalLengthSize { .. }
            | DecodeHeicError::TruncatedNalLengthField { .. }
            | DecodeHeicError::TruncatedNalUnit { .. }
            | DecodeHeicError::NalUnitTooLarge { .. }
            | DecodeHeicError::TruncatedLengthPrefixedStreamLength { .. }
            | DecodeHeicError::TruncatedLengthPrefixedStreamNalUnit { .. }
            | DecodeHeicError::MissingSpsNalUnit
            | DecodeHeicError::SpsParseFailed { .. }
            | DecodeHeicError::InvalidSpsGeometry { .. }
            | DecodeHeicError::UnsupportedSpsChromaArrayType { .. }
            | DecodeHeicError::MissingVclNalUnit
            | DecodeHeicError::DecodedGeometryMismatch { .. }
            | DecodeHeicError::DecodedBitDepthMismatch { .. }
            | DecodeHeicError::DecodedLayoutMismatch { .. } => DecodeErrorCategory::MalformedInput,
        }
    }
}

impl Display for DecodeHeicError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeHeicError::ParsePrimaryProperties(err) => write!(f, "{err}"),
            DecodeHeicError::ParsePrimaryTransforms(err) => write!(f, "{err}"),
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
            DecodeHeicError::UnsupportedMatrixCoefficients {
                matrix_coefficients,
            } => write!(
                f,
                "HEIC nclx matrix_coefficients {matrix_coefficients} is not supported for YCbCr->RGB conversion"
            ),
        }
    }
}

impl Error for DecodeHeicError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            DecodeHeicError::ParsePrimaryProperties(err) => Some(err),
            DecodeHeicError::ParsePrimaryTransforms(err) => Some(err),
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
    let mut ycbcr_range = YCbCrRange::Full;
    let mut ycbcr_matrix = YCbCrMatrixCoefficients::default();
    let (elementary_stream, expected_geometry) =
        match isobmff::parse_primary_avif_item_properties(input) {
            Ok(properties) => {
                ycbcr_range = ycbcr_range_from_primary_colr(&properties.colr);
                ycbcr_matrix = ycbcr_matrix_from_primary_colr(&properties.colr);
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

    let mut decoded = decode_av1_bitstream_to_image(&elementary_stream)?;
    decoded.ycbcr_range = ycbcr_range;
    decoded.ycbcr_matrix = ycbcr_matrix;
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
    let (stream, metadata, ycbcr_range, ycbcr_matrix) =
        decode_primary_heic_stream_and_metadata(input)?;
    let mut decoded = decode_hevc_stream_to_image(&stream)?;
    decoded.ycbcr_range = ycbcr_range;
    decoded.ycbcr_matrix = ycbcr_matrix;
    validate_decoded_heic_image_against_metadata(&decoded, &metadata)?;
    Ok(decoded)
}

/// Parse primary HEIC stream metadata from the first SPS NAL in the assembled HEVC stream.
pub fn decode_primary_heic_to_metadata(
    input: &[u8],
) -> Result<DecodedHeicImageMetadata, DecodeHeicError> {
    let (_, metadata, _, _) = decode_primary_heic_stream_and_metadata(input)?;
    Ok(metadata)
}

fn decode_primary_heic_stream_and_metadata(
    input: &[u8],
) -> Result<
    (
        Vec<u8>,
        DecodedHeicImageMetadata,
        YCbCrRange,
        YCbCrMatrixCoefficients,
    ),
    DecodeHeicError,
> {
    let properties = isobmff::parse_primary_heic_item_preflight_properties(input)?;
    let item_data = isobmff::extract_primary_heic_item_data(input)?;
    let ycbcr_range = ycbcr_range_from_primary_colr(&properties.colr);
    let ycbcr_matrix = ycbcr_matrix_from_primary_colr(&properties.colr);
    let stream = assemble_heic_hevc_stream_from_components(&properties.hvcc, &item_data.payload)?;
    let decoded = decode_hevc_stream_metadata_from_sps(&stream)?;
    validate_decoded_heic_geometry_against_ispe(
        &decoded,
        properties.ispe.width,
        properties.ispe.height,
    )?;
    Ok((stream, decoded, ycbcr_range, ycbcr_matrix))
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
        ycbcr_range: YCbCrRange::Full,
        ycbcr_matrix: YCbCrMatrixCoefficients::default(),
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
        let transforms = isobmff::parse_primary_item_transform_properties(&input)
            .map_err(DecodeAvifError::ParsePrimaryTransforms)?;
        let icc_profile = primary_icc_profile_from_avif(&input);
        let decoded = decode_primary_avif_to_image(&input)?;
        write_decoded_avif_to_png(
            &decoded,
            &transforms.transforms,
            icc_profile.as_deref(),
            output_path,
        )?;
        return Ok(());
    }

    if matches!(extension, Some(ext) if ext.eq_ignore_ascii_case("heic") || ext.eq_ignore_ascii_case("heif"))
    {
        let input = std::fs::read(input_path)?;
        let transforms = isobmff::parse_primary_item_transform_properties(&input)
            .map_err(DecodeHeicError::ParsePrimaryTransforms)?;
        let icc_profile = primary_icc_profile_from_heic(&input);
        let decoded = decode_primary_heic_to_image(&input)?;
        write_decoded_heic_to_png(
            &decoded,
            &transforms.transforms,
            icc_profile.as_deref(),
            output_path,
        )?;
        return Ok(());
    }

    Err(DecodeError::Unsupported(format!(
        "Unsupported file extension for input: {}",
        input_path.display()
    )))
}

fn primary_icc_profile_from_avif(input: &[u8]) -> Option<Vec<u8>> {
    // Provenance: primary-item colr extraction follows libheif item-property
    // traversal in libheif/libheif/context.cc, with colr payload parsing from
    // libheif/libheif/nclx.cc:Box_colr::parse.
    isobmff::parse_primary_avif_item_properties(input)
        .ok()
        .and_then(|properties| properties.colr.icc.map(|profile| profile.profile))
}

fn primary_icc_profile_from_heic(input: &[u8]) -> Option<Vec<u8>> {
    // Provenance: primary-item colr extraction follows libheif item-property
    // traversal in libheif/libheif/context.cc, with colr payload parsing from
    // libheif/libheif/nclx.cc:Box_colr::parse.
    isobmff::parse_primary_heic_item_preflight_properties(input)
        .ok()
        .and_then(|properties| properties.colr.icc.map(|profile| profile.profile))
}

fn ycbcr_range_from_primary_colr(colr: &isobmff::PrimaryItemColorProperties) -> YCbCrRange {
    // Provenance: mirrors libheif default/input range selection in
    // libheif/libheif/color-conversion/yuv2rgb.cc:Op_YCbCr_to_RGB::convert_colorspace,
    // where full range is assumed unless an nclx profile explicitly marks
    // limited range.
    match colr.nclx.as_ref().map(|nclx| nclx.full_range_flag) {
        Some(true) | None => YCbCrRange::Full,
        Some(false) => YCbCrRange::Limited,
    }
}

fn ycbcr_matrix_from_primary_colr(
    colr: &isobmff::PrimaryItemColorProperties,
) -> YCbCrMatrixCoefficients {
    // Provenance: default/parsed matrix metadata mirrors libheif nclx handling in
    // libheif/libheif/nclx.cc:{nclx_profile::set_undefined,Box_colr::parse}.
    match colr.nclx.as_ref() {
        Some(nclx) => YCbCrMatrixCoefficients {
            matrix_coefficients: nclx.matrix_coefficients,
            colour_primaries: nclx.colour_primaries,
        },
        None => YCbCrMatrixCoefficients::default(),
    }
}

#[derive(Clone, Copy)]
enum YCbCrToRgbTransform {
    Identity,
    Matrix(YCbCrToRgbCoefficientsFp8),
}

#[derive(Clone, Copy)]
struct YCbCrToRgbCoefficientsFp8 {
    r_cr: i32,
    g_cb: i32,
    g_cr: i32,
    b_cb: i32,
}

#[derive(Clone, Copy)]
struct ColourPrimaries {
    red_x: f64,
    red_y: f64,
    green_x: f64,
    green_y: f64,
    blue_x: f64,
    blue_y: f64,
    white_x: f64,
    white_y: f64,
}

// Provenance: default conversion constants/mapping align with libheif's
// YCbCr->RGB defaults in libheif/libheif/nclx.cc
// (YCbCr_to_RGB_coefficients::defaults).
const DEFAULT_YCBCR_TO_RGB_COEFFICIENTS_FP8: YCbCrToRgbCoefficientsFp8 =
    YCbCrToRgbCoefficientsFp8 {
        r_cr: 359,
        g_cb: -88,
        g_cr: -183,
        b_cb: 454,
    };

fn ycbcr_transform_from_matrix(
    matrix: YCbCrMatrixCoefficients,
) -> Result<YCbCrToRgbTransform, u16> {
    // Provenance: unsupported-matrix behavior follows libheif's RGB-conversion
    // operation selection in libheif/libheif/color-conversion/yuv2rgb.cc:
    // Op_YCbCr_to_RGB::state_after_conversion (matrix 11/14 rejected) and the
    // dedicated matrix-specific paths in convert_colorspace (identity=0, YCgCo=8,
    // ICTCP=16).
    if matrix.matrix_coefficients == 0 {
        return Ok(YCbCrToRgbTransform::Identity);
    }

    if matches!(matrix.matrix_coefficients, 8 | 11 | 14 | 16) {
        return Err(matrix.matrix_coefficients);
    }

    Ok(YCbCrToRgbTransform::Matrix(ycbcr_coefficients_from_matrix(
        matrix.matrix_coefficients,
        matrix.colour_primaries,
    )))
}

fn ycbcr_coefficients_from_matrix(
    matrix_coefficients: u16,
    colour_primaries: u16,
) -> YCbCrToRgbCoefficientsFp8 {
    // Provenance: coefficient derivation mirrors
    // libheif/libheif/nclx.cc:{get_Kr_Kb,get_YCbCr_to_RGB_coefficients}.
    let Some((kr, kb)) = kr_kb_from_matrix(matrix_coefficients, colour_primaries) else {
        return DEFAULT_YCBCR_TO_RGB_COEFFICIENTS_FP8;
    };

    if kr == 0.0 && kb == 0.0 {
        return DEFAULT_YCBCR_TO_RGB_COEFFICIENTS_FP8;
    }

    let denom = kb + kr - 1.0;
    if denom == 0.0 {
        return DEFAULT_YCBCR_TO_RGB_COEFFICIENTS_FP8;
    }

    ycbcr_coefficients_from_kr_kb(kr, kb)
}

fn ycbcr_coefficients_from_kr_kb(kr: f64, kb: f64) -> YCbCrToRgbCoefficientsFp8 {
    let r_cr = 2.0 * (1.0 - kr);
    let g_cb = 2.0 * kb * (1.0 - kb) / (kb + kr - 1.0);
    let g_cr = 2.0 * kr * (1.0 - kr) / (kb + kr - 1.0);
    let b_cb = 2.0 * (1.0 - kb);

    YCbCrToRgbCoefficientsFp8 {
        r_cr: (256.0 * r_cr).round() as i32,
        g_cb: (256.0 * g_cb).round() as i32,
        g_cr: (256.0 * g_cr).round() as i32,
        b_cb: (256.0 * b_cb).round() as i32,
    }
}

fn kr_kb_from_matrix(matrix_coefficients: u16, colour_primaries: u16) -> Option<(f64, f64)> {
    match matrix_coefficients {
        1 => Some((0.2126, 0.0722)),
        4 => Some((0.30, 0.11)),
        5 | 6 => Some((0.299, 0.114)),
        7 => Some((0.212, 0.087)),
        9 | 10 => Some((0.2627, 0.0593)),
        12 | 13 => chromaticity_derived_kr_kb(colour_primaries),
        _ => None,
    }
}

fn chromaticity_derived_kr_kb(colour_primaries: u16) -> Option<(f64, f64)> {
    let p = colour_primaries_from_index(colour_primaries)?;
    let zr = 1.0 - (p.red_x + p.red_y);
    let zg = 1.0 - (p.green_x + p.green_y);
    let zb = 1.0 - (p.blue_x + p.blue_y);
    let zw = 1.0 - (p.white_x + p.white_y);

    let denom = p.white_y
        * (p.red_x * (p.green_y * zb - p.blue_y * zg)
            + p.green_x * (p.blue_y * zr - p.red_y * zb)
            + p.blue_x * (p.red_y * zg - p.green_y * zr));
    if denom == 0.0 {
        return None;
    }

    let kr = (p.red_y
        * (p.white_x * (p.green_y * zb - p.blue_y * zg)
            + p.white_y * (p.blue_x * zg - p.green_x * zb)
            + zw * (p.green_x * p.blue_y - p.blue_x * p.green_y)))
        / denom;
    let kb = (p.blue_y
        * (p.white_x * (p.red_y * zg - p.green_y * zr)
            + p.white_y * (p.green_x * zr - p.red_x * zg)
            + zw * (p.red_x * p.green_y - p.green_x * p.red_y)))
        / denom;
    Some((kr, kb))
}

fn colour_primaries_from_index(primaries_idx: u16) -> Option<ColourPrimaries> {
    // Provenance: primaries table mirrors libheif/libheif/nclx.cc:get_colour_primaries.
    match primaries_idx {
        1 => Some(ColourPrimaries {
            green_x: 0.300,
            green_y: 0.600,
            blue_x: 0.150,
            blue_y: 0.060,
            red_x: 0.640,
            red_y: 0.330,
            white_x: 0.3127,
            white_y: 0.3290,
        }),
        4 => Some(ColourPrimaries {
            green_x: 0.21,
            green_y: 0.71,
            blue_x: 0.14,
            blue_y: 0.08,
            red_x: 0.67,
            red_y: 0.33,
            white_x: 0.310,
            white_y: 0.316,
        }),
        5 => Some(ColourPrimaries {
            green_x: 0.29,
            green_y: 0.60,
            blue_x: 0.15,
            blue_y: 0.06,
            red_x: 0.64,
            red_y: 0.33,
            white_x: 0.3127,
            white_y: 0.3290,
        }),
        6 | 7 => Some(ColourPrimaries {
            green_x: 0.310,
            green_y: 0.595,
            blue_x: 0.155,
            blue_y: 0.070,
            red_x: 0.630,
            red_y: 0.340,
            white_x: 0.3127,
            white_y: 0.3290,
        }),
        8 => Some(ColourPrimaries {
            green_x: 0.243,
            green_y: 0.692,
            blue_x: 0.145,
            blue_y: 0.049,
            red_x: 0.681,
            red_y: 0.319,
            white_x: 0.310,
            white_y: 0.316,
        }),
        9 => Some(ColourPrimaries {
            green_x: 0.170,
            green_y: 0.797,
            blue_x: 0.131,
            blue_y: 0.046,
            red_x: 0.708,
            red_y: 0.292,
            white_x: 0.3127,
            white_y: 0.3290,
        }),
        10 => Some(ColourPrimaries {
            green_x: 0.0,
            green_y: 1.0,
            blue_x: 0.0,
            blue_y: 0.0,
            red_x: 1.0,
            red_y: 0.0,
            white_x: 0.333333,
            white_y: 0.333333,
        }),
        11 => Some(ColourPrimaries {
            green_x: 0.265,
            green_y: 0.690,
            blue_x: 0.150,
            blue_y: 0.060,
            red_x: 0.680,
            red_y: 0.320,
            white_x: 0.314,
            white_y: 0.351,
        }),
        12 => Some(ColourPrimaries {
            green_x: 0.265,
            green_y: 0.690,
            blue_x: 0.150,
            blue_y: 0.060,
            red_x: 0.680,
            red_y: 0.320,
            white_x: 0.3127,
            white_y: 0.3290,
        }),
        22 => Some(ColourPrimaries {
            green_x: 0.295,
            green_y: 0.605,
            blue_x: 0.155,
            blue_y: 0.077,
            red_x: 0.630,
            red_y: 0.340,
            white_x: 0.3127,
            white_y: 0.3290,
        }),
        _ => None,
    }
}

fn write_decoded_avif_to_png(
    decoded: &DecodedAvifImage,
    transforms: &[isobmff::PrimaryItemTransformProperty],
    icc_profile: Option<&[u8]>,
    output_path: &Path,
) -> Result<(), DecodeError> {
    if decoded.bit_depth <= 8 {
        let pixels = convert_avif_to_rgba8(decoded)?;
        let (width, height, transformed) =
            apply_primary_item_transforms_rgba(decoded.width, decoded.height, pixels, transforms)?;
        return write_rgba8_png(width, height, &transformed, icc_profile, output_path);
    }

    let pixels = convert_avif_to_rgba16(decoded)?;
    let (width, height, transformed) =
        apply_primary_item_transforms_rgba(decoded.width, decoded.height, pixels, transforms)?;
    write_rgba16_png(width, height, &transformed, icc_profile, output_path)
}

fn write_decoded_heic_to_png(
    decoded: &DecodedHeicImage,
    transforms: &[isobmff::PrimaryItemTransformProperty],
    icc_profile: Option<&[u8]>,
    output_path: &Path,
) -> Result<(), DecodeError> {
    if decoded.bit_depth_luma <= 8 {
        let pixels = convert_heic_to_rgba8(decoded)?;
        let (width, height, transformed) =
            apply_primary_item_transforms_rgba(decoded.width, decoded.height, pixels, transforms)?;
        return write_rgba8_png(width, height, &transformed, icc_profile, output_path);
    }

    let pixels = convert_heic_to_rgba16(decoded)?;
    let (width, height, transformed) =
        apply_primary_item_transforms_rgba(decoded.width, decoded.height, pixels, transforms)?;
    write_rgba16_png(width, height, &transformed, icc_profile, output_path)
}

fn apply_primary_item_transforms_rgba<T: Copy + Default>(
    width: u32,
    height: u32,
    pixels: Vec<T>,
    transforms: &[isobmff::PrimaryItemTransformProperty],
) -> Result<(u32, u32, Vec<T>), DecodeError> {
    let expected = checked_rgba_sample_count(width, height)?;
    if pixels.len() != expected {
        return Err(DecodeError::TransformGuard(
            TransformGuardError::RgbaSampleCountMismatch {
                stage: "transform input",
                actual: pixels.len(),
                expected,
                width,
                height,
            },
        ));
    }

    let mut current_width = width;
    let mut current_height = height;
    let mut current_pixels = pixels;

    for transform in transforms {
        match transform {
            isobmff::PrimaryItemTransformProperty::CleanAperture(clean_aperture) => {
                let (next_width, next_height, next_pixels) = crop_rgba_by_clean_aperture(
                    current_width,
                    current_height,
                    &current_pixels,
                    *clean_aperture,
                )?;
                current_width = next_width;
                current_height = next_height;
                current_pixels = next_pixels;
            }
            isobmff::PrimaryItemTransformProperty::Rotation(rotation) => {
                let (next_width, next_height, next_pixels) = rotate_rgba_ccw(
                    current_width,
                    current_height,
                    &current_pixels,
                    rotation.rotation_ccw_degrees,
                )?;
                current_width = next_width;
                current_height = next_height;
                current_pixels = next_pixels;
            }
            isobmff::PrimaryItemTransformProperty::Mirror(mirror) => {
                current_pixels = mirror_rgba(
                    current_width,
                    current_height,
                    &current_pixels,
                    mirror.direction,
                )?;
            }
        }
    }

    Ok((current_width, current_height, current_pixels))
}

fn checked_rgba_sample_count(width: u32, height: u32) -> Result<usize, DecodeError> {
    let pixel_count = u64::from(width).checked_mul(u64::from(height)).ok_or({
        DecodeError::TransformGuard(TransformGuardError::PixelCountOverflow { width, height })
    })?;
    let sample_count = pixel_count.checked_mul(4).ok_or({
        DecodeError::TransformGuard(TransformGuardError::SampleCountOverflow { width, height })
    })?;
    usize::try_from(sample_count).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::SampleCountExceedsAddressSpace {
            width,
            height,
        })
    })
}

fn rotate_rgba_ccw<T: Copy + Default>(
    width: u32,
    height: u32,
    pixels: &[T],
    rotation_ccw_degrees: u16,
) -> Result<(u32, u32, Vec<T>), DecodeError> {
    let normalized = rotation_ccw_degrees % 360;
    if normalized == 0 {
        return Ok((width, height, pixels.to_vec()));
    }

    let (dst_width, dst_height) = match normalized {
        90 | 270 => (height, width),
        180 => (width, height),
        _ => {
            return Err(DecodeError::TransformGuard(
                TransformGuardError::UnsupportedRotation {
                    rotation_ccw_degrees,
                },
            ));
        }
    };

    let src_width = usize::try_from(width).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::DimensionTooLargeForPlatform {
            stage: "rotation",
            dimension: "source width",
            value: u64::from(width),
        })
    })?;
    let src_height = usize::try_from(height).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::DimensionTooLargeForPlatform {
            stage: "rotation",
            dimension: "source height",
            value: u64::from(height),
        })
    })?;
    let dst_width_usize = usize::try_from(dst_width).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::DimensionTooLargeForPlatform {
            stage: "rotation",
            dimension: "destination width",
            value: u64::from(dst_width),
        })
    })?;
    let output_len = checked_rgba_sample_count(dst_width, dst_height)?;
    let mut out = vec![T::default(); output_len];

    for y in 0..src_height {
        for x in 0..src_width {
            let (dst_x, dst_y) = match normalized {
                90 => (y, src_width - 1 - x),
                180 => (src_width - 1 - x, src_height - 1 - y),
                270 => (src_height - 1 - y, x),
                _ => unreachable!(),
            };

            let src_index = y
                .checked_mul(src_width)
                .and_then(|row| row.checked_add(x))
                .and_then(|pixel| pixel.checked_mul(4))
                .ok_or({
                    DecodeError::TransformGuard(TransformGuardError::PixelIndexOverflow {
                        stage: "rotation source",
                        x,
                        y,
                        width,
                        height,
                    })
                })?;
            let dst_index = dst_y
                .checked_mul(dst_width_usize)
                .and_then(|row| row.checked_add(dst_x))
                .and_then(|pixel| pixel.checked_mul(4))
                .ok_or({
                    DecodeError::TransformGuard(TransformGuardError::PixelIndexOverflow {
                        stage: "rotation destination",
                        x: dst_x,
                        y: dst_y,
                        width: dst_width,
                        height: dst_height,
                    })
                })?;

            out[dst_index..dst_index + 4].copy_from_slice(&pixels[src_index..src_index + 4]);
        }
    }

    Ok((dst_width, dst_height, out))
}

fn mirror_rgba<T: Copy + Default>(
    width: u32,
    height: u32,
    pixels: &[T],
    direction: isobmff::ImageMirrorDirection,
) -> Result<Vec<T>, DecodeError> {
    let src_width = usize::try_from(width).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::DimensionTooLargeForPlatform {
            stage: "mirror",
            dimension: "source width",
            value: u64::from(width),
        })
    })?;
    let src_height = usize::try_from(height).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::DimensionTooLargeForPlatform {
            stage: "mirror",
            dimension: "source height",
            value: u64::from(height),
        })
    })?;
    let output_len = checked_rgba_sample_count(width, height)?;
    let mut out = vec![T::default(); output_len];

    for y in 0..src_height {
        for x in 0..src_width {
            let (dst_x, dst_y) = match direction {
                isobmff::ImageMirrorDirection::Horizontal => (src_width - 1 - x, y),
                isobmff::ImageMirrorDirection::Vertical => (x, src_height - 1 - y),
            };

            let src_index = y
                .checked_mul(src_width)
                .and_then(|row| row.checked_add(x))
                .and_then(|pixel| pixel.checked_mul(4))
                .ok_or({
                    DecodeError::TransformGuard(TransformGuardError::PixelIndexOverflow {
                        stage: "mirror source",
                        x,
                        y,
                        width,
                        height,
                    })
                })?;
            let dst_index = dst_y
                .checked_mul(src_width)
                .and_then(|row| row.checked_add(dst_x))
                .and_then(|pixel| pixel.checked_mul(4))
                .ok_or({
                    DecodeError::TransformGuard(TransformGuardError::PixelIndexOverflow {
                        stage: "mirror destination",
                        x: dst_x,
                        y: dst_y,
                        width,
                        height,
                    })
                })?;

            out[dst_index..dst_index + 4].copy_from_slice(&pixels[src_index..src_index + 4]);
        }
    }

    Ok(out)
}

fn crop_rgba_by_clean_aperture<T: Copy>(
    width: u32,
    height: u32,
    pixels: &[T],
    clean_aperture: isobmff::ImageCleanApertureProperty,
) -> Result<(u32, u32, Vec<T>), DecodeError> {
    if width == 0 || height == 0 {
        return Err(DecodeError::TransformGuard(
            TransformGuardError::EmptyImageGeometry { width, height },
        ));
    }

    let expected = checked_rgba_sample_count(width, height)?;
    if pixels.len() != expected {
        return Err(DecodeError::TransformGuard(
            TransformGuardError::RgbaSampleCountMismatch {
                stage: "clean-aperture input",
                actual: pixels.len(),
                expected,
                width,
                height,
            },
        ));
    }

    // Provenance: crop rounding/clamp order mirrors libheif's primary decode
    // transform path in libheif/libheif/image-items/image_item.cc:
    // ImageItem::decode_image and Box_clap border math in
    // libheif/libheif/box.cc:{Box_clap::left_rounded,right_rounded,top_rounded,bottom_rounded}.
    let mut left = clap_left_rounded(clean_aperture, width);
    let mut right = clap_right_rounded(clean_aperture, width);
    let mut top = clap_top_rounded(clean_aperture, height);
    let mut bottom = clap_bottom_rounded(clean_aperture, height);

    left = left.max(0);
    top = top.max(0);
    let max_x = i128::from(width) - 1;
    let max_y = i128::from(height) - 1;
    right = right.min(max_x);
    bottom = bottom.min(max_y);

    if left > right || top > bottom {
        return Err(DecodeError::TransformGuard(
            TransformGuardError::InvalidCleanApertureBounds {
                width,
                height,
                left,
                right,
                top,
                bottom,
            },
        ));
    }

    let crop_width_i128 = right - left + 1;
    let crop_height_i128 = bottom - top + 1;
    let crop_width = u32::try_from(crop_width_i128).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::CleanApertureCropDimensionOutOfRange {
            dimension: "width",
            value: crop_width_i128,
        })
    })?;
    let crop_height = u32::try_from(crop_height_i128).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::CleanApertureCropDimensionOutOfRange {
            dimension: "height",
            value: crop_height_i128,
        })
    })?;

    let src_width = usize::try_from(width).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::DimensionTooLargeForPlatform {
            stage: "clean aperture",
            dimension: "source width",
            value: u64::from(width),
        })
    })?;
    let left_usize = usize::try_from(left).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::CleanApertureBoundOutOfRange {
            bound: "left",
            value: left,
        })
    })?;
    let right_usize = usize::try_from(right).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::CleanApertureBoundOutOfRange {
            bound: "right",
            value: right,
        })
    })?;
    let top_usize = usize::try_from(top).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::CleanApertureBoundOutOfRange {
            bound: "top",
            value: top,
        })
    })?;
    let bottom_usize = usize::try_from(bottom).map_err(|_| {
        DecodeError::TransformGuard(TransformGuardError::CleanApertureBoundOutOfRange {
            bound: "bottom",
            value: bottom,
        })
    })?;

    let out_len = checked_rgba_sample_count(crop_width, crop_height)?;
    let mut out = Vec::with_capacity(out_len);
    for y in top_usize..=bottom_usize {
        let row_pixel_start = y
            .checked_mul(src_width)
            .and_then(|row| row.checked_add(left_usize))
            .ok_or({
                DecodeError::TransformGuard(TransformGuardError::CleanApertureRowOffsetOverflow {
                    stage: "source row start",
                    y,
                    width,
                    height,
                })
            })?;
        let row_pixel_end = y
            .checked_mul(src_width)
            .and_then(|row| row.checked_add(right_usize))
            .and_then(|pixel| pixel.checked_add(1))
            .ok_or({
                DecodeError::TransformGuard(TransformGuardError::CleanApertureRowOffsetOverflow {
                    stage: "source row end",
                    y,
                    width,
                    height,
                })
            })?;
        let row_sample_start = row_pixel_start.checked_mul(4).ok_or({
            DecodeError::TransformGuard(TransformGuardError::CleanApertureRowOffsetOverflow {
                stage: "source row sample start",
                y,
                width,
                height,
            })
        })?;
        let row_sample_end = row_pixel_end.checked_mul(4).ok_or({
            DecodeError::TransformGuard(TransformGuardError::CleanApertureRowOffsetOverflow {
                stage: "source row sample end",
                y,
                width,
                height,
            })
        })?;

        out.extend_from_slice(&pixels[row_sample_start..row_sample_end]);
    }

    debug_assert_eq!(out.len(), out_len);
    Ok((crop_width, crop_height, out))
}

#[derive(Clone, Copy)]
struct RationalValue {
    numerator: i128,
    denominator: i128,
}

impl RationalValue {
    fn new(numerator: i128, denominator: i128) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    fn integer(value: i128) -> Self {
        Self::new(value, 1)
    }

    fn add(self, other: Self) -> Self {
        Self::new(
            self.numerator * other.denominator + other.numerator * self.denominator,
            self.denominator * other.denominator,
        )
    }

    fn sub(self, other: Self) -> Self {
        Self::new(
            self.numerator * other.denominator - other.numerator * self.denominator,
            self.denominator * other.denominator,
        )
    }

    fn sub_int(self, value: i128) -> Self {
        Self::new(self.numerator - value * self.denominator, self.denominator)
    }

    fn div_int(self, value: i128) -> Self {
        Self::new(self.numerator, self.denominator * value)
    }

    fn round_down(self) -> i128 {
        self.numerator / self.denominator
    }

    fn round(self) -> i128 {
        (self.numerator + self.denominator / 2) / self.denominator
    }
}

fn clap_left_rounded(
    clean_aperture: isobmff::ImageCleanApertureProperty,
    image_width: u32,
) -> i128 {
    let principal_x = RationalValue::new(
        i128::from(clean_aperture.horizontal_offset_num),
        i128::from(clean_aperture.horizontal_offset_den),
    )
    .add(RationalValue::new(i128::from(image_width) - 1, 2));
    principal_x
        .sub(
            RationalValue::new(
                i128::from(clean_aperture.clean_aperture_width_num),
                i128::from(clean_aperture.clean_aperture_width_den),
            )
            .sub_int(1)
            .div_int(2),
        )
        .round_down()
}

fn clap_right_rounded(
    clean_aperture: isobmff::ImageCleanApertureProperty,
    image_width: u32,
) -> i128 {
    RationalValue::new(
        i128::from(clean_aperture.clean_aperture_width_num),
        i128::from(clean_aperture.clean_aperture_width_den),
    )
    .sub_int(1)
    .add(RationalValue::integer(clap_left_rounded(
        clean_aperture,
        image_width,
    )))
    .round()
}

fn clap_top_rounded(
    clean_aperture: isobmff::ImageCleanApertureProperty,
    image_height: u32,
) -> i128 {
    let principal_y = RationalValue::new(
        i128::from(clean_aperture.vertical_offset_num),
        i128::from(clean_aperture.vertical_offset_den),
    )
    .add(RationalValue::new(i128::from(image_height) - 1, 2));
    principal_y
        .sub(
            RationalValue::new(
                i128::from(clean_aperture.clean_aperture_height_num),
                i128::from(clean_aperture.clean_aperture_height_den),
            )
            .sub_int(1)
            .div_int(2),
        )
        .round()
}

fn clap_bottom_rounded(
    clean_aperture: isobmff::ImageCleanApertureProperty,
    image_height: u32,
) -> i128 {
    RationalValue::new(
        i128::from(clean_aperture.clean_aperture_height_num),
        i128::from(clean_aperture.clean_aperture_height_den),
    )
    .sub_int(1)
    .add(RationalValue::integer(clap_top_rounded(
        clean_aperture,
        image_height,
    )))
    .round()
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
    icc_profile: Option<&[u8]>,
    output_path: &Path,
) -> Result<(), DecodeError> {
    let file = File::create(output_path)?;
    let writer = BufWriter::new(file);

    let encoder = rgba_png_encoder_with_optional_icc_profile(
        writer,
        width,
        height,
        png::BitDepth::Eight,
        icc_profile,
    )?;
    let mut png_writer = encoder.write_header()?;
    png_writer.write_image_data(pixels)?;

    Ok(())
}

fn write_rgba16_png(
    width: u32,
    height: u32,
    pixels: &[u16],
    icc_profile: Option<&[u8]>,
    output_path: &Path,
) -> Result<(), DecodeError> {
    let file = File::create(output_path)?;
    let writer = BufWriter::new(file);

    let encoder = rgba_png_encoder_with_optional_icc_profile(
        writer,
        width,
        height,
        png::BitDepth::Sixteen,
        icc_profile,
    )?;
    let mut png_writer = encoder.write_header()?;

    let byte_len = pixels
        .len()
        .checked_mul(2)
        .ok_or(DecodeError::OutputBufferOverflow {
            buffer_name: "RGBA16 PNG byte buffer",
            element_count: pixels.len(),
            element_size_bytes: 2,
        })?;
    let mut bytes = Vec::with_capacity(byte_len);
    for sample in pixels {
        bytes.extend_from_slice(&sample.to_be_bytes());
    }
    png_writer.write_image_data(&bytes)?;

    Ok(())
}

fn rgba_png_encoder_with_optional_icc_profile<W: std::io::Write>(
    writer: W,
    width: u32,
    height: u32,
    bit_depth: png::BitDepth,
    icc_profile: Option<&[u8]>,
) -> Result<png::Encoder<'static, W>, DecodeError> {
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = bit_depth;
    if let Some(profile) = icc_profile {
        info.icc_profile = Some(Cow::Owned(profile.to_vec()));
    }

    png::Encoder::with_info(writer, info).map_err(DecodeError::PngEncoding)
}

fn convert_avif_to_rgba8(decoded: &DecodedAvifImage) -> Result<Vec<u8>, DecodeAvifError> {
    let ycbcr_transform =
        ycbcr_transform_from_matrix(decoded.ycbcr_matrix).map_err(|matrix_coefficients| {
            DecodeAvifError::UnsupportedMatrixCoefficients {
                matrix_coefficients,
            }
        })?;

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
    let chroma_midpoint = chroma_midpoint(decoded.bit_depth);

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

            let (cb_sample, cr_sample) = match &chroma {
                ChromaPlanesU8::Monochrome => (chroma_midpoint, chroma_midpoint),
                ChromaPlanesU8::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]),
                        i32::from(v_samples[chroma_index]),
                    )
                }
            };

            let (r, g, b) = ycbcr_to_rgb_components(
                y_sample,
                cb_sample,
                cr_sample,
                decoded.bit_depth,
                decoded.ycbcr_range,
                ycbcr_transform,
            );
            out.push(scale_sample_to_u8(r, decoded.bit_depth));
            out.push(scale_sample_to_u8(g, decoded.bit_depth));
            out.push(scale_sample_to_u8(b, decoded.bit_depth));
            out.push(u8::MAX);
        }
    }

    Ok(out)
}

fn convert_avif_to_rgba16(decoded: &DecodedAvifImage) -> Result<Vec<u16>, DecodeAvifError> {
    let ycbcr_transform =
        ycbcr_transform_from_matrix(decoded.ycbcr_matrix).map_err(|matrix_coefficients| {
            DecodeAvifError::UnsupportedMatrixCoefficients {
                matrix_coefficients,
            }
        })?;

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
    let chroma_midpoint = chroma_midpoint(decoded.bit_depth);

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

            let (cb_sample, cr_sample) = match &chroma {
                ChromaPlanesU16::Monochrome => (chroma_midpoint, chroma_midpoint),
                ChromaPlanesU16::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]),
                        i32::from(v_samples[chroma_index]),
                    )
                }
            };

            let (r, g, b) = ycbcr_to_rgb_components(
                y_sample,
                cb_sample,
                cr_sample,
                decoded.bit_depth,
                decoded.ycbcr_range,
                ycbcr_transform,
            );
            out.push(scale_sample_to_u16(r, decoded.bit_depth));
            out.push(scale_sample_to_u16(g, decoded.bit_depth));
            out.push(scale_sample_to_u16(b, decoded.bit_depth));
            out.push(u16::MAX);
        }
    }

    Ok(out)
}

fn convert_heic_to_rgba8(decoded: &DecodedHeicImage) -> Result<Vec<u8>, DecodeHeicError> {
    let ycbcr_transform =
        ycbcr_transform_from_matrix(decoded.ycbcr_matrix).map_err(|matrix_coefficients| {
            DecodeHeicError::UnsupportedMatrixCoefficients {
                matrix_coefficients,
            }
        })?;

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
    let chroma_midpoint = chroma_midpoint(bit_depth);

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

            let (cb_sample, cr_sample) = match &chroma {
                HeicChromaPlanes::Monochrome => (chroma_midpoint, chroma_midpoint),
                HeicChromaPlanes::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = heic_chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]),
                        i32::from(v_samples[chroma_index]),
                    )
                }
            };

            let (r, g, b) = ycbcr_to_rgb_components(
                y_sample,
                cb_sample,
                cr_sample,
                bit_depth,
                decoded.ycbcr_range,
                ycbcr_transform,
            );
            out.push(scale_sample_to_u8(r, bit_depth));
            out.push(scale_sample_to_u8(g, bit_depth));
            out.push(scale_sample_to_u8(b, bit_depth));
            out.push(u8::MAX);
        }
    }

    Ok(out)
}

fn convert_heic_to_rgba16(decoded: &DecodedHeicImage) -> Result<Vec<u16>, DecodeHeicError> {
    let ycbcr_transform =
        ycbcr_transform_from_matrix(decoded.ycbcr_matrix).map_err(|matrix_coefficients| {
            DecodeHeicError::UnsupportedMatrixCoefficients {
                matrix_coefficients,
            }
        })?;

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
    let chroma_midpoint = chroma_midpoint(bit_depth);

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

            let (cb_sample, cr_sample) = match &chroma {
                HeicChromaPlanes::Monochrome => (chroma_midpoint, chroma_midpoint),
                HeicChromaPlanes::Color {
                    u_samples,
                    v_samples,
                    chroma_width,
                    layout,
                } => {
                    let chroma_index = heic_chroma_sample_index(x, y, *chroma_width, *layout);
                    (
                        i32::from(u_samples[chroma_index]),
                        i32::from(v_samples[chroma_index]),
                    )
                }
            };

            let (r, g, b) = ycbcr_to_rgb_components(
                y_sample,
                cb_sample,
                cr_sample,
                bit_depth,
                decoded.ycbcr_range,
                ycbcr_transform,
            );
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
    y_sample: i32,
    cb_sample: i32,
    cr_sample: i32,
    bit_depth: u8,
    range: YCbCrRange,
    transform: YCbCrToRgbTransform,
) -> (u16, u16, u16) {
    match transform {
        YCbCrToRgbTransform::Identity => {
            let (r, g, b) =
                normalize_nclx_identity_samples(y_sample, cb_sample, cr_sample, bit_depth, range);
            (
                clip_to_bit_depth(i64::from(r), bit_depth),
                clip_to_bit_depth(i64::from(g), bit_depth),
                clip_to_bit_depth(i64::from(b), bit_depth),
            )
        }
        YCbCrToRgbTransform::Matrix(coeffs) => {
            let (y, cb_centered, cr_centered) =
                normalize_nclx_ycbcr_samples(y_sample, cb_sample, cr_sample, bit_depth, range);
            let r = i64::from(y) + ((i64::from(coeffs.r_cr) * i64::from(cr_centered) + 128) >> 8);
            let g = i64::from(y)
                + ((i64::from(coeffs.g_cb) * i64::from(cb_centered)
                    + i64::from(coeffs.g_cr) * i64::from(cr_centered)
                    + 128)
                    >> 8);
            let b = i64::from(y) + ((i64::from(coeffs.b_cb) * i64::from(cb_centered) + 128) >> 8);

            (
                clip_to_bit_depth(r, bit_depth),
                clip_to_bit_depth(g, bit_depth),
                clip_to_bit_depth(b, bit_depth),
            )
        }
    }
}

fn normalize_nclx_identity_samples(
    y_sample: i32,
    cb_sample: i32,
    cr_sample: i32,
    bit_depth: u8,
    range: YCbCrRange,
) -> (i32, i32, i32) {
    // Provenance: matrix_coefficients=0 handling mirrors
    // libheif/libheif/color-conversion/yuv2rgb.cc:Op_YCbCr_to_RGB::convert_colorspace.
    if range == YCbCrRange::Full {
        return (cr_sample, y_sample, cb_sample);
    }

    let limited_offset = limited_range_offset(bit_depth);
    let r = div_round_nearest(i64::from(cr_sample - limited_offset) * 256, 224) as i32;
    let g = div_round_nearest(i64::from(y_sample - limited_offset) * 256, 219) as i32;
    let b = div_round_nearest(i64::from(cb_sample - limited_offset) * 256, 224) as i32;
    (r, g, b)
}

fn normalize_nclx_ycbcr_samples(
    y_sample: i32,
    cb_sample: i32,
    cr_sample: i32,
    bit_depth: u8,
    range: YCbCrRange,
) -> (i32, i32, i32) {
    let chroma_midpoint = chroma_midpoint(bit_depth);
    if range == YCbCrRange::Full {
        return (
            y_sample,
            cb_sample - chroma_midpoint,
            cr_sample - chroma_midpoint,
        );
    }

    // Provenance: limited-range normalization mirrors the pre-matrix scaling in
    // libheif/libheif/color-conversion/yuv2rgb.cc:Op_YCbCr_to_RGB::convert_colorspace:
    // y'=(y-offset)*(256/219) and cb/cr centered terms scaled by (256/224).
    let limited_offset = limited_range_offset(bit_depth);
    let y_scaled = div_round_nearest(i64::from(y_sample - limited_offset) * 256, 219) as i32;
    let cb_scaled = div_round_nearest(i64::from(cb_sample - chroma_midpoint) * 256, 224) as i32;
    let cr_scaled = div_round_nearest(i64::from(cr_sample - chroma_midpoint) * 256, 224) as i32;
    (y_scaled, cb_scaled, cr_scaled)
}

fn div_round_nearest(value: i64, divisor: i64) -> i64 {
    debug_assert!(divisor > 0);
    if value >= 0 {
        (value + (divisor / 2)) / divisor
    } else {
        (value - (divisor / 2)) / divisor
    }
}

fn limited_range_offset(bit_depth: u8) -> i32 {
    if bit_depth == 0 {
        return 0;
    }
    if bit_depth >= 8 {
        16_i32 << u32::from(bit_depth - 8)
    } else {
        16_i32 >> u32::from(8 - bit_depth)
    }
}

fn chroma_midpoint(bit_depth: u8) -> i32 {
    1_i32 << u32::from(bit_depth.saturating_sub(1))
}

fn clip_to_bit_depth(value: i64, bit_depth: u8) -> u16 {
    let max_value = ((1_i64 << bit_depth) - 1).max(0);
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
        ycbcr_range: YCbCrRange::Full,
        ycbcr_matrix: YCbCrMatrixCoefficients::default(),
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
        append_normalized_hevc_payload_nals, apply_primary_item_transforms_rgba,
        assemble_primary_heic_hevc_stream, convert_avif_to_rgba8, convert_heic_to_rgba8,
        decode_file_to_png, decode_hevc_stream_metadata_from_sps, decode_hevc_stream_to_image,
        decode_primary_avif_to_image, decode_primary_heic_to_image,
        decode_primary_heic_to_metadata, parse_length_prefixed_hevc_nal_units,
        validate_decoded_heic_image_against_metadata, write_rgba8_png, AvifPixelLayout, AvifPlane,
        AvifPlaneSamples, DecodeAvifError, DecodeError, DecodeErrorCategory, DecodeHeicError,
        DecodedAvifImage, DecodedHeicImage, DecodedHeicImageMetadata, HeicPixelLayout, HeicPlane,
        HevcNalClass, TransformGuardError, YCbCrMatrixCoefficients, YCbCrRange,
    };
    use scuffle_h265::NALUnitType;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::process::Command;
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
    fn writes_rgba8_png_with_embedded_icc_profile() {
        let output = test_output_png_path("rgba8-icc");
        let _guard = TempFileGuard(output.clone());
        let profile = vec![0x01, 0x23, 0x45, 0x67, 0x89];

        write_rgba8_png(1, 1, &[0, 0, 0, 255], Some(&profile), &output)
            .expect("RGBA8 PNG writer should embed provided ICC bytes");

        let decoded_profile =
            png_icc_profile(&output).expect("PNG reader should expose embedded ICC profile");
        assert_eq!(decoded_profile, profile);
    }

    #[test]
    fn rotates_rgba_pixels_by_primary_irot_transform() {
        let pixels = vec![
            1_u8, 0, 0, 255, // A
            2_u8, 0, 0, 255, // B
        ];
        let transforms = vec![crate::isobmff::PrimaryItemTransformProperty::Rotation(
            crate::isobmff::ImageRotationProperty {
                rotation_ccw_degrees: 90,
            },
        )];

        let (width, height, transformed) =
            apply_primary_item_transforms_rgba(2, 1, pixels, &transforms)
                .expect("irot transform should rotate RGBA geometry and pixel order");
        assert_eq!((width, height), (1, 2));
        assert_eq!(
            transformed,
            vec![
                2_u8, 0, 0, 255, // B (top)
                1_u8, 0, 0, 255, // A (bottom)
            ]
        );
    }

    #[test]
    fn applies_primary_imir_and_irot_in_property_order() {
        let pixels = vec![
            1_u8, 0, 0, 255, // A (0,0)
            2_u8, 0, 0, 255, // B (1,0)
            3_u8, 0, 0, 255, // C (0,1)
            4_u8, 0, 0, 255, // D (1,1)
        ];
        let transforms = vec![
            crate::isobmff::PrimaryItemTransformProperty::Mirror(
                crate::isobmff::ImageMirrorProperty {
                    direction: crate::isobmff::ImageMirrorDirection::Horizontal,
                },
            ),
            crate::isobmff::PrimaryItemTransformProperty::Rotation(
                crate::isobmff::ImageRotationProperty {
                    rotation_ccw_degrees: 90,
                },
            ),
        ];

        let (width, height, transformed) =
            apply_primary_item_transforms_rgba(2, 2, pixels, &transforms)
                .expect("imir+irot should be applied in transform-property order");
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            transformed,
            vec![
                1_u8, 0, 0, 255, // A
                3_u8, 0, 0, 255, // C
                2_u8, 0, 0, 255, // B
                4_u8, 0, 0, 255, // D
            ]
        );
    }

    #[test]
    fn applies_primary_clap_crop_transform() {
        let pixels = vec![
            1_u8, 0, 0, 255, // A (0,0)
            2_u8, 0, 0, 255, // B (1,0)
            3_u8, 0, 0, 255, // C (2,0)
            4_u8, 0, 0, 255, // D (0,1)
            5_u8, 0, 0, 255, // E (1,1)
            6_u8, 0, 0, 255, // F (2,1)
        ];
        let transforms = vec![crate::isobmff::PrimaryItemTransformProperty::CleanAperture(
            crate::isobmff::ImageCleanApertureProperty {
                clean_aperture_width_num: 2,
                clean_aperture_width_den: 1,
                clean_aperture_height_num: 1,
                clean_aperture_height_den: 1,
                horizontal_offset_num: 0,
                horizontal_offset_den: 1,
                vertical_offset_num: 0,
                vertical_offset_den: 1,
            },
        )];

        let (width, height, transformed) =
            apply_primary_item_transforms_rgba(3, 2, pixels, &transforms)
                .expect("clap transform should crop RGBA geometry and pixel order");
        assert_eq!((width, height), (2, 1));
        assert_eq!(
            transformed,
            vec![
                4_u8, 0, 0, 255, // D
                5_u8, 0, 0, 255, // E
            ]
        );
    }

    #[test]
    fn rejects_primary_clap_transform_with_empty_crop_after_clamp() {
        let pixels = vec![
            1_u8, 0, 0, 255, // A
            2_u8, 0, 0, 255, // B
            3_u8, 0, 0, 255, // C
            4_u8, 0, 0, 255, // D
        ];
        let transforms = vec![crate::isobmff::PrimaryItemTransformProperty::CleanAperture(
            crate::isobmff::ImageCleanApertureProperty {
                clean_aperture_width_num: 0,
                clean_aperture_width_den: 1,
                clean_aperture_height_num: 1,
                clean_aperture_height_den: 1,
                horizontal_offset_num: 0,
                horizontal_offset_den: 1,
                vertical_offset_num: 0,
                vertical_offset_den: 1,
            },
        )];

        let err = apply_primary_item_transforms_rgba(2, 2, pixels, &transforms)
            .expect_err("empty clap crop should fail deterministically");
        assert_eq!(err.category(), DecodeErrorCategory::MalformedInput);
        match err {
            DecodeError::TransformGuard(TransformGuardError::InvalidCleanApertureBounds {
                width,
                height,
                ..
            }) => {
                assert_eq!(width, 2);
                assert_eq!(height, 2);
            }
            other => panic!("unexpected error kind for invalid clap crop: {other}"),
        }
    }

    #[test]
    fn classifies_avif_decode_errors_into_stable_categories() {
        let backend = DecodeAvifError::DecoderNoFrameOutput;
        assert_eq!(backend.category(), DecodeErrorCategory::DecoderBackend);

        let unsupported = DecodeAvifError::UnsupportedMatrixCoefficients {
            matrix_coefficients: 8,
        };
        assert_eq!(
            unsupported.category(),
            DecodeErrorCategory::UnsupportedFeature
        );

        let malformed = DecodeAvifError::PlaneSizeOverflow {
            plane: "Y",
            width: u32::MAX,
            height: 2,
        };
        assert_eq!(malformed.category(), DecodeErrorCategory::MalformedInput);
    }

    #[test]
    fn classifies_heic_decode_errors_into_stable_categories() {
        let backend = DecodeHeicError::BackendDecodeFailed {
            detail: "decoder failed".to_string(),
        };
        assert_eq!(backend.category(), DecodeErrorCategory::DecoderBackend);

        let unsupported = DecodeHeicError::UnsupportedMatrixCoefficients {
            matrix_coefficients: 8,
        };
        assert_eq!(
            unsupported.category(),
            DecodeErrorCategory::UnsupportedFeature
        );

        let malformed = DecodeHeicError::TruncatedNalUnit {
            offset: 4,
            declared: 12,
            available: 2,
        };
        assert_eq!(malformed.category(), DecodeErrorCategory::MalformedInput);
    }

    #[test]
    fn classifies_top_level_decode_errors_into_stable_categories() {
        let io_error = DecodeError::Io(std::io::Error::other("disk read failed"));
        assert_eq!(io_error.category(), DecodeErrorCategory::Io);

        let nested = DecodeError::HeicDecode(DecodeHeicError::MissingSpsNalUnit);
        assert_eq!(nested.category(), DecodeErrorCategory::MalformedInput);

        let transform = DecodeError::TransformGuard(TransformGuardError::EmptyImageGeometry {
            width: 0,
            height: 16,
        });
        assert_eq!(transform.category(), DecodeErrorCategory::MalformedInput);

        let output = DecodeError::OutputBufferOverflow {
            buffer_name: "RGBA16 PNG byte buffer",
            element_count: usize::MAX,
            element_size_bytes: 2,
        };
        assert_eq!(output.category(), DecodeErrorCategory::OutputEncoding);

        let unsupported = DecodeError::Unsupported("unsupported extension".to_string());
        assert_eq!(
            unsupported.category(),
            DecodeErrorCategory::UnsupportedFeature
        );
    }

    #[test]
    fn rejects_rgba_transform_input_when_sample_count_overflows() {
        let err = apply_primary_item_transforms_rgba::<u8>(u32::MAX, u32::MAX, Vec::new(), &[])
            .expect_err("RGBA sample-count overflow should fail deterministically");
        assert_eq!(err.category(), DecodeErrorCategory::MalformedInput);
        assert!(matches!(
            err,
            DecodeError::TransformGuard(TransformGuardError::SampleCountOverflow {
                width,
                height,
            }) if width == u32::MAX && height == u32::MAX
        ));
    }

    #[test]
    fn converts_monochrome_u8_planes_to_rgba8() {
        let image = DecodedAvifImage {
            width: 2,
            height: 1,
            bit_depth: 8,
            layout: AvifPixelLayout::Yuv400,
            ycbcr_range: YCbCrRange::Full,
            ycbcr_matrix: YCbCrMatrixCoefficients::default(),
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
    fn applies_nclx_limited_range_when_converting_avif_monochrome_to_rgba8() {
        let image = DecodedAvifImage {
            width: 2,
            height: 1,
            bit_depth: 8,
            layout: AvifPixelLayout::Yuv400,
            ycbcr_range: YCbCrRange::Limited,
            ycbcr_matrix: YCbCrMatrixCoefficients::default(),
            y_plane: AvifPlane {
                width: 2,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![16, 235]),
            },
            u_plane: None,
            v_plane: None,
        };

        let rgba = convert_avif_to_rgba8(&image)
            .expect("limited-range YUV400 AVIF should normalize to full-range RGBA");
        assert_eq!(rgba, vec![0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn applies_nclx_limited_range_when_converting_heic_monochrome_to_rgba8() {
        let image = DecodedHeicImage {
            width: 2,
            height: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            layout: HeicPixelLayout::Yuv400,
            ycbcr_range: YCbCrRange::Limited,
            ycbcr_matrix: YCbCrMatrixCoefficients::default(),
            y_plane: HeicPlane {
                width: 2,
                height: 1,
                samples: vec![16, 235],
            },
            u_plane: None,
            v_plane: None,
        };

        let rgba = convert_heic_to_rgba8(&image)
            .expect("limited-range YUV400 HEIC should normalize to full-range RGBA");
        assert_eq!(rgba, vec![0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn applies_nclx_identity_matrix_when_converting_avif_to_rgba8() {
        let image = DecodedAvifImage {
            width: 1,
            height: 1,
            bit_depth: 8,
            layout: AvifPixelLayout::Yuv444,
            ycbcr_range: YCbCrRange::Full,
            ycbcr_matrix: YCbCrMatrixCoefficients {
                matrix_coefficients: 0,
                colour_primaries: 1,
            },
            y_plane: AvifPlane {
                width: 1,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![20]),
            },
            u_plane: Some(AvifPlane {
                width: 1,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![30]),
            }),
            v_plane: Some(AvifPlane {
                width: 1,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![40]),
            }),
        };

        let rgba = convert_avif_to_rgba8(&image).expect("matrix=0 should map channels as GBR->RGB");
        assert_eq!(rgba, vec![40, 20, 30, 255]);
    }

    #[test]
    fn reports_unsupported_nclx_matrix_when_converting_avif_to_rgba8() {
        let image = DecodedAvifImage {
            width: 1,
            height: 1,
            bit_depth: 8,
            layout: AvifPixelLayout::Yuv444,
            ycbcr_range: YCbCrRange::Full,
            ycbcr_matrix: YCbCrMatrixCoefficients {
                matrix_coefficients: 8,
                colour_primaries: 1,
            },
            y_plane: AvifPlane {
                width: 1,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![64]),
            },
            u_plane: Some(AvifPlane {
                width: 1,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![96]),
            }),
            v_plane: Some(AvifPlane {
                width: 1,
                height: 1,
                samples: AvifPlaneSamples::U8(vec![128]),
            }),
        };

        let err = convert_avif_to_rgba8(&image)
            .expect_err("matrix=8 should fail with unsupported-matrix error");
        assert!(matches!(
            err,
            DecodeAvifError::UnsupportedMatrixCoefficients {
                matrix_coefficients: 8,
            }
        ));
    }

    #[test]
    fn reports_unsupported_nclx_matrix_when_converting_heic_to_rgba8() {
        let image = DecodedHeicImage {
            width: 1,
            height: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            layout: HeicPixelLayout::Yuv444,
            ycbcr_range: YCbCrRange::Full,
            ycbcr_matrix: YCbCrMatrixCoefficients {
                matrix_coefficients: 8,
                colour_primaries: 1,
            },
            y_plane: HeicPlane {
                width: 1,
                height: 1,
                samples: vec![64],
            },
            u_plane: Some(HeicPlane {
                width: 1,
                height: 1,
                samples: vec![96],
            }),
            v_plane: Some(HeicPlane {
                width: 1,
                height: 1,
                samples: vec![128],
            }),
        };

        let err = convert_heic_to_rgba8(&image)
            .expect_err("matrix=8 should fail with unsupported-matrix error");
        assert!(matches!(
            err,
            DecodeHeicError::UnsupportedMatrixCoefficients {
                matrix_coefficients: 8,
            }
        ));
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
        assert_eq!(err.category(), DecodeErrorCategory::MalformedInput);
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
    fn matches_libheif_primary_icc_profile_size_for_prof_fixture() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../libheif/fuzzing/data/corpus/clusterfuzz-testcase-minimized-file-fuzzer-5752063708495872.heic",
        );
        let input = std::fs::read(&fixture).expect("ICC fixture should be readable");
        let preflight = crate::isobmff::parse_primary_heic_item_preflight_properties(&input)
            .expect("HEIC preflight should parse ICC fixture");
        let icc = preflight
            .colr
            .icc
            .expect("fixture should provide a primary ICC colr payload");
        assert_eq!(icc.profile_type.as_bytes(), *b"prof");

        // Differential oracle: compare against libheif heif-info --dump-boxes
        // output for the same fixture.
        let heif_info_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.ralph/tools/libheif-build/examples/heif-info");
        if !heif_info_bin.is_file() {
            eprintln!(
                "skipping libheif differential ICC test because {} is missing",
                heif_info_bin.display()
            );
            return;
        }

        let output = Command::new(&heif_info_bin)
            .arg("--dump-boxes")
            .arg(&fixture)
            .output()
            .expect("heif-info should run for ICC differential check");
        assert!(
            output.status.success(),
            "heif-info --dump-boxes failed with status {:?}",
            output.status.code()
        );

        let reported_sizes = profile_sizes_from_libheif_dump(&output.stdout);
        assert!(
            !reported_sizes.is_empty(),
            "libheif dump did not report any ICC profile size entries"
        );
        assert!(
            reported_sizes.iter().all(|size| *size == icc.profile.len()),
            "libheif profile sizes {reported_sizes:?} differ from parsed primary ICC payload size {}",
            icc.profile.len()
        );
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
            ycbcr_range: YCbCrRange::Full,
            ycbcr_matrix: YCbCrMatrixCoefficients::default(),
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
            ycbcr_range: YCbCrRange::Full,
            ycbcr_matrix: YCbCrMatrixCoefficients::default(),
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

    fn png_icc_profile(path: &PathBuf) -> Option<Vec<u8>> {
        let png_data = std::fs::read(path).expect("PNG fixture should be readable");
        let decoder = png::Decoder::new(Cursor::new(png_data));
        let reader = decoder.read_info().expect("PNG info should decode");
        reader
            .info()
            .icc_profile
            .as_ref()
            .map(|profile| profile.to_vec())
    }

    fn profile_sizes_from_libheif_dump(output: &[u8]) -> Vec<usize> {
        String::from_utf8_lossy(output)
            .lines()
            .filter_map(|line| {
                let (_, value) = line.split_once("profile size: ")?;
                value.split_whitespace().next()?.parse::<usize>().ok()
            })
            .collect()
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
