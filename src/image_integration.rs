use crate::{
    decode_bufread_to_rgba, decode_bytes_to_rgba, decode_path_to_rgba, decode_read_to_rgba,
    decode_seekable_to_rgba_with_hint, DecodeError, DecodedRgbaImage, DecodedRgbaPixels,
    HeifInputFamily,
};
use image::error::{
    DecodingError, ImageFormatHint, ParameterError, ParameterErrorKind, UnsupportedError,
    UnsupportedErrorKind,
};
use image::hooks;
use image::{ColorType, DynamicImage, ImageBuffer, ImageDecoder, ImageError, ImageResult, Rgba};
use std::error::Error;
use std::ffi::OsString;
use std::fmt::{Display, Formatter};
use std::io::{BufRead, Read, Seek};
use std::path::Path;
use std::sync::Once;

const HOOK_EXTENSION_HEIC: &str = "heic";
const HOOK_EXTENSION_HEIF: &str = "heif";
const HOOK_EXTENSION_AVIF: &str = "avif";

const FTYP_MASK_12: [u8; 12] = [
    0x00, 0x00, 0x00, 0x00, // size field is ignored
    0xFF, 0xFF, 0xFF, 0xFF, // "ftyp"
    0xFF, 0xFF, 0xFF, 0xFF, // major_brand
];

const FTYP_SIG_AVIF: [u8; 12] = *b"\0\0\0\0ftypavif";
const FTYP_SIG_AVIS: [u8; 12] = *b"\0\0\0\0ftypavis";

const FTYP_SIG_HEIC: [u8; 12] = *b"\0\0\0\0ftypheic";
const FTYP_SIG_HEIX: [u8; 12] = *b"\0\0\0\0ftypheix";
const FTYP_SIG_HEVC: [u8; 12] = *b"\0\0\0\0ftyphevc";
const FTYP_SIG_HEVX: [u8; 12] = *b"\0\0\0\0ftyphevx";
const FTYP_SIG_HEIM: [u8; 12] = *b"\0\0\0\0ftypheim";
const FTYP_SIG_HEIS: [u8; 12] = *b"\0\0\0\0ftypheis";
const FTYP_SIG_MIF1: [u8; 12] = *b"\0\0\0\0ftypmif1";
const FTYP_SIG_MSF1: [u8; 12] = *b"\0\0\0\0ftypmsf1";
const FTYP_SIG_MIAF: [u8; 12] = *b"\0\0\0\0ftypmiaf";

static REGISTER_IMAGE_FORMAT_DETECTION_HOOKS: Once = Once::new();

pub type Rgba8ImageBuffer = ImageBuffer<Rgba<u8>, Vec<u8>>;
pub type Rgba16ImageBuffer = ImageBuffer<Rgba<u16>, Vec<u16>>;

/// Dedicated `image::ImageDecoder` adapter backed by decoded RGBA samples.
///
/// This adapter decodes HEIF/HEIC/AVIF inputs directly into in-memory RGBA and
/// exposes the buffer via the `image` crate's decoder trait without any PNG
/// intermediate transcode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeifImageDecoder {
    decoded: DecodedRgbaImage,
}

impl HeifImageDecoder {
    /// Build an adapter from an already decoded RGBA image.
    pub fn from_decoded(decoded: DecodedRgbaImage) -> ImageResult<Self> {
        validate_decoded_rgba_image(&decoded)?;
        Ok(Self { decoded })
    }

    /// Decode HEIF/HEIC/AVIF bytes into an `image::ImageDecoder` adapter.
    pub fn from_bytes(input: &[u8]) -> ImageResult<Self> {
        let decoded = decode_bytes_to_rgba(input).map_err(decode_error_to_image_error)?;
        Self::from_decoded(decoded)
    }

    /// Decode a `Read` source into an `image::ImageDecoder` adapter.
    pub fn from_read<R: Read>(input_reader: R) -> ImageResult<Self> {
        let decoded = decode_read_to_rgba(input_reader).map_err(decode_error_to_image_error)?;
        Self::from_decoded(decoded)
    }

