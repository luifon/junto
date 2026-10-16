#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub frames: usize,
    pub stopped: usize,
    pub invalid: bool,
    pub dimensions: (u32, u32),
    pub configured: (u32, u32),
    /// Frames that arrived as owned I420 planes, which the camera produces only for tagged 709
    /// buffers.
    pub normalized: usize,
}

impl Progress {
    pub fn picture(&mut self, dimensions: (u32, u32), valid: bool) {
        self.invalid |= self.frames > 0 && self.dimensions != dimensions;
        self.invalid |= self.configured != (0, 0) && self.configured != dimensions;
        self.frames += 1;
        self.dimensions = dimensions;
        self.invalid |= !valid || dimensions.0 == 0 || dimensions.1 == 0;
    }
    pub fn configure(&mut self, dimensions: (u32, u32)) {
        self.invalid |= self.frames > 0 && self.dimensions != dimensions;
        self.configured = dimensions;
    }
}
