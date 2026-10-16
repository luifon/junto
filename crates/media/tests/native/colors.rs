pub fn matches(actual: (u8, u8, u8), expected: (u8, u8, u8)) -> bool {
    actual.0.abs_diff(expected.0) < 12
        && actual.1.abs_diff(expected.1) < 12
        && actual.2.abs_diff(expected.2) < 12
}