    /// Decode a seekable `Read` source into an `image::ImageDecoder` adapter.
    pub fn from_seekable<R: Read + Seek>(input_reader: R) -> ImageResult<Self> {
        Self::from_seekable_with_hint(input_reader, None)
    }

    /// Decode a `BufRead` source into an `image::ImageDecoder` adapter.
    pub fn from_bufread<R: BufRead>(input_reader: R) -> ImageResult<Self> {
        let decoded = decode_bufread_to_rgba(input_reader).map_err(decode_error_to_image_error)?;
        Self::from_decoded(decoded)
    }

    /// Decode a file path into an `image::ImageDecoder` adapter.
    pub fn from_path(input_path: &Path) -> ImageResult<Self> {
        let decoded = decode_path_to_rgba(input_path).map_err(decode_error_to_image_error)?;
        Self::from_decoded(decoded)
    }

    /// Consume the adapter and return the owned decoded RGBA buffer.
    pub fn into_decoded_rgba(self) -> DecodedRgbaImage {
        self.decoded
    }

    fn from_seekable_with_hint<R: Read + Seek>(
        input_reader: R,
        hint: Option<HeifInputFamily>,
    ) -> ImageResult<Self> {
        let decoded = decode_seekable_to_rgba_with_hint(input_reader, hint)
            .map_err(decode_error_to_image_error)?;
        Self::from_decoded(decoded)
    }

    fn storage_color_type(&self) -> ColorType {
        match self.decoded.storage_bit_depth() {
            8 => ColorType::Rgba8,
            16 => ColorType::Rgba16,
            other => {
                unreachable!("validated storage bit depth must be 8 or 16, got {other}")
            }
        }
    }

    fn expected_total_bytes(&self) -> ImageResult<usize> {
        expected_rgba_byte_count(
            self.decoded.width,
            self.decoded.height,
            self.decoded.storage_bit_depth(),
        )
        .ok_or_else(|| {
            parameter_error(format!(
                "decoded RGBA buffer size overflow for {}x{} image",
                self.decoded.width, self.decoded.height
            ))
        })
    }
}

impl ImageDecoder for HeifImageDecoder {
    fn dimensions(&self) -> (u32, u32) {
        (self.decoded.width, self.decoded.height)
    }

    fn color_type(&self) -> ColorType {
        self.storage_color_type()
    }

    fn icc_profile(&mut self) -> ImageResult<Option<Vec<u8>>> {
        Ok(self.decoded.icc_profile.clone())
    }

    fn read_image(self, buf: &mut [u8]) -> ImageResult<()>
    where
        Self: Sized,
    {
        let expected_total_bytes = self.expected_total_bytes()?;
        if buf.len() != expected_total_bytes {
            return Err(ImageError::Parameter(ParameterError::from_kind(
                ParameterErrorKind::DimensionMismatch,
            )));
        }

        match self.decoded.pixels {
            DecodedRgbaPixels::U8(pixels) => {
                buf.copy_from_slice(&pixels);
            }
            DecodedRgbaPixels::U16(pixels) => {
                write_rgba16_native_endian_bytes(&pixels, buf);
            }
        }

        Ok(())
    }

    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> ImageResult<()> {
        (*self).read_image(buf)
    }
}

/// Result of attempting to install `image` crate decoder hooks for this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageHookRegistration {
    pub heic_decoder_hook_registered: bool,
    pub heif_decoder_hook_registered: bool,
    pub avif_decoder_hook_registered: bool,
}

impl ImageHookRegistration {
    pub fn any_decoder_hook_registered(self) -> bool {
        self.heic_decoder_hook_registered
            || self.heif_decoder_hook_registered
            || self.avif_decoder_hook_registered
    }

    pub fn all_decoder_hooks_registered(self) -> bool {
        self.heic_decoder_hook_registered
            && self.heif_decoder_hook_registered
            && self.avif_decoder_hook_registered
    }
}

