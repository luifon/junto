use super::*;

fn layout(pixel: Pixel, matrix: Tag) -> Layout {
    Layout {
        width: 4,
        height: 2,
        stride: 8,
        pixel,
        matrix,
    }
}

#[test]
fn padded_nv12_and_i420_have_the_same_601_planes() {
    let nv12 = [
        135, 135, 135, 135, 0, 0, 0, 0, 135, 135, 135, 135, 0, 0, 0, 0, 67, 57, 67, 57, 0, 0, 0, 0,
    ];
    let i420 = [
        135, 135, 135, 135, 0, 0, 0, 0, 135, 135, 135, 135, 0, 0, 0, 0, 67, 67, 0, 0, 57, 57, 0, 0,
    ];
    for (pixel, bytes) in [(Pixel::Nv12, nv12), (Pixel::I420, i420)] {
        let frame = layout(pixel, Tag::Bt601).convert(&bytes).unwrap();
        let (y, u, v) = frame.data();
        assert_eq!(y, &[135; 8]);
        assert_eq!(u, &[67; 2]);
        assert_eq!(v, &[57; 2]);
    }
}

#[test]
fn camera_709_green_is_normalized_before_publication() {
    let bytes = [
        157, 157, 157, 157, 0, 0, 0, 0, 157, 157, 157, 157, 0, 0, 0, 0, 58, 51, 58, 51, 0, 0, 0, 0,
    ];
    let frame = layout(Pixel::Nv12, Tag::Bt709).convert(&bytes).unwrap();
    let (y, u, v) = frame.data();
    assert_eq!(y, &[135; 8]);
    assert_eq!(u, &[67; 2]);
    assert_eq!(v, &[57; 2]);
}

#[test]
fn short_planes_odd_sizes_and_oversized_frames_are_refused() {
    for pixel in [Pixel::Nv12, Pixel::I420, Pixel::Yuy2] {
        let good = layout(pixel, Tag::Missing);
        assert!(good.convert(&[0; 15]).is_err());
        for bad in [
            Layout { stride: 1, ..good },
            Layout { width: 0, ..good },
            Layout { height: 3, ..good },
            Layout {
                width: u32::MAX,
                ..good
            },
            Layout {
                stride: usize::MAX,
                ..good
            },
        ] {
            assert!(bad.convert(&[0; 32]).is_err());
        }
    }
}

#[test]
fn packed_422_averages_chroma_rows_without_mirroring() {
    let bytes = [
        30, 60, 40, 80, 50, 70, 60, 90, 70, 80, 80, 100, 90, 90, 100, 110,
    ];
    let frame = layout(Pixel::Yuy2, Tag::Bt601).convert(&bytes).unwrap();
    let (y, u, v) = frame.data();
    assert_eq!(y, &[30, 40, 50, 60, 70, 80, 90, 100]);
    assert_eq!(u, &[70, 80]);
    assert_eq!(v, &[90, 100]);
}
