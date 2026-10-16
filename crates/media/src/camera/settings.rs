use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSNumber, NSString};

pub(super) fn video(size: (u32, u32)) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    let key = NSString::from_str("PixelFormatType");
    let width_key = NSString::from_str("Width");
    let height_key = NSString::from_str("Height");
    let format = NSNumber::new_u32(u32::from_be_bytes(*b"420v"));
    let width = NSNumber::new_u32(size.0);
    let height = NSNumber::new_u32(size.1);
    // SAFETY: these framework constants are immutable.
    let (scaling_key, scaling_mode) = unsafe {
        (
            objc2_av_foundation::AVVideoScalingModeKey?,
            objc2_av_foundation::AVVideoScalingModeResizeAspect?,
        )
    };
    Some(NSDictionary::<NSString, AnyObject>::from_slices(
        &[&*key, &*width_key, &*height_key, scaling_key],
        &[&*format, &*width, &*height, scaling_mode],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_settings_enforce_both_declared_dimensions() {
        let settings = video((640, 480)).unwrap();
        for (key, expected) in [
            ("Width", 640),
            ("Height", 480),
            ("PixelFormatType", u32::from_be_bytes(*b"420v")),
        ] {
            let value = settings.objectForKey(&NSString::from_str(key)).unwrap();
            assert_eq!(value.downcast_ref::<NSNumber>().unwrap().as_u32(), expected);
        }
    }
    #[test]
    fn resizing_preserves_the_camera_aspect_ratio() {
        // SAFETY: these framework constants are immutable.
        unsafe {
            use objc2_av_foundation::{AVVideoScalingModeKey, AVVideoScalingModeResizeAspect};
            let settings = video((640, 480)).unwrap();
            let value = settings
                .objectForKey(AVVideoScalingModeKey.unwrap())
                .unwrap();
            assert_eq!(
                value.downcast_ref::<NSString>().unwrap(),
                AVVideoScalingModeResizeAspect.unwrap()
            );
        }
    }
}