/// Register HEIF/HEIC/AVIF decoder hooks with `image::hooks`.
///
/// After registration, `image::ImageReader` can decode `.heic`, `.heif`, and
/// `.avif` inputs through this crate's pure-Rust decode path, including direct
/// extension-based dispatch and content-based `ftyp` guesses for common brands.
pub fn register_image_decoder_hooks() -> ImageHookRegistration {
    let heic_decoder_hook_registered = hooks::register_decoding_hook(
        OsString::from(HOOK_EXTENSION_HEIC),
        Box::new(|reader| {
            let decoder =
                HeifImageDecoder::from_seekable_with_hint(reader, Some(HeifInputFamily::Heif))?;
            Ok(Box::new(decoder))
        }),
    );
    let heif_decoder_hook_registered = hooks::register_decoding_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        Box::new(|reader| {
            let decoder =
                HeifImageDecoder::from_seekable_with_hint(reader, Some(HeifInputFamily::Heif))?;
            Ok(Box::new(decoder))
        }),
    );
    let avif_decoder_hook_registered = hooks::register_decoding_hook(
        OsString::from(HOOK_EXTENSION_AVIF),
        Box::new(|reader| {
            let decoder =
                HeifImageDecoder::from_seekable_with_hint(reader, Some(HeifInputFamily::Avif))?;
            Ok(Box::new(decoder))
        }),
    );

    REGISTER_IMAGE_FORMAT_DETECTION_HOOKS.call_once(register_image_format_detection_hooks);

    ImageHookRegistration {
        heic_decoder_hook_registered,
        heif_decoder_hook_registered,
        avif_decoder_hook_registered,
    }
}

fn register_image_format_detection_hooks() {
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_AVIF),
        &FTYP_SIG_AVIF,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_AVIF),
        &FTYP_SIG_AVIS,
        Some(&FTYP_MASK_12),
    );

    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_HEIC,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_HEIX,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_HEVC,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_HEVX,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_HEIM,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_HEIS,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_MIF1,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_MSF1,
        Some(&FTYP_MASK_12),
    );
    hooks::register_format_detection_hook(
        OsString::from(HOOK_EXTENSION_HEIF),
        &FTYP_SIG_MIAF,
        Some(&FTYP_MASK_12),
    );
}

/// ImageBuffer variants produced by `DecodedRgbaImage` conversion helpers.
#[derive(Debug)]
pub enum ImageBufferKind {
    Rgba8(Rgba8ImageBuffer),
    Rgba16(Rgba16ImageBuffer),
}

/// `image::ImageBuffer` conversion output plus metadata that cannot be stored
/// directly inside `ImageBuffer`.
#[derive(Debug)]
pub struct ImageBufferWithMetadata {
    pub image: ImageBufferKind,
    pub source_bit_depth: u8,
    pub icc_profile: Option<Vec<u8>>,
}

impl ImageBufferWithMetadata {
    pub fn storage_bit_depth(&self) -> u8 {
        match self.image {
            ImageBufferKind::Rgba8(_) => 8,
            ImageBufferKind::Rgba16(_) => 16,
        }
    }

    pub fn into_dynamic_image_with_metadata(self) -> DynamicImageWithMetadata {
        let image = match self.image {
            ImageBufferKind::Rgba8(buffer) => DynamicImage::ImageRgba8(buffer),
            ImageBufferKind::Rgba16(buffer) => DynamicImage::ImageRgba16(buffer),
        };
        DynamicImageWithMetadata {
            image,
            source_bit_depth: self.source_bit_depth,
            icc_profile: self.icc_profile,
        }
    }
}

/// `image::DynamicImage` conversion output plus metadata that cannot be stored
/// directly inside `DynamicImage`.
#[derive(Debug)]
pub struct DynamicImageWithMetadata {
    pub image: DynamicImage,
    pub source_bit_depth: u8,
    pub icc_profile: Option<Vec<u8>>,
}

