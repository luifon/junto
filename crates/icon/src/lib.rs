//! The application icon (ADR 0756). Every raster of the packages is a rendering of the SVG
//! source `packaging/icon/junto.svg` by resvg: the PNG pictures of the Linux icon theme, the
//! macOS icon file, the Windows icon file and the icon resource of the Windows program.

/// The source of every rendering: the rounded square with the margin of the macOS icon grid.
pub const SVG: &str = include_str!("../../../packaging/icon/junto.svg");

/// How an entry of the macOS icon file holds its picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// A PNG picture.
    Png,
    /// `ARGB` followed by the alpha, red, green and blue values of the pixels, each channel
    /// with the run-length code of the icon file. `iconutil` of macOS writes the entries of
    /// 16 and 32 pixels at 1x in this form.
    Argb,
}

/// The entries of the macOS icon file: the type of each entry, the side of its picture in
/// pixels and its encoding. Each size of the macOS icon grid from 16 to 512 points, at 1x and
/// at 2x, with the types and encodings that `iconutil` of macOS writes.
pub const ICNS: [(&[u8; 4], u32, Encoding); 10] = [
    (b"ic04", 16, Encoding::Argb),
    (b"ic11", 32, Encoding::Png),
    (b"ic05", 32, Encoding::Argb),
    (b"ic12", 64, Encoding::Png),
    (b"ic07", 128, Encoding::Png),
    (b"ic13", 256, Encoding::Png),
    (b"ic08", 256, Encoding::Png),
    (b"ic14", 512, Encoding::Png),
    (b"ic09", 512, Encoding::Png),
    (b"ic10", 1024, Encoding::Png),
];

/// The pictures of the Windows icon. Windows reads pictures up to 64 pixels as bitmaps and
/// the picture of 256 pixels as PNG.
pub const ICO: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// The side of the window icon that the client gives to an X11 window manager.
pub const WINDOW: u32 = 64;

/// The side of the picture that a notification of the client shows on Windows.
pub const NOTIFICATION: u32 = 256;

/// The mark of a call state that a tray picture draws over the application icon (ADR 0759).
/// Each source is a picture of 1024 by 1024 with the mark in the lower right corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// In a call: a green disc.
    Call,
    /// In a call with the microphone closed: a red disc with a crossed microphone.
    Muted,
    /// In a call with the sound off: a red disc with crossed headphones.
    SoundOff,
}

impl Mark {
    pub const ALL: [Mark; 3] = [Mark::Call, Mark::Muted, Mark::SoundOff];

    pub fn svg(self) -> &'static str {
        match self {
            Mark::Call => include_str!("../../../packaging/icon/marks/call.svg"),
            Mark::Muted => include_str!("../../../packaging/icon/marks/muted.svg"),
            Mark::SoundOff => include_str!("../../../packaging/icon/marks/sound-off.svg"),
        }
    }

    /// The name of the mark in the names of files.
    pub fn name(self) -> &'static str {
        match self {
            Mark::Call => "call",
            Mark::Muted => "muted",
            Mark::SoundOff => "sound-off",
        }
    }
}

/// The sides of the tray pictures of Windows and Linux: the small icon of Windows at 100, 125,
/// 150, 200 and 300 % of scale, and the sizes that Linux status areas ask for.
pub const TRAY: [u32; 6] = [16, 20, 22, 24, 32, 48];

/// The side of the tray picture of macOS: 18 points of the menu bar at 2x.
pub const MENU_BAR: u32 = 36;

/// The side of the body of the icon in the source, and the margin of the macOS icon grid
/// around it.
#[cfg(feature = "render")]
const BODY: f32 = 824.0;
#[cfg(feature = "render")]
const MARGIN: f32 = 100.0;

