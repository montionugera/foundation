//! Checks that the validity of a certificate in the open62541 copy reads the wall time
//! of the event loop of its group, never the OS clock.
//!
//! The fixtures in `validity/` are P-256 certificates in DER from Python
//! `cryptography` 41.0.7. Each window is in 2001, before the OS clock, so a `Good`
//! result comes only from the loop.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use env::rng::Rng;
use types::time::Stamp;

use crate::event::{self, Loop};
use crate::ffi::{self, Bytes, Status};

/// Self-signed, not a CA, valid from [`START`] to [`END`].
const LEAF: &[u8] = include_bytes!("validity/leaf.der");

/// A self-signed CA (keyCertSign and cRLSign), valid from [`START`] to [`CA_END`].
const CA: &[u8] = include_bytes!("validity/ca.der");

/// Not a CA, issued by [`CA`], valid from [`START`] to [`END`].
const ISSUED: &[u8] = include_bytes!("validity/issued.der");

/// 2001-01-01, in seconds from the Unix epoch.
const START: i64 = 978_307_200;

/// 2002-01-01.
const END: i64 = 1_009_843_200;

/// 2001-07-01.
const CA_END: i64 = 993_945_600;

/// 2001-03-01.
const MARCH: i64 = 983_404_800;

/// 2001-08-01.
const AUGUST: i64 = 996_624_000;

const SECOND: i64 = 1_000_000_000;

fn bytes(der: &[u8]) -> Bytes {
    Bytes {
        length: der.len(),
        data: der.as_ptr().cast_mut(),
    }
}

/// Gives the status of the check of `certificate` by a memory group that trusts
/// `trusted`, on a loop whose wall time is `nanos` from the Unix epoch.
fn verify(certificate: &[u8], trusted: &[u8], nanos: i64) -> &'static str {
    let mut sim = sim::Sim::new(sim::Config::default());
    let clock = sim.node(sim::node::Config::default()).clock();
    let time = event::Time {
        clock,
        wall: Box::new(move || Stamp::from_nanos(nanos)),
    };
    let events = Loop::new(time, &mut Rng::from_seed(0));
    // SAFETY: both point at live bytes, which the call only reads, and the loop lives
    // past the call.
    let status = unsafe {
        ffi::test::shim_verify_certificate(
            &bytes(certificate),
            &bytes(trusted),
            events.raw(),
        )
    };
    Status(status).name()
}

/// Each bound of the window of a leaf, `notBefore` and `notAfter` in
/// `openSSL_verifyChain`, follows the wall time of the loop.
#[test]
fn a_leaf_is_valid_only_inside_its_window_on_the_loop() {
    let at = [
        (START * SECOND - 1, "BadCertificateTimeInvalid"),
        (START * SECOND, "Good"),
        (END * SECOND - 1, "Good"),
        (END * SECOND, "BadCertificateTimeInvalid"),
    ];
    for (nanos, expected) in at {
        assert_eq!(verify(LEAF, LEAF, nanos), expected, "{nanos}");
    }
}

/// An issuer outside its window gives its own code, while its leaf is inside.
#[test]
fn an_issuer_past_its_window_on_the_loop_is_refused() {
    const { assert!(CA_END < AUGUST && AUGUST < END) };
    assert_eq!(verify(ISSUED, CA, MARCH * SECOND), "Good");
    assert_eq!(
        verify(ISSUED, CA, AUGUST * SECOND),
        "BadCertificateIssuerTimeInvalid"
    );
}

/// A group with no loop has no time, so it refuses each certificate.
#[test]
fn a_group_with_no_loop_refuses_each_certificate() {
    // SAFETY: both point at live bytes, which the call only reads.
    let status = unsafe {
        ffi::test::shim_verify_certificate(
            &bytes(LEAF),
            &bytes(LEAF),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(Status(status).name(), "BadInternalError");
}
