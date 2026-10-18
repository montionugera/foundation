//! `connector::http::Roots::with_pem` never panics on the text of a `ca`, and a UTF-8
//! byte order mark at the start does not change what it gives.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use connector::http::Roots;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let got = Roots::default().with_pem(text);
    if !text.starts_with('\u{feff}') {
        let marked = Roots::default().with_pem(&format!("\u{feff}{text}"));
        assert_eq!(marked.as_ref().err(), got.as_ref().err(), "{text:?}");
    }
    if let Ok(roots) = got {
        assert!(!roots.empty(), "{text:?}");
    }
});