/// The largest picture that this crate renders: the largest entry of the macOS icon file.
pub const LARGEST: u32 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(feature = "render")]
    #[error("the SVG source of the icon is not valid: {0}")]
    Source(#[from] resvg::usvg::Error),
    #[error("this build of junto-icon has no renderer: the build needs its feature `render`")]
    NoRenderer,
    #[error("an icon picture has a side of 1 to {LARGEST} pixels, not {0}")]
    Size(u32),
    #[error("the PNG picture of {0} pixels was not encoded: {1}")]
    Png(u32, String),
}

/// A square rendering of the icon with straight (not premultiplied) RGBA pixels, row by row
/// from the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub size: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * self.size + x) * 4) as usize;
        [
            self.rgba[start],
            self.rgba[start + 1],
            self.rgba[start + 2],
            self.rgba[start + 3],
        ]
    }
}

/// The part of the crate that uses resvg. Without the feature `render` every rendering fails
/// with [`Error::NoRenderer`] (ADR 0756).
#[cfg(feature = "render")]
mod renderer {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg;

    use super::{Error, LARGEST, Picture};

    fn pixmap(size: u32) -> Result<Pixmap, Error> {
        if !(1..=LARGEST).contains(&size) {
            return Err(Error::Size(size));
        }
        let tree = usvg::Tree::from_str(super::SVG, &usvg::Options::default())?;
        let mut pixmap = Pixmap::new(size, size).ok_or(Error::Size(size))?;
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        Ok(pixmap)
    }

    /// The body of the icon fills the picture without the margin and the shadow of the macOS
    /// icon grid, and the mark is drawn over it.
    fn tray_pixmap(mark: Option<super::Mark>, size: u32) -> Result<Pixmap, Error> {
        if !(1..=LARGEST).contains(&size) {
            return Err(Error::Size(size));
        }
        let tree = usvg::Tree::from_str(super::SVG, &usvg::Options::default())?;
        let mut pixmap = Pixmap::new(size, size).ok_or(Error::Size(size))?;
        let scale = size as f32 / super::BODY;
        let shift = -super::MARGIN * scale;
        resvg::render(
            &tree,
            Transform::from_row(scale, 0.0, 0.0, scale, shift, shift),
            &mut pixmap.as_mut(),
        );
        if let Some(mark) = mark {
            let tree = usvg::Tree::from_str(mark.svg(), &usvg::Options::default())?;
            let scale = size as f32 / tree.size().width();
            resvg::render(
                &tree,
                Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            );
        }
        Ok(pixmap)
    }

    fn straight(pixmap: &Pixmap, size: u32) -> Picture {
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect();
        Picture { size, rgba }
    }

    pub fn render(size: u32) -> Result<Picture, Error> {
        Ok(straight(&pixmap(size)?, size))
    }

    pub fn tray(mark: Option<super::Mark>, size: u32) -> Result<Picture, Error> {
        Ok(straight(&tray_pixmap(mark, size)?, size))
    }

    pub fn tray_png(mark: Option<super::Mark>, size: u32) -> Result<Vec<u8>, Error> {
        tray_pixmap(mark, size)?
            .encode_png()
            .map_err(|error| Error::Png(size, error.to_string()))
    }

    pub fn png(size: u32) -> Result<Vec<u8>, Error> {
        pixmap(size)?
            .encode_png()
            .map_err(|error| Error::Png(size, error.to_string()))
    }

    pub fn premultiplied(size: u32) -> Result<Vec<u8>, Error> {
        Ok(pixmap(size)?.take())
    }
}

#[cfg(not(feature = "render"))]
mod renderer {
    use super::{Error, Picture};

    pub fn render(_: u32) -> Result<Picture, Error> {
        Err(Error::NoRenderer)
    }

    pub fn png(_: u32) -> Result<Vec<u8>, Error> {
        Err(Error::NoRenderer)
    }

    pub fn tray(_: Option<super::Mark>, _: u32) -> Result<Picture, Error> {
        Err(Error::NoRenderer)
    }

    pub fn tray_png(_: Option<super::Mark>, _: u32) -> Result<Vec<u8>, Error> {
        Err(Error::NoRenderer)
    }

