//! Recognizes an avatar (ADR 0119) and a picture of a message (ADR 0408) by its bytes. The
//! declared type of an upload is not read.

use std::io::Read;

use junto_protocol::account::ImageType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub kind: ImageType,
    pub width: u32,
    pub height: u32,
}

/// The format of a picture of a message: the formats of an avatar, and GIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureKind {
    Png,
    Jpeg,
    Webp,
    Gif,
}

impl PictureKind {
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }
}

/// What the bytes of a picture state about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    pub kind: PictureKind,
    /// The size of the picture, or of the canvas of an animation.
    pub width: u32,
    pub height: u32,
    /// The file states an animation: the animation chunk of a PNG file or the animation
    /// flag of a WebP file. A GIF file states none.
    pub animated: bool,
    /// The frames of the picture: 1 for a still picture.
    pub frames: u32,
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Reads the parts of a file in order. A read past the end of the file fails.
struct Source<R> {
    reader: R,
}

impl<R: Read> Source<R> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let mut bytes = [0; N];
        self.reader.read_exact(&mut bytes).ok()?;
        Some(bytes)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }

    /// The next byte, or `Some(None)` at the end of the file.
    fn next(&mut self) -> Option<Option<u8>> {
        let mut byte = [0];
        loop {
            match self.reader.read(&mut byte) {
                Ok(0) => return Some(None),
                Ok(_) => return Some(Some(byte[0])),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }

    /// The next `count` bytes; `count` is bounded by the caller.
    fn read(&mut self, count: u64) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        (&mut self.reader)
            .take(count)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 == count).then_some(bytes)
    }

    fn skip(&mut self, count: u64) -> Option<()> {
        let skipped =
            std::io::copy(&mut (&mut self.reader).take(count), &mut std::io::sink()).ok()?;
        (skipped == count).then_some(())
    }
}

fn be16(bytes: [u8; 2]) -> u32 {
    u32::from(u16::from_be_bytes(bytes))
}

fn le16(bytes: [u8; 2]) -> u32 {
    u32::from(u16::from_le_bytes(bytes))
}

fn le24(bytes: [u8; 3]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

/// The longest EXIF block that is read for its orientation; a longer one is skipped.
const MOST_EXIF: u64 = 65_536;

/// The picture is stored turned by a quarter: its EXIF orientation is 5 to 8, so it is
/// shown with its width and height exchanged (ADR 0408). `tiff` is the TIFF structure of
/// the EXIF block: its byte order, its first directory and the orientation entry there.
fn turned(tiff: &[u8]) -> bool {
    let little = match tiff.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return false,
    };
    let u16_at = |at: usize| {
        let bytes: [u8; 2] = tiff.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let u32_at = |at: usize| {
        let bytes: [u8; 4] = tiff.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };
    let orientation = || -> Option<u16> {
        if u16_at(2)? != 42 {
            return None;
        }
        let directory = usize::try_from(u32_at(4)?).ok()?;
        let entries = usize::from(u16_at(directory)?);
        (0..entries).find_map(|index| {
            let entry = directory
                .checked_add(2)?
                .checked_add(index.checked_mul(12)?)?;
            const ORIENTATION: u16 = 0x0112;
            const SHORT: u16 = 3;
            (u16_at(entry)? == ORIENTATION && u16_at(entry + 2)? == SHORT)
                .then(|| u16_at(entry + 8))
                .flatten()
        })
    };
    matches!(orientation(), Some(5..=8))
}

/// The EXIF block of a JPEG segment or of a WebP chunk, which starts with `Exif` and two
/// zero bytes before its TIFF structure; a WebP writer may leave them out.
fn exif_tiff(block: &[u8]) -> &[u8] {
    block.strip_prefix(b"Exif\0\0").unwrap_or(block)
}

/// A PNG: the header chunk first. An animation chunk before the image data states the
/// frames of an animated PNG.
fn png<R: Read>(file: &mut Source<R>, start: [u8; 4]) -> Option<Picture> {
    let rest: [u8; 4] = file.take()?;
    if [start, rest].concat() != PNG_SIGNATURE {
        return None;
    }
    if file.take::<8>()? != [0, 0, 0, 13, b'I', b'H', b'D', b'R'] {
        return None;
    }
    let width = u32::from_be_bytes(file.take()?);
    let height = u32::from_be_bytes(file.take()?);
    // The rest of the header and its check value.
    file.skip(5 + 4)?;
    let mut frames = None;
    let mut quarter = false;
    loop {
        let length = u64::from(u32::from_be_bytes(file.take()?));
        match &file.take::<4>()? {
            b"IDAT" => break,
            b"IEND" => return None,
            b"acTL" if frames.is_none() && length >= 4 => {
                frames = Some(u32::from_be_bytes(file.take()?));
                // The rest of the chunk and its check value.
                file.skip(length)?;
            }
            b"eXIf" if length <= MOST_EXIF => {
                let block = file.read(length)?;
                quarter = turned(&block);
                file.skip(4)?;
            }
            _ => file.skip(length.checked_add(4)?)?,
        }
    }
    if frames == Some(0) {
        return None;
    }
    let (width, height) = if quarter {
        (height, width)
    } else {
        (width, height)
    };
    Some(Picture {
        kind: PictureKind::Png,
        width,
        height,
        animated: frames.is_some(),
        frames: frames.unwrap_or(1),
    })
}

/// A JPEG: the size is in the first frame header before the scan data.
fn jpeg<R: Read>(file: &mut Source<R>, start: [u8; 4]) -> Option<Picture> {
    if start[..3] != [0xff, 0xd8, 0xff] {
        return None;
    }
    let mut marker = start[3];
    let mut quarter = None;
    loop {
        match marker {
            0xff => {}
            0x01 | 0xd0..=0xd7 => {
                if file.byte()? != 0xff {
                    return None;
                }
            }
            0xd8..=0xda => return None,
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                // The length of the header and the precision of the samples.
                file.skip(3)?;
                let height = be16(file.take()?);
                let width = be16(file.take()?);
                let (width, height) = if quarter == Some(true) {
                    (height, width)
                } else {
                    (width, height)
                };
                return Some(Picture {
                    kind: PictureKind::Jpeg,
                    width,
                    height,
                    animated: false,
                    frames: 1,
                });
            }
            _ => {
                let length = be16(file.take()?);
                if length < 2 {
                    return None;
                }
                let block = u64::from(length) - 2;
                // The first EXIF segment states the orientation.
                if marker == 0xe1 && quarter.is_none() {
                    let data = file.read(block)?;
                    if data.starts_with(b"Exif\0\0") {
                        quarter = Some(turned(exif_tiff(&data)));
                    }
                } else {
                    file.skip(block)?;
                }
                if file.byte()? != 0xff {
                    return None;
                }
            }
        }
        marker = file.byte()?;
    }
}

