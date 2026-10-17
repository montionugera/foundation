use proptest::prelude::*;

use super::{LONGEST, MARK, cut};

proptest! {
    #[test]
    fn a_cut_fits_and_keeps_a_head_and_a_tail_of_the_text(
        text in "(a|é|😀){0,700}",
    ) {
        let cut = cut(&text);
        prop_assert!(cut.len() <= LONGEST, "{} bytes", cut.len());
        if text.len() <= LONGEST {
            prop_assert_eq!(&*cut, text.as_str());
        } else {
            let (head, tail) = cut.split_once(MARK).expect("a cut text holds the mark");
            let half = (LONGEST - MARK.len()) / 2;
            prop_assert!(text.starts_with(head) && text.ends_with(tail));
            prop_assert!(head.len() > half - 4 && tail.len() > half - 4);
        }
    }
}