    pub fn premultiplied(_: u32) -> Result<Vec<u8>, Error> {
        Err(Error::NoRenderer)
    }
}

/// The icon rendered at `size` by `size` pixels.
pub fn render(size: u32) -> Result<Picture, Error> {
    renderer::render(size)
}

/// The icon rendered at `size` by `size` pixels, as a PNG picture.
pub fn png(size: u32) -> Result<Vec<u8>, Error> {
    renderer::png(size)
}

/// The tray picture of `size` by `size` pixels: the body of the icon, with the mark of a call
/// state in a call (ADR 0759).
pub fn tray(mark: Option<Mark>, size: u32) -> Result<Picture, Error> {
    renderer::tray(mark, size)
}

/// The tray picture of [`tray`] as a PNG picture.
pub fn tray_png(mark: Option<Mark>, size: u32) -> Result<Vec<u8>, Error> {
    renderer::tray_png(mark, size)
}

/// The macOS icon file: an entry for each line of [`ICNS`].
pub fn icns() -> Result<Vec<u8>, Error> {
    let mut entries = Vec::new();
    for (kind, size, encoding) in ICNS {
        let picture = match encoding {
            Encoding::Png => png(size)?,
            Encoding::Argb => argb(&renderer::premultiplied(size)?),
        };
        entries.extend_from_slice(kind);
        entries.extend_from_slice(&length(8 + picture.len()).to_be_bytes());
        entries.extend_from_slice(&picture);
    }
    let mut file = b"icns".to_vec();
    file.extend_from_slice(&length(8 + entries.len()).to_be_bytes());
    file.extend_from_slice(&entries);
    Ok(file)
}

/// The `ARGB` entry of premultiplied RGBA pixels, row by row from the top.
fn argb(premultiplied: &[u8]) -> Vec<u8> {
    let mut data = b"ARGB".to_vec();
    for channel in [3, 0, 1, 2] {
        let values: Vec<u8> = premultiplied
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| {
                let alpha = u32::from(pixel[3]);
                if channel == 3 {
                    pixel[3]
                } else {
                    // Integer rounding of straight channels matches the ARGB entries of iconutil.
                    (u32::from(pixel[channel]) * 255 + alpha / 2)
                        .checked_div(alpha)
                        .unwrap_or(0) as u8
                }
            })
            .collect();
        data.extend_from_slice(&run_length(&values));
    }
    data
}

/// The run-length code of the icon file: a byte below 128 is followed by that number plus one
/// of values as they are; a byte of 128 or more is followed by one value that repeats that
/// number minus 125 times (3 to 130).
fn run_length(values: &[u8]) -> Vec<u8> {
    let mut code = Vec::new();
    let mut literal = Vec::new();
    let flush = |code: &mut Vec<u8>, literal: &mut Vec<u8>| {
        for chunk in literal.chunks(128) {
            code.push((chunk.len() - 1) as u8);
            code.extend_from_slice(chunk);
        }
        literal.clear();
    };
    let mut position = 0;
    while position < values.len() {
        let value = values[position];
        let run = values[position..]
            .iter()
            .take(130)
            .take_while(|&&other| other == value)
            .count();
        if run >= 3 {
            flush(&mut code, &mut literal);
            code.push((run + 125) as u8);
            code.push(value);
        } else {
            literal.extend_from_slice(&values[position..position + run]);
        }
        position += run;
    }
    flush(&mut code, &mut literal);
    code
}