/// A WebP in one of its three forms: lossy, lossless, or extended. An extended file with
/// the animation flag holds one frame chunk for each frame. The RIFF header states the
/// length of the file, `total`.
fn webp<R: Read>(file: &mut Source<R>, start: [u8; 4], total: u64) -> Option<Picture> {
    if &start != b"RIFF" {
        return None;
    }
    let stated = u64::from(u32::from_le_bytes(file.take()?));
    if &file.take::<4>()? != b"WEBP" || stated.checked_add(8)? != total {
        return None;
    }
    let kind: [u8; 4] = file.take()?;
    let size = u64::from(u32::from_le_bytes(file.take()?));
    let mut frames = 1;
    let mut animated = false;
    let (width, height) = match &kind {
        b"VP8 " => {
            file.skip(3)?;
            if file.take::<3>()? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            (le16(file.take()?) & 0x3fff, le16(file.take()?) & 0x3fff)
        }
        b"VP8L" => {
            if file.byte()? != 0x2f {
                return None;
            }
            let packed = u32::from_le_bytes(file.take()?);
            ((packed & 0x3fff) + 1, ((packed >> 14) & 0x3fff) + 1)
        }
        b"VP8X" => {
            const ANIMATION: u8 = 0x02;
            const EXIF: u8 = 0x08;
            let flags = file.byte()?;
            animated = flags & ANIMATION != 0;
            file.skip(3)?;
            let canvas = (le24(file.take()?) + 1, le24(file.take()?) + 1);
            let mut quarter = false;
            // The frames of an animation and the EXIF chunk follow the image data: each
            // chunk is read to the end of the file.
            if animated || flags & EXIF != 0 {
                frames = if animated { 0u32 } else { 1 };
                // The rest of the header chunk, then each chunk to the end of the file.
                let read = 10;
                let padded = size.checked_add(size & 1)?;
                file.skip(padded.checked_sub(read)?)?;
                let mut at = 20 + padded;
                while at < total {
                    let chunk: [u8; 4] = file.take()?;
                    let length = u64::from(u32::from_le_bytes(file.take()?));
                    let padded = length + (length & 1);
                    if animated && &chunk == b"ANMF" {
                        frames = frames.checked_add(1)?;
                    }
                    if &chunk == b"EXIF" && length <= MOST_EXIF {
                        let block = file.read(length)?;
                        quarter = turned(exif_tiff(&block));
                        file.skip(padded - length)?;
                    } else {
                        file.skip(padded)?;
                    }
                    at = at.checked_add(8 + padded)?;
                }
                if frames == 0 || at != total {
                    return None;
                }
            }
            if quarter {
                (canvas.1, canvas.0)
            } else {
                canvas
            }
        }
        _ => return None,
    };
    Some(Picture {
        kind: PictureKind::Webp,
        width,
        height,
        animated,
        frames,
    })
}

