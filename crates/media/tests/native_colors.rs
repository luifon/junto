#[path = "native/colors.rs"]
mod colors;

#[test]
fn green_decoded_with_the_wrong_matrix_is_rejected() {
    assert!(colors::matches((27, 203, 25), (26, 204, 26)));
    assert!(!colors::matches((39, 236, 32), (26, 204, 26)));
}
