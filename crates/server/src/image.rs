//! Recognizes an avatar by its bytes (ADR 0119). The declared type of an upload is not read.

use junto_protocol::account::ImageType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub kind: ImageType,
    pub width: u32,
    pub height: u32,
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn be16(bytes: &[u8], at: usize) -> Option<u32> {
    let pair = bytes.get(at..at + 2)?;
    Some(u32::from(u16::from_be_bytes([pair[0], pair[1]])))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
}

fn le16(bytes: &[u8], at: usize) -> Option<u32> {
    let pair = bytes.get(at..at + 2)?;
    Some(u32::from(u16::from_le_bytes([pair[0], pair[1]])))
}

fn le24(bytes: &[u8], at: usize) -> Option<u32> {
    let triple = bytes.get(at..at + 3)?;
    Some(u32::from_le_bytes([triple[0], triple[1], triple[2], 0]))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

/// A still PNG: the header chunk first, and no animation chunk before the image data.
fn png(bytes: &[u8]) -> Option<Image> {
    if bytes.get(..8)? != PNG_SIGNATURE || be32(bytes, 8)? != 13 || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let (width, height) = (be32(bytes, 16)?, be32(bytes, 20)?);
    let mut at = 8;
    loop {
        let length = usize::try_from(be32(bytes, at)?).ok()?;
        match bytes.get(at + 4..at + 8)? {
            b"IDAT" => break,
            b"acTL" | b"IEND" => return None,
            _ => at = at.checked_add(length)?.checked_add(12)?,
        }
    }
    Some(Image {
        kind: ImageType::Png,
        width,
        height,
    })
}

/// A JPEG: the size is in the first frame header before the scan data.
fn jpeg(bytes: &[u8]) -> Option<Image> {
    if bytes.get(..3)? != [0xff, 0xd8, 0xff] {
        return None;
    }
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xff {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            0xff => at += 1,
            0x01 | 0xd0..=0xd7 => at += 2,
            0xd8..=0xda => return None,
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return Some(Image {
                    kind: ImageType::Jpeg,
                    height: be16(bytes, at + 5)?,
                    width: be16(bytes, at + 7)?,
                });
            }
            _ => {
                let length = usize::try_from(be16(bytes, at + 2)?).ok()?;
                if length < 2 {
                    return None;
                }
                at = at.checked_add(2)?.checked_add(length)?;
            }
        }
    }
}

/// A still WebP in one of its three forms: lossy, lossless, or extended without animation.
fn webp(bytes: &[u8]) -> Option<Image> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    let stated = usize::try_from(le32(bytes, 4)?).ok()?;
    if stated.checked_add(8)? != bytes.len() {
        return None;
    }
    let (width, height) = match bytes.get(12..16)? {
        b"VP8 " => {
            if bytes.get(23..26)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            (le16(bytes, 26)? & 0x3fff, le16(bytes, 28)? & 0x3fff)
        }
        b"VP8L" => {
            if *bytes.get(20)? != 0x2f {
                return None;
            }
            let packed = le32(bytes, 21)?;
            ((packed & 0x3fff) + 1, ((packed >> 14) & 0x3fff) + 1)
        }
        b"VP8X" => {
            const ANIMATION: u8 = 0x02;
            if bytes.get(20)? & ANIMATION != 0 {
                return None;
            }
            (le24(bytes, 24)? + 1, le24(bytes, 27)? + 1)
        }
        _ => return None,
    };
    Some(Image {
        kind: ImageType::Webp,
        width,
        height,
    })
}

/// The format and the size that the bytes state, when they are a still PNG, JPEG or WebP
/// image. The image data itself is not decoded.
pub fn inspect(bytes: &[u8]) -> Option<Image> {
    png(bytes)
        .or_else(|| jpeg(bytes))
        .or_else(|| webp(bytes))
        .filter(|image| image.width > 0 && image.height > 0)
}