/// Skips the data blocks of a GIF block, which end with a block of length 0.
fn gif_blocks<R: Read>(file: &mut Source<R>) -> Option<()> {
    loop {
        match file.byte()? {
            0 => return Some(()),
            length => file.skip(u64::from(length))?,
        }
    }
}

/// A GIF: the size of the canvas in the screen descriptor, and one image descriptor for
/// each frame. A file that ends after a whole block without its trailer is read up to its
/// end, as browsers show it.
fn gif<R: Read>(file: &mut Source<R>, start: [u8; 4]) -> Option<Picture> {
    let version: [u8; 2] = file.take()?;
    if &start != b"GIF8" || !matches!(&version, b"7a" | b"9a") {
        return None;
    }
    let width = le16(file.take()?);
    let height = le16(file.take()?);
    let [packed, _, _]: [u8; 3] = file.take()?;
    let table = |packed: u8| 3u64 << ((packed & 0x07) + 1);
    if packed & 0x80 != 0 {
        file.skip(table(packed))?;
    }
    let mut frames = 0u32;
    loop {
        match file.next()? {
            None | Some(0x3b) => break,
            Some(0x21) => {
                file.byte()?;
                gif_blocks(file)?;
            }
            Some(0x2c) => {
                let [.., packed]: [u8; 9] = file.take()?;
                if packed & 0x80 != 0 {
                    file.skip(table(packed))?;
                }
                // The code size of the compressed data.
                file.byte()?;
                gif_blocks(file)?;
                frames = frames.checked_add(1)?;
            }
            Some(_) => return None,
        }
    }
    (frames > 0).then_some(Picture {
        kind: PictureKind::Gif,
        width,
        height,
        animated: false,
        frames,
    })
}

/// The format, the size and the frames that the bytes of a file of `total` bytes state,
/// when they are a PNG, JPEG, WebP or GIF picture. The image data is not decoded. The file
/// is read once, up to the parts that state these.
pub fn picture(reader: impl Read, total: u64) -> Option<Picture> {
    let mut file = Source { reader };
    let start: [u8; 4] = file.take()?;
    let found = match start[0] {
        0x89 => png(&mut file, start),
        0xff => jpeg(&mut file, start),
        b'R' => webp(&mut file, start, total),
        b'G' => gif(&mut file, start),
        _ => None,
    }?;
    (found.width > 0 && found.height > 0).then_some(found)
}

/// The format and the size that the bytes state, when they are a still PNG, JPEG or WebP
/// image. The image data itself is not decoded.
pub fn inspect(bytes: &[u8]) -> Option<Image> {
    let found = picture(bytes, bytes.len() as u64).filter(|found| !found.animated)?;
    let kind = match found.kind {
        PictureKind::Png => ImageType::Png,
        PictureKind::Jpeg => ImageType::Jpeg,
        PictureKind::Webp => ImageType::Webp,
        PictureKind::Gif => return None,
    };
    Some(Image {
        kind,
        width: found.width,
        height: found.height,
    })
}

