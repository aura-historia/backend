const START_OF_IMAGE: [u8; 2] = [0xFF, 0xD8];
const MARKER_PREFIX: u8 = 0xFF;
const APP2: u8 = 0xE2;
const START_OF_SCAN: u8 = 0xDA;
const END_OF_IMAGE: u8 = 0xD9;
/// APP2 identifier of the CIPA DC-007 Multi-Picture Format index.
const MULTI_PICTURE_IDENTIFIER: &[u8] = b"MPF\0";

/// Returns the JPEG without its Multi-Picture Format index segments, or `None` when it has none
/// or its header segments cannot be walked.
///
/// Workers AI rejects a JPEG that carries an MPF index ("image payload does not match its content
/// type") although its primary image is an ordinary JPEG. Phone cameras write the index to link
/// HDR gain maps or depth maps appended after the primary image. Removing only the index segment
/// keeps every other byte; the appended images stay as trailing data that decoders ignore.
pub(super) fn without_multi_picture_index(jpeg: &[u8]) -> Option<Vec<u8>> {
    if !jpeg.starts_with(&START_OF_IMAGE) {
        return None;
    }
    let mut kept = Vec::with_capacity(jpeg.len());
    kept.extend_from_slice(&START_OF_IMAGE);
    let mut removed = false;
    let mut offset = START_OF_IMAGE.len();
    loop {
        if *jpeg.get(offset)? != MARKER_PREFIX {
            return None;
        }
        match *jpeg.get(offset + 1)? {
            MARKER_PREFIX => {
                kept.push(MARKER_PREFIX);
                offset += 1;
            }
            START_OF_SCAN | END_OF_IMAGE => {
                kept.extend_from_slice(&jpeg[offset..]);
                return removed.then_some(kept);
            }
            marker if has_length(marker) => {
                let length = usize::from(u16::from_be_bytes([
                    *jpeg.get(offset + 2)?,
                    *jpeg.get(offset + 3)?,
                ]));
                let end = offset + 2 + length;
                if length < 2 || end > jpeg.len() {
                    return None;
                }
                let segment = &jpeg[offset..end];
                if marker == APP2 && segment[4..].starts_with(MULTI_PICTURE_IDENTIFIER) {
                    removed = true;
                } else {
                    kept.extend_from_slice(segment);
                }
                offset = end;
            }
            _ => return None,
        }
    }
}

/// Markers without a length (TEM, RSTn, SOI) and the reserved `0x00` cannot precede the scan.
fn has_length(marker: u8) -> bool {
    !matches!(marker, 0x00 | 0x01 | 0xD0..=0xD8)
}

#[cfg(test)]
mod tests {
    use super::without_multi_picture_index;
    use image::{DynamicImage, ImageFormat};
    use std::io::Cursor;

    fn jpeg() -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(2, 2)
            .write_to(&mut output, ImageFormat::Jpeg)
            .unwrap_or_else(|_| panic!("encode test JPEG"));
        output.into_inner()
    }

    fn app2(payload: &[u8]) -> Vec<u8> {
        let length = u16::try_from(payload.len() + 2).unwrap_or_else(|_| panic!("short segment"));
        [&[0xFF, 0xE2][..], &length.to_be_bytes(), payload].concat()
    }

    /// Mirrors a camera JPEG: an ICC profile and an MPF index after SOI, a gain map appended.
    fn camera_jpeg(primary: &[u8], icc: &[u8], index: &[u8], appended: &[u8]) -> Vec<u8> {
        [&primary[..2], icc, index, &primary[2..], appended].concat()
    }

    #[test]
    fn should_remove_only_the_index_when_the_jpeg_carries_a_multi_picture_index() {
        let primary = jpeg();
        let icc = app2(b"ICC_PROFILE\0\x01\x01");
        let index = app2(b"MPF\0MM\0\x2a\0\0\0\x08");
        let appended = jpeg();

        let stripped = without_multi_picture_index(&camera_jpeg(&primary, &icc, &index, &appended))
            .unwrap_or_else(|| panic!("MPF index is removed"));

        assert_eq!(camera_jpeg(&primary, &icc, &[], &appended), stripped);
        assert!(image::load_from_memory_with_format(&stripped, ImageFormat::Jpeg).is_ok());
    }

    #[test]
    fn should_remove_every_index_and_keep_fill_bytes_when_segments_are_padded() {
        let primary = jpeg();
        let index = app2(b"MPF\0");
        let fill = [0xFF];
        let padded = [&fill[..], &index, &fill, &index, &fill].concat();

        let stripped = without_multi_picture_index(&camera_jpeg(&primary, &[], &padded, &[]))
            .unwrap_or_else(|| panic!("MPF indexes are removed"));

        assert_eq!(camera_jpeg(&primary, &[], &[0xFF; 3], &[]), stripped);
        assert!(image::load_from_memory_with_format(&stripped, ImageFormat::Jpeg).is_ok());
    }

    #[test]
    fn should_keep_the_jpeg_when_it_has_no_multi_picture_index() {
        let primary = jpeg();
        let icc = app2(b"ICC_PROFILE\0\x01\x01");
        let short_app2 = app2(b"MP");

        assert_eq!(None, without_multi_picture_index(&primary));
        assert_eq!(
            None,
            without_multi_picture_index(&camera_jpeg(&primary, &icc, &[], &jpeg()))
        );
        assert_eq!(
            None,
            without_multi_picture_index(&camera_jpeg(&primary, &short_app2, &[], &[]))
        );
    }

    #[test]
    fn should_keep_the_jpeg_when_its_header_segments_cannot_be_walked() {
        let index = app2(b"MPF\0");
        let mut overlong = [&[0xFF, 0xD8][..], &index].concat();
        overlong[5] = 0xFF;
        let truncated = [&[0xFF, 0xD8][..], &index[..5]].concat();
        let standalone_marker = [&[0xFF, 0xD8, 0xFF, 0xD0][..], &index].concat();

        for jpeg in [overlong, truncated, standalone_marker, b"\x89PNG".to_vec()] {
            assert_eq!(None, without_multi_picture_index(&jpeg), "{jpeg:02x?}");
        }
    }
}
