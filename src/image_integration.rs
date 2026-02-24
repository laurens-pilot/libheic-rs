use crate::{DecodedRgbaImage, DecodedRgbaPixels};
use image::{DynamicImage, ImageBuffer, Rgba};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub type Rgba8ImageBuffer = ImageBuffer<Rgba<u8>, Vec<u8>>;
pub type Rgba16ImageBuffer = ImageBuffer<Rgba<u16>, Vec<u16>>;

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

#[cfg(test)]
mod tests {
    use super::{ImageBufferKind, ImageConversionError};
    use crate::{DecodedRgbaImage, DecodedRgbaPixels};
    use image::DynamicImage;

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
}