/// Images for tests: the smallest files that state a format and a size.
pub mod samples {
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = u32::try_from(data.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes()
            .to_vec();
        chunk.extend_from_slice(kind);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&[0; 4]);
        chunk
    }

    pub fn png(width: u32, height: u32) -> Vec<u8> {
        let mut header = width.to_be_bytes().to_vec();
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut image = super::PNG_SIGNATURE.to_vec();
        image.extend(chunk(b"IHDR", &header));
        image.extend(chunk(b"IDAT", &[0; 16]));
        image.extend(chunk(b"IEND", &[]));
        image
    }

    pub fn animated_png(width: u32, height: u32) -> Vec<u8> {
        let still = png(width, height);
        let mut image = still[..33].to_vec();
        image.extend(chunk(b"acTL", &[0; 8]));
        image.extend_from_slice(&still[33..]);
        image
    }

    pub fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut image = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
        image.extend_from_slice(b"JFIF\0");
        image.extend_from_slice(&[1, 1, 0, 0, 1, 0, 1, 0, 0]);
        image.extend_from_slice(&[0xff, 0xdb, 0x00, 0x04, 0, 0]);
        image.extend_from_slice(&[0xff, 0xc0, 0x00, 0x0b, 8]);
        image.extend_from_slice(&height.to_be_bytes());
        image.extend_from_slice(&width.to_be_bytes());
        image.extend_from_slice(&[1, 1, 0x11, 0]);
        image.extend_from_slice(&[0xff, 0xda, 0x00, 0x02, 0xff, 0xd9]);
        image
    }

    fn riff(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        body.extend_from_slice(kind);
        body.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_le_bytes());
        body.extend_from_slice(data);
        let mut image = b"RIFF".to_vec();
        image.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_le_bytes());
        image.extend(body);
        image
    }

    pub fn webp_lossy(width: u16, height: u16) -> Vec<u8> {
        let mut data = vec![0, 0, 0, 0x9d, 0x01, 0x2a];
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        riff(b"VP8 ", &data)
    }

    pub fn webp_lossless(width: u32, height: u32) -> Vec<u8> {
        let packed = (width - 1) | ((height - 1) << 14);
        let mut data = vec![0x2f];
        data.extend_from_slice(&packed.to_le_bytes());
        riff(b"VP8L", &data)
    }

    pub fn webp_extended(width: u32, height: u32, animated: bool) -> Vec<u8> {
        let mut data = vec![if animated { 0x02 } else { 0 }, 0, 0, 0];
        data.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        data.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        riff(b"VP8X", &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(kind: ImageType, width: u32, height: u32) -> Option<Image> {
        Some(Image {
            kind,
            width,
            height,
        })
    }

    #[test]
    fn the_three_formats_are_recognized_by_their_bytes() {
        assert_eq!(
            inspect(&samples::png(64, 48)),
            image(ImageType::Png, 64, 48)
        );
        assert_eq!(
            inspect(&samples::jpeg(640, 480)),
            image(ImageType::Jpeg, 640, 480)
        );
        assert_eq!(
            inspect(&samples::webp_lossy(320, 200)),
            image(ImageType::Webp, 320, 200)
        );
        assert_eq!(
            inspect(&samples::webp_lossless(1024, 1)),
            image(ImageType::Webp, 1024, 1)
        );
        assert_eq!(
            inspect(&samples::webp_extended(300, 400, false)),
            image(ImageType::Webp, 300, 400)
        );
    }

    #[test]
    fn other_bytes_animations_and_cut_files_are_not_images() {
        let gif = b"GIF89a\x10\x00\x10\x00".to_vec();
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec();
        let html = b"<!doctype html><script>alert(1)</script>".to_vec();
        let mut wrong_length = samples::webp_lossy(8, 8);
        wrong_length.push(0);
        let cases = [
            Vec::new(),
            gif,
            svg,
            html,
            samples::animated_png(8, 8),
            samples::webp_extended(8, 8, true),
            samples::png(0, 8),
            samples::jpeg(0, 8),
            samples::png(8, 8)[..20].to_vec(),
            samples::jpeg(8, 8)[..24].to_vec(),
            samples::webp_lossy(8, 8)[..20].to_vec(),
            wrong_length,
            vec![0xff, 0xd8, 0xff, 0xd9],
            vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x01],
        ];
        for bytes in cases {
            assert_eq!(inspect(&bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn a_segment_length_that_points_outside_the_file_ends_the_search() {
        let mut image = vec![0xff, 0xd8, 0xff, 0xe0, 0xff, 0xff];
        image.extend_from_slice(&[0; 32]);
        assert_eq!(inspect(&image), None);
        let mut png = samples::png(8, 8)[..33].to_vec();
        png.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        png.extend_from_slice(b"tEXt");
        assert_eq!(inspect(&png), None);
    }
}
