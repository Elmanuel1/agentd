use super::*;

#[test]
fn both_day_fields_restricted_is_a_union_and_one_alone_is_kept() {
    assert_eq!(crons("0 9 1 * MON"), ["0 9 1 * *", "0 9 * * MON"]);
    assert_eq!(crons("0 9 1,15 6 1-5"), ["0 9 1,15 6 *", "0 9 * 6 1-5"]);
    for one in ["0 9 1 * *", "0 9 * * MON", "* * * * *"] {
        assert_eq!(crons(one), [one]);
    }
}
