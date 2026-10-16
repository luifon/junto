#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub frames: usize,
    pub stopped: usize,
    pub invalid: bool,
    pub dimensions: (u32, u32),
    pub configured: (u32, u32),
    /// Frames that arrived as owned I420 planes, which the camera code produces only for buffers
    /// in a matrix other than 601.
    pub converted: usize,
    /// Frames that arrived on the native path in the 601 matrix.
    pub native_601: usize,
}

impl Progress {
    pub fn picture(&mut self, dimensions: (u32, u32), valid: bool) {
        self.invalid |= self.frames > 0 && self.dimensions != dimensions;
        self.invalid |= self.configured != (0, 0) && self.configured != dimensions;
        self.frames += 1;
        self.dimensions = dimensions;
        self.invalid |= !valid || dimensions.0 == 0 || dimensions.1 == 0;
    }
    /// Counts how the camera code delivered a frame: `native_601` is `None` for a converted
    /// buffer.
    pub fn delivered(&mut self, converted: bool, native_601: Option<bool>) {
        self.converted += usize::from(converted);
        self.native_601 += usize::from(native_601 == Some(true));
    }
    /// The frames since `before` that the camera code must have converted, and the frames that
    /// it converted.
    pub fn conversions(&self, before: &Progress) -> (usize, usize) {
        (
            (self.frames - before.frames) - (self.native_601 - before.native_601),
            self.converted - before.converted,
        )
    }
    pub fn configure(&mut self, dimensions: (u32, u32)) {
        self.invalid |= self.frames > 0 && self.dimensions != dimensions;
        self.configured = dimensions;
    }
}
