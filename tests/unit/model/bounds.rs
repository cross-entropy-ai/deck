use crate::bounds::step_clamped;

#[test]
fn step_clamped_covers_movement_boundaries_and_degenerate_lists() {
    let cases = [
        ("forward", 0, 3, 1, 1),
        ("forward to last", 1, 3, 1, 2),
        ("forward at last", 2, 3, 1, 2),
        ("backward", 2, 3, -1, 1),
        ("backward to first", 1, 3, -1, 0),
        ("backward at first", 0, 3, -1, 0),
        ("empty forward", 0, 0, 1, 0),
        ("empty backward", 0, 0, -1, 0),
        ("single forward", 0, 1, 1, 0),
        ("single backward", 0, 1, -1, 0),
    ];
    for (name, current, len, direction, expected) in cases {
        assert_eq!(step_clamped(current, len, direction), expected, "{name}");
    }
}
