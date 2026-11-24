//! A perceptual hash that tells whether two image files show the same picture. It survives
//! resizing, re-encoding and EXIF rotation, which is how image CDNs and shop systems usually
//! serve one photo under several URLs, but not a different photo.
//!
//! It is a ternary gradient hash: the upright image, composited over white like a re-encoded
//! JPEG, is shrunk to 17 × 16 grey pixels, and each of the 256 horizontal neighbour pairs records
//! whether the left pixel is clearly brighter, clearly darker or about as bright. Fine detail and
//! compression noise average out in the shrink; the layout of light and dark that tells two photos
//! apart does not. The third state keeps flat areas, such as a plain backdrop, from contributing
//! noise: a two-state hash would flip their bits at random and blur the line between a rescaled
//! copy and another object photographed on the same backdrop.

use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, GrayImage};

const GRID: u32 = 16;
/// The size an image is first shrunk to: small enough that rotating it costs nothing, large
/// enough that the second shrink still averages over several pixels.
const THUMBNAIL: u32 = 64;
const CELLS: usize = (GRID * GRID) as usize;
/// Brightness steps, of 255, within which neighbours count as equally bright.
const FLAT: i16 = 2;
/// Hashes at most this far apart show the same picture. Calibrated on 69 product photos from
/// listings, including coins and small objects on plain backdrops: copies rescaled to between 30 %
/// and 150 % and re-encoded as JPEG or WebP stayed within 22 and copies cropped by 1 % on each side
/// within 38, while distinct photos were at least 57 apart.
const MAX_MATCHING_DISTANCE: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageHash([u8; Self::BYTES]);

impl ImageHash {
    /// A bit set of the clearly brighter pairs, then one of the clearly darker pairs.
    pub const BYTES: usize = 2 * CELLS / 8;