/// Conversion failures while handing off decoded RGBA buffers to the `image` crate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageConversionError {
    SampleCountOverflow {
        width: u32,
        height: u32,
    },
    SampleCountMismatch {
        storage_bit_depth: u8,
        width: u32,
        height: u32,
        expected_samples: usize,
        actual_samples: usize,
    },
}

impl Display for ImageConversionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageConversionError::SampleCountOverflow { width, height } => {
                write!(f, "image sample count overflow for dimensions {width}x{height}")
            }
            ImageConversionError::SampleCountMismatch {
                storage_bit_depth,
                width,
                height,
                expected_samples,
                actual_samples,
            } => write!(
                f,
                "decoded RGBA{storage_bit_depth} sample count mismatch for {width}x{height}: expected {expected_samples}, got {actual_samples}"
            ),
        }
    }
}

impl Error for ImageConversionError {}

impl DecodedRgbaImage {
    /// Convert decoded pixels into `image::ImageBuffer` while carrying metadata.
    pub fn into_image_buffer_with_metadata(
        self,
    ) -> Result<ImageBufferWithMetadata, ImageConversionError> {
        let expected_samples =
            expected_rgba_sample_count(self.width, self.height).ok_or_else(|| {
                ImageConversionError::SampleCountOverflow {
                    width: self.width,
                    height: self.height,
                }
            })?;

        let source_bit_depth = self.source_bit_depth;
        let icc_profile = self.icc_profile;

        let image = match self.pixels {
            DecodedRgbaPixels::U8(pixels) => {
                let actual_samples = pixels.len();
                let buffer =
                    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(self.width, self.height, pixels)
                        .ok_or_else(|| ImageConversionError::SampleCountMismatch {
                            storage_bit_depth: 8,
                            width: self.width,
                            height: self.height,
                            expected_samples,
                            actual_samples,
                        })?;
                ImageBufferKind::Rgba8(buffer)
            }
            DecodedRgbaPixels::U16(pixels) => {
                let actual_samples = pixels.len();
                let buffer =
                    ImageBuffer::<Rgba<u16>, Vec<u16>>::from_raw(self.width, self.height, pixels)
                        .ok_or_else(|| ImageConversionError::SampleCountMismatch {
                        storage_bit_depth: 16,
                        width: self.width,
                        height: self.height,
                        expected_samples,
                        actual_samples,
                    })?;
                ImageBufferKind::Rgba16(buffer)
            }
        };

        Ok(ImageBufferWithMetadata {
            image,
            source_bit_depth,
            icc_profile,
        })
    }

    /// Convert decoded pixels into `image::ImageBuffer`.
    pub fn into_image_buffer(self) -> Result<ImageBufferKind, ImageConversionError> {
        Ok(self.into_image_buffer_with_metadata()?.image)
    }

    /// Convert decoded pixels into `image::DynamicImage` while carrying metadata.
    pub fn into_dynamic_image_with_metadata(
        self,
    ) -> Result<DynamicImageWithMetadata, ImageConversionError> {
        Ok(self
            .into_image_buffer_with_metadata()?
            .into_dynamic_image_with_metadata())
    }

    /// Convert decoded pixels into `image::DynamicImage`.
    pub fn into_dynamic_image(self) -> Result<DynamicImage, ImageConversionError> {
        Ok(self.into_dynamic_image_with_metadata()?.image)
    }
}

fn expected_rgba_sample_count(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}

fn expected_rgba_byte_count(width: u32, height: u32, storage_bit_depth: u8) -> Option<usize> {
    let bytes_per_sample = match storage_bit_depth {
        8 => 1,
        16 => 2,
        _ => return None,
    };
    expected_rgba_sample_count(width, height)?.checked_mul(bytes_per_sample)
}

