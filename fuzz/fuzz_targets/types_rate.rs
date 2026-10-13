//! The reader of rates never panics, and a rate it reads is a valid reduced fraction
//! whose denominator divides 10^9, because text gives at most 9 places of a hertz.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::fuzz_target;
use types::time::Rate;

fuzz_target!(|text: &str| {
    let Ok(rate) = text.parse::<Rate>() else {
        return;
    };
    assert_eq!(Rate::new(rate.num(), rate.den()), Ok(rate), "{text:?}");
    assert_eq!(1_000_000_000 % rate.den(), 0, "{text:?} gives {rate:?}");
});