    /// A stored hash; `None` unless `bytes` has the hash's length.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    pub fn as_bytes(&self) -> &[u8; Self::BYTES] {
        &self.0
    }

    /// How far the hashes are apart: 2 for each pair whose gradient is reversed, 1 for each pair
    /// that is flat in one hash only.
    pub fn distance(&self, other: &Self) -> u32 {
        self.0
            .iter()
            .zip(other.0)
            .map(|(left, right)| (left ^ right).count_ones())
            .sum()
    }

    /// Whether both hashes show the same picture.
    pub fn matches(&self, other: &Self) -> bool {
        self.distance(other) <= MAX_MATCHING_DISTANCE
    }

    pub(crate) fn of(image: &DynamicImage, orientation: Orientation) -> Self {
        let mut thumbnail = image.resize_exact(THUMBNAIL, THUMBNAIL, FilterType::Triangle);
        thumbnail.apply_orientation(orientation);
        let grey = DynamicImage::ImageRgb8(crate::downscale::opaque_rgb(thumbnail)).to_luma8();
        let grid = image::imageops::resize(&grey, GRID + 1, GRID, FilterType::Triangle);
        Self::of_gradients(&grid)
    }

    fn of_gradients(grid: &GrayImage) -> Self {
        let mut bytes = [0_u8; Self::BYTES];
        for y in 0..GRID {
            for x in 0..GRID {
                let gradient =
                    i16::from(grid.get_pixel(x, y).0[0]) - i16::from(grid.get_pixel(x + 1, y).0[0]);
                let cell = (y * GRID + x) as usize;
                let bit = if gradient > FLAT {
                    cell
                } else if gradient < -FLAT {
                    CELLS + cell
                } else {
                    continue;
                };
                bytes[bit / 8] |= 1 << (bit % 8);
            }
        }
        Self(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downscale::decode;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
    use std::io::Cursor;

    /// A photo-like scene: a lit object with a shadow on a light backdrop, varied by `seed`.
    fn scene(width: u32, height: u32, seed: u32) -> DynamicImage {
        let (cx, cy) = (
            0.3 + 0.4 * f64::from(seed % 5) / 4.0,
            0.35 + 0.3 * f64::from(seed % 3) / 2.0,
        );
        let radius = 0.18 + 0.04 * f64::from(seed % 4);
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            let (u, v) = (
                f64::from(x) / f64::from(width),
                f64::from(y) / f64::from(height),
            );
            let backdrop = 235.0 - 30.0 * v;
            let distance = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
            let shade = if distance < radius {
                60.0 + 120.0 * (u - cx + radius) / (2.0 * radius)
            } else if ((u - cx - 0.05).powi(2) + (v - cy - 0.08).powi(2)).sqrt() < radius {
                backdrop - 50.0
            } else {
                backdrop
            };
            let value = shade.clamp(0.0, 255.0) as u8;
            Rgb([value, value.saturating_sub(10), value.saturating_sub(25)])
        }))
    }

    fn hash(image: &DynamicImage, format: ImageFormat) -> ImageHash {
        let mut file = Cursor::new(Vec::new());
        image.write_to(&mut file, format).unwrap();
        let mime_type = match format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            _ => "image/webp",
        };
        decode(mime_type, file.get_ref()).unwrap().perceptual_hash()
    }

    fn jpeg(image: &DynamicImage, quality: u8) -> ImageHash {
        let mut file = Vec::new();
        JpegEncoder::new_with_quality(&mut file, quality)
            .encode_image(&image.to_rgb8())
            .unwrap();
        decode("image/jpeg", &file).unwrap().perceptual_hash()
    }

    #[test]
    fn should_match_a_rescaled_and_reencoded_copy_of_the_same_picture() {
        let original = scene(1600, 1200, 1);
        let hash_of_original = hash(&original, ImageFormat::Png);

        for copy in [
            jpeg(&original.resize_exact(400, 300, FilterType::Lanczos3), 60),
            jpeg(&original.resize_exact(800, 600, FilterType::Triangle), 90),
            hash(
                &original.resize_exact(1024, 768, FilterType::CatmullRom),
                ImageFormat::WebP,
            ),
        ] {
            assert!(
                hash_of_original.matches(&copy),
                "distance {}",
                hash_of_original.distance(&copy)
            );
        }
    }

    #[test]
    fn should_tell_different_pictures_on_the_same_backdrop_apart() {
        let hashes = (0..6)
            .map(|seed| hash(&scene(800, 600, seed), ImageFormat::Png))
            .collect::<Vec<_>>();

        for (index, left) in hashes.iter().enumerate() {
            for right in &hashes[index + 1..] {
                assert!(!left.matches(right), "distance {}", left.distance(right));
            }
        }
    }

    #[test]
    fn should_match_a_picture_with_its_upright_transparent_free_copy() {
        let picture = scene(300, 200, 2);
        let mut rgba = RgbaImage::from_fn(300, 200, |x, y| {
            let Rgb([red, green, blue]) = *picture.to_rgb8().get_pixel(x, y);
            Rgba([red, green, blue, 255])
        });
        // A transparent margin becomes white, as in the re-encoded JPEG.
        for x in 0..20 {
            for y in 0..200 {
                rgba.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            }
        }
        let transparent = DynamicImage::ImageRgba8(rgba);
        let flattened = DynamicImage::ImageRgb8(crate::downscale::opaque_rgb(transparent.clone()));

        assert!(
            hash(&transparent, ImageFormat::Png).matches(&jpeg(&flattened, 85)),
            "transparency is composited before hashing"
        );
        let rotated = ImageHash::of(&picture.rotate90(), Orientation::Rotate270);
        assert!(
            ImageHash::of(&picture, Orientation::NoTransforms).matches(&rotated),
            "orientation is applied before hashing"
        );
    }

    #[test]
    fn should_round_trip_through_its_bytes_and_reject_other_lengths() {
        let hash = hash(&scene(64, 64, 3), ImageFormat::Png);

        assert_eq!(Some(hash), ImageHash::from_bytes(hash.as_bytes()));
        assert_eq!(None, ImageHash::from_bytes(&[0; 63]));
        assert_eq!(0, hash.distance(&hash));
    }
}
