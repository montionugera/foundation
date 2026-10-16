//! The readers of hosts never panic, and a host reads back from its text.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use env::net::host::{self, Host};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    if let Ok(host) = text.parse::<Host>() {
        assert_eq!(host.to_string().parse(), Ok(host.clone()), "the host changed");
        if let Some(name) = host.name() {
            assert!(!name.bytes().any(|b| b.is_ascii_uppercase()), "{name}");
        }
    }
    if let Ok(host) = host::uri(text) {
        assert_eq!(host.to_string().parse(), Ok(host.clone()), "the URI host changed");
    }
});