fn length(bytes: usize) -> u32 {
    // The largest file holds ten PNG pictures of at most 1024 pixels: far below 4 GiB.
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

/// One picture of the Windows icon: its side and the bytes that the icon file and the
/// resource hold for it.
struct Image {
    size: u32,
    data: Vec<u8>,
}

/// A picture as Windows reads it in an icon: a bitmap of 32 bits per pixel (blue, green,
/// red, alpha) with its rows from the bottom, followed by the mask of one bit per pixel that
/// marks the transparent pixels. The height in the header counts both.
fn bitmap(picture: &Picture) -> Vec<u8> {
    let size = picture.size;
    let mask_row = size.div_ceil(32) * 4;
    let pixel_bytes = size * size * 4;
    let mask_bytes = mask_row * size;
    let mut data = Vec::with_capacity((40 + pixel_bytes + mask_bytes) as usize);
    for value in [40, size, size * 2] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&32u16.to_le_bytes());
    for value in [0, pixel_bytes + mask_bytes, 0, 0, 0, 0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for y in (0..size).rev() {
        for x in 0..size {
            let [red, green, blue, alpha] = picture.pixel(x, y);
            data.extend_from_slice(&[blue, green, red, alpha]);
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_row as usize];
        for x in 0..size {
            if picture.pixel(x, y)[3] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        data.extend_from_slice(&row);
    }
    data
}

fn images() -> Result<Vec<Image>, Error> {
    ICO.iter()
        .map(|&size| {
            let data = if size >= 256 {
                png(size)?
            } else {
                bitmap(&render(size)?)
            };
            Ok(Image { size, data })
        })
        .collect()
}

/// The fields that the icon file and the icon group resource share for one picture: side,
/// side, no palette, a reserved byte, one plane, 32 bits per pixel, the length of the data.
/// A side of 256 is written as 0.
fn entry(image: &Image) -> Vec<u8> {
    let side = if image.size >= 256 {
        0
    } else {
        image.size as u8
    };
    let mut entry = vec![side, side, 0, 0];
    entry.extend_from_slice(&1u16.to_le_bytes());
    entry.extend_from_slice(&32u16.to_le_bytes());
    entry.extend_from_slice(&length(image.data.len()).to_le_bytes());
    entry
}

fn directory_header(count: usize) -> Vec<u8> {
    let mut header = 0u16.to_le_bytes().to_vec();
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&(count as u16).to_le_bytes());
    header
}

/// The Windows icon file with the pictures of [`ICO`].
pub fn ico() -> Result<Vec<u8>, Error> {
    let images = images()?;
    let mut file = directory_header(images.len());
    let mut offset = 6 + 16 * images.len();
    for image in &images {
        file.extend_from_slice(&entry(image));
        file.extend_from_slice(&length(offset).to_le_bytes());
        offset += image.data.len();
    }
    for image in &images {
        file.extend_from_slice(&image.data);
    }
    Ok(file)
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// The number of the icon group in the program. gpui gives the windows of the program the
/// icon group with this number, and Explorer shows the first group of a program.
pub const ICON_GROUP: u16 = 1;

/// One resource of a compiled resource file: a header with the type and the number of the
/// resource, then its data, each padded to four bytes.
fn resource(file: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    file.extend_from_slice(&length(data.len()).to_le_bytes());
    file.extend_from_slice(&32u32.to_le_bytes());
    for value in [0xffff, kind, 0xffff, name] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    // Data version, memory flags, the neutral language, version and characteristics.
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&flags.to_le_bytes());
    file.extend_from_slice(&0u16.to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(data);
    file.resize(file.len().next_multiple_of(4), 0);
}

/// The compiled resource file (`.res`) that the linker of the MSVC tools puts into the
/// Windows program: one icon resource for each picture of [`ICO`], numbered from 1, and the
/// icon group [`ICON_GROUP`] that lists them.
pub fn windows_resource() -> Result<Vec<u8>, Error> {
    let images = images()?;
    // A compiled resource file starts with an empty resource of type 0 and number 0.
    let mut file = Vec::new();
    resource(&mut file, 0, 0, 0, &[]);
    let mut group = directory_header(images.len());
    for (number, image) in (1u16..).zip(&images) {
        resource(&mut file, RT_ICON, number, 0x1010, &image.data);
        group.extend_from_slice(&entry(image));
        group.extend_from_slice(&number.to_le_bytes());
    }
    resource(&mut file, RT_GROUP_ICON, ICON_GROUP, 0x1030, &group);
    Ok(file)
}

#[cfg(test)]
mod tests;