fn validate_decoded_rgba_image(decoded: &DecodedRgbaImage) -> ImageResult<()> {
    if decoded.storage_bit_depth() != 8 && decoded.storage_bit_depth() != 16 {
        return Err(ImageError::Unsupported(
            UnsupportedError::from_format_and_kind(
                heif_image_format_hint(),
                UnsupportedErrorKind::GenericFeature(format!(
                    "unsupported decoded RGBA storage bit depth {}",
                    decoded.storage_bit_depth()
                )),
            ),
        ));
    }

    let expected_samples =
        expected_rgba_sample_count(decoded.width, decoded.height).ok_or_else(|| {
            parameter_error(format!(
                "decoded RGBA sample count overflow for {}x{} image",
                decoded.width, decoded.height
            ))
        })?;
    let actual_samples = match &decoded.pixels {
        DecodedRgbaPixels::U8(pixels) => pixels.len(),
        DecodedRgbaPixels::U16(pixels) => pixels.len(),
    };
    if actual_samples != expected_samples {
        return Err(parameter_error(format!(
            "decoded RGBA sample count mismatch for {}x{} image: expected {expected_samples}, got {actual_samples}",
            decoded.width, decoded.height
        )));
    }

    Ok(())
}

fn write_rgba16_native_endian_bytes(samples: &[u16], out: &mut [u8]) {
    for (sample, chunk) in samples.iter().zip(out.chunks_exact_mut(2)) {
        chunk.copy_from_slice(&sample.to_ne_bytes());
    }
}

fn heif_image_format_hint() -> ImageFormatHint {
    ImageFormatHint::Name("heif/heic/avif".to_string())
}

fn parameter_error(message: String) -> ImageError {
    ImageError::Parameter(ParameterError::from_kind(ParameterErrorKind::Generic(
        message,
    )))
}

fn decode_error_to_image_error(err: DecodeError) -> ImageError {
    match err {
        DecodeError::Io(io_err) => ImageError::IoError(io_err),
        DecodeError::Unsupported(message) => {
            ImageError::Unsupported(UnsupportedError::from_format_and_kind(
                heif_image_format_hint(),
                UnsupportedErrorKind::GenericFeature(message),
            ))
        }
        other => ImageError::Decoding(DecodingError::new(heif_image_format_hint(), other)),
    }
}

#[cfg(test)]
mod tests {
    use super::{HeifImageDecoder, ImageBufferKind, ImageConversionError, ImageHookRegistration};
    use crate::{DecodedRgbaImage, DecodedRgbaPixels};
    use image::error::ParameterErrorKind;
    use image::{DynamicImage, ImageDecoder, ImageError};

    #[test]
    fn converts_rgba8_to_dynamic_image_and_preserves_metadata() {
        let decoded = DecodedRgbaImage {
            width: 1,
            height: 1,
            source_bit_depth: 10,
            pixels: DecodedRgbaPixels::U8(vec![1, 2, 3, 4]),
            icc_profile: Some(vec![0x49, 0x43, 0x43]),
        };

        let converted = decoded
            .into_dynamic_image_with_metadata()
            .expect("RGBA8 conversion should succeed");
        match converted.image {
            DynamicImage::ImageRgba8(image) => assert_eq!(image.into_raw(), vec![1, 2, 3, 4]),
            _ => panic!("expected DynamicImage::ImageRgba8"),
        }
        assert_eq!(converted.source_bit_depth, 10);
        assert_eq!(converted.icc_profile, Some(vec![0x49, 0x43, 0x43]));
    }

    #[test]
    fn converts_rgba16_to_image_buffer_and_preserves_alpha_channel_layout() {
        let decoded = DecodedRgbaImage {
            width: 1,
            height: 1,
            source_bit_depth: 12,
            pixels: DecodedRgbaPixels::U16(vec![1000, 2000, 3000, 4000]),
            icc_profile: None,
        };

        let converted = decoded
            .into_image_buffer()
            .expect("RGBA16 conversion should succeed");
        match converted {
            ImageBufferKind::Rgba16(image) => {
                assert_eq!(image.into_raw(), vec![1000, 2000, 3000, 4000])
            }
            _ => panic!("expected ImageBufferKind::Rgba16"),
        }
    }