/// Images for tests: the smallest files that state a format, a size and frames.
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

    /// A PNG with an animation chunk that states two frames.
    pub fn animated_png(width: u32, height: u32) -> Vec<u8> {
        animated_png_of(width, height, 2)
    }

    pub fn animated_png_of(width: u32, height: u32, frames: u32) -> Vec<u8> {
        let still = png(width, height);
        let mut image = still[..33].to_vec();
        let mut control = frames.to_be_bytes().to_vec();
        control.extend_from_slice(&[0; 4]);
        image.extend(chunk(b"acTL", &control));
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

    fn extended_header(width: u32, height: u32, animated: bool) -> Vec<u8> {
        let mut data = vec![if animated { 0x02 } else { 0 }, 0, 0, 0];
        data.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        data.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        data
    }

    pub fn webp_extended(width: u32, height: u32, animated: bool) -> Vec<u8> {
        riff(b"VP8X", &extended_header(width, height, animated))
    }

    /// An animated WebP with this many frame chunks, each of an odd length.
    pub fn webp_animated(width: u32, height: u32, frames: u32) -> Vec<u8> {
        let mut data = extended_header(width, height, true);
        let mut add = |kind: &[u8; 4], content: &[u8]| {
            data.extend_from_slice(kind);
            data.extend_from_slice(
                &u32::try_from(content.len())
                    .unwrap_or(u32::MAX)
                    .to_le_bytes(),
            );
            data.extend_from_slice(content);
            if content.len() % 2 == 1 {
                data.push(0);
            }
        };
        add(b"ANIM", &[0; 6]);
        for _ in 0..frames {
            add(b"ANMF", &[0; 29]);
        }
        let mut body = b"WEBP".to_vec();
        body.extend_from_slice(b"VP8X");
        body.extend_from_slice(&10u32.to_le_bytes());
        body.extend_from_slice(&data);
        let mut image = b"RIFF".to_vec();
        image.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_le_bytes());
        image.extend(body);
        image
    }

    /// A GIF with a colour table and this many frames, each with its delay.
    pub fn gif(width: u16, height: u16, frames: u32) -> Vec<u8> {
        let mut image = b"GIF89a".to_vec();
        image.extend_from_slice(&width.to_le_bytes());
        image.extend_from_slice(&height.to_le_bytes());
        image.extend_from_slice(&[0x80, 0, 0]);
        image.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        for _ in 0..frames {
            image.extend_from_slice(&[0x21, 0xf9, 4, 0, 10, 0, 0, 0]);
            image.push(0x2c);
            image.extend_from_slice(&[0, 0, 0, 0]);
            image.extend_from_slice(&width.to_le_bytes());
            image.extend_from_slice(&height.to_le_bytes());
            image.extend_from_slice(&[0, 2, 2, 0x4c, 0x01, 0]);
        }
        image.push(0x3b);
        image
    }

    /// The TIFF structure of an EXIF block with one entry, the orientation; `big` writes it
    /// in big-endian order.
    pub fn exif(orientation: u16, big: bool) -> Vec<u8> {
        let two = |value: u16| {
            if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            }
        };
        let four = |value: u32| {
            if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            }
        };
        let mut tiff = if big { b"MM".to_vec() } else { b"II".to_vec() };
        tiff.extend_from_slice(&two(42));
        tiff.extend_from_slice(&four(8));
        tiff.extend_from_slice(&two(1));
        tiff.extend_from_slice(&two(0x0112));
        tiff.extend_from_slice(&two(3));
        tiff.extend_from_slice(&four(1));
        tiff.extend_from_slice(&two(orientation));
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&four(0));
        tiff
    }

    /// A JPEG whose EXIF segment, after the start of the file, states this orientation.
    pub fn jpeg_turned(width: u16, height: u16, orientation: u16) -> Vec<u8> {
        let mut block = b"Exif\0\0".to_vec();
        block.extend(exif(orientation, true));
        let mut segment = vec![0xff, 0xe1];
        segment.extend_from_slice(
            &u16::try_from(block.len() + 2)
                .unwrap_or(u16::MAX)
                .to_be_bytes(),
        );
        segment.extend(block);
        let still = jpeg(width, height);
        let mut image = still[..2].to_vec();
        image.extend(segment);
        image.extend_from_slice(&still[2..]);
        image
    }

    /// A PNG with an EXIF chunk before its image data.
    pub fn png_turned(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        let still = png(width, height);
        let mut image = still[..33].to_vec();
        image.extend(chunk(b"eXIf", &exif(orientation, false)));
        image.extend_from_slice(&still[33..]);
        image
    }

    /// An extended still WebP with the EXIF flag, its image data, and an EXIF chunk after it.
    pub fn webp_turned(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        let mut data = extended_header(width, height, false);
        data[0] |= 0x08;
        let mut add = |kind: &[u8; 4], content: &[u8]| {
            data.extend_from_slice(kind);
            data.extend_from_slice(
                &u32::try_from(content.len())
                    .unwrap_or(u32::MAX)
                    .to_le_bytes(),
            );
            data.extend_from_slice(content);
            if content.len() % 2 == 1 {
                data.push(0);
            }
        };
        add(b"VP8L", &[0x2f, 0, 0, 0, 0]);
        add(b"EXIF", &exif(orientation, false));
        let mut body = b"WEBP".to_vec();
        body.extend_from_slice(b"VP8X");
        body.extend_from_slice(&10u32.to_le_bytes());
        body.extend_from_slice(&data);
        let mut image = b"RIFF".to_vec();
        image.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_le_bytes());
        image.extend(body);
        image
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

    fn found(kind: PictureKind, size: (u32, u32), frames: u32) -> Option<Picture> {
        Some(Picture {
            kind,
            width: size.0,
            height: size.1,
            animated: frames > 1 && kind != PictureKind::Gif,
            frames,
        })
    }

    fn of(bytes: &[u8]) -> Option<Picture> {
        picture(bytes, bytes.len() as u64)
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
            samples::gif(8, 8, 2),
            svg,
            html,
            samples::animated_png(8, 8),
            samples::webp_extended(8, 8, true),
            samples::webp_animated(8, 8, 2),
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

    /// A phone stores a portrait photo as a landscape picture with an EXIF orientation of
    /// 6; the size after the turn is stated, for every orientation that turns by a quarter
    /// and in both byte orders. Orientations 1 to 4 keep the size.
    #[test]
    fn a_picture_turned_by_its_exif_orientation_states_its_turned_size() {
        for orientation in 1..=8 {
            let quarter = orientation >= 5;
            let size = |width: u32, height: u32| {
                if quarter {
                    (height, width)
                } else {
                    (width, height)
                }
            };
            assert_eq!(
                of(&samples::jpeg_turned(4032, 3024, orientation)),
                found(PictureKind::Jpeg, size(4032, 3024), 1),
                "JPEG, orientation {orientation}"
            );
            assert_eq!(
                of(&samples::png_turned(1920, 1080, orientation)),
                found(PictureKind::Png, size(1920, 1080), 1),
                "PNG, orientation {orientation}"
            );
            assert_eq!(
                of(&samples::webp_turned(1600, 1200, orientation)),
                found(PictureKind::Webp, size(1600, 1200), 1),
                "WebP, orientation {orientation}"
            );
        }
    }

    #[test]
    fn a_picture_of_a_message_states_its_format_its_size_and_its_frames() {
        let cases = [
            (
                samples::png(2560, 1440),
                found(PictureKind::Png, (2560, 1440), 1),
            ),
            (
                samples::jpeg(5712, 4284),
                found(PictureKind::Jpeg, (5712, 4284), 1),
            ),
            (
                samples::webp_lossy(320, 200),
                found(PictureKind::Webp, (320, 200), 1),
            ),
            (
                samples::webp_extended(300, 400, false),
                found(PictureKind::Webp, (300, 400), 1),
            ),
            (
                samples::gif(498, 280, 1),
                found(PictureKind::Gif, (498, 280), 1),
            ),
            (
                samples::gif(498, 280, 37),
                found(PictureKind::Gif, (498, 280), 37),
            ),
            (
                samples::webp_animated(480, 270, 12),
                found(PictureKind::Webp, (480, 270), 12),
            ),
            (
                samples::animated_png_of(100, 80, 9),
                found(PictureKind::Png, (100, 80), 9),
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(of(&bytes), expected, "{expected:?}");
            // A reader that gives one byte at a time states the same.
            let slow = std::io::BufReader::with_capacity(1, bytes.as_slice());
            assert_eq!(picture(slow, bytes.len() as u64), expected);
        }
    }

    #[test]
    fn an_animation_without_frames_or_with_a_cut_frame_is_no_picture() {
        let mut cut_webp = samples::webp_animated(8, 8, 3);
        cut_webp.truncate(cut_webp.len() - 10);
        let stated = u32::try_from(cut_webp.len() - 8).unwrap().to_le_bytes();
        cut_webp[4..8].copy_from_slice(&stated);
        let mut cut_gif = samples::gif(8, 8, 3);
        cut_gif.truncate(cut_gif.len() - 3);
        let mut other_block = samples::gif(8, 8, 1);
        other_block.insert(other_block.len() - 1, 0x99);
        for bytes in [
            samples::webp_extended(8, 8, true),
            samples::webp_animated(8, 8, 0),
            cut_webp,
            samples::gif(8, 8, 0),
            cut_gif,
            other_block,
            samples::animated_png_of(8, 8, 0),
        ] {
            assert_eq!(of(&bytes), None, "{bytes:?}");
        }
        // A GIF without its trailer is read up to its end.
        let mut open = samples::gif(8, 8, 2);
        open.pop();
        assert_eq!(of(&open), found(PictureKind::Gif, (8, 8), 2));
    }
}