    #[test]
    fn reports_rgba_sample_mismatch_for_invalid_u8_buffer_length() {
        let decoded = DecodedRgbaImage {
            width: 2,
            height: 1,
            source_bit_depth: 8,
            pixels: DecodedRgbaPixels::U8(vec![1, 2, 3, 4, 5, 6, 7]),
            icc_profile: None,
        };

        let err = decoded
            .into_image_buffer()
            .expect_err("invalid RGBA8 sample length should fail conversion");
        assert_eq!(
            err,
            ImageConversionError::SampleCountMismatch {
                storage_bit_depth: 8,
                width: 2,
                height: 1,
                expected_samples: 8,
                actual_samples: 7,
            }
        );
    }

    #[test]
    fn image_decoder_reads_rgba8_without_png_intermediate() {
        let decoded = DecodedRgbaImage {
            width: 1,
            height: 1,
            source_bit_depth: 10,
            pixels: DecodedRgbaPixels::U8(vec![10, 20, 30, 40]),
            icc_profile: Some(vec![0x49, 0x43, 0x43]),
        };

        let mut decoder = HeifImageDecoder::from_decoded(decoded)
            .expect("decoded RGBA8 image should build ImageDecoder adapter");
        assert_eq!(decoder.dimensions(), (1, 1));
        assert_eq!(decoder.color_type(), image::ColorType::Rgba8);
        assert_eq!(decoder.total_bytes(), 4);
        assert_eq!(
            decoder.icc_profile().expect("icc query should succeed"),
            Some(vec![0x49, 0x43, 0x43])
        );

        let mut out = vec![0_u8; 4];
        decoder
            .read_image(&mut out)
            .expect("ImageDecoder::read_image should copy RGBA8 bytes");
        assert_eq!(out, vec![10, 20, 30, 40]);
    }

    #[test]
    fn image_decoder_reads_rgba16_in_native_endian() {
        let decoded = DecodedRgbaImage {
            width: 1,
            height: 1,
            source_bit_depth: 12,
            pixels: DecodedRgbaPixels::U16(vec![0x0102, 0x0304, 0x0506, 0x0708]),
            icc_profile: None,
        };

        let decoder = HeifImageDecoder::from_decoded(decoded)
            .expect("decoded RGBA16 image should build ImageDecoder adapter");
        assert_eq!(decoder.color_type(), image::ColorType::Rgba16);
        assert_eq!(decoder.total_bytes(), 8);

        let mut out = vec![0_u8; 8];
        decoder
            .read_image(&mut out)
            .expect("ImageDecoder::read_image should copy RGBA16 bytes");
        let expected = [
            0x0102_u16.to_ne_bytes(),
            0x0304_u16.to_ne_bytes(),
            0x0506_u16.to_ne_bytes(),
            0x0708_u16.to_ne_bytes(),
        ]
        .concat();
        assert_eq!(out, expected);
    }

    #[test]
    fn image_decoder_rejects_output_buffer_size_mismatch() {
        let decoded = DecodedRgbaImage {
            width: 1,
            height: 1,
            source_bit_depth: 8,
            pixels: DecodedRgbaPixels::U8(vec![1, 2, 3, 4]),
            icc_profile: None,
        };

        let decoder = HeifImageDecoder::from_decoded(decoded)
            .expect("decoded RGBA image should build ImageDecoder adapter");
        let mut out = vec![0_u8; 3];
        let err = decoder
            .read_image(&mut out)
            .expect_err("read_image should reject wrong output buffer length");
        match err {
            ImageError::Parameter(parameter) => {
                assert_eq!(parameter.kind(), ParameterErrorKind::DimensionMismatch);
            }
            other => panic!("expected parameter error, got {other:?}"),
        }
    }

    #[test]
    fn image_hook_registration_helpers_report_status_bits() {
        let status = ImageHookRegistration {
            heic_decoder_hook_registered: true,
            heif_decoder_hook_registered: false,
            avif_decoder_hook_registered: false,
        };
        assert!(status.any_decoder_hook_registered());
        assert!(!status.all_decoder_hooks_registered());
    }
}
