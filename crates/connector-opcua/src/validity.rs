//! Checks that the validity of a certificate in the open62541 copy reads the wall time
//! of the event loop of its group, never the OS clock.
//!
//! The fixtures in `validity/` are P-256 certificates in DER from Python
//! `cryptography` 41.0.7. Each window but that of [`LONG`] is before the OS clock, so a
//! `Good` result comes only from the loop.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use env::rng::Rng;
use types::time::{Span, Stamp};

use crate::event::{self, Loop};
use crate::ffi::{self, Bytes, Status};
use crate::secure::{END, START};

/// Self-signed, not a CA, valid from [`START`] to [`END`].
const LEAF: &[u8] = include_bytes!("validity/leaf.der");

/// A self-signed CA (keyCertSign and cRLSign), valid from [`START`] to [`CA_END`].
const CA: &[u8] = include_bytes!("validity/ca.der");

/// Not a CA, issued by [`CA`], valid from [`START`] to [`END`].
const ISSUED: &[u8] = include_bytes!("validity/issued.der");

/// Self-signed, not a CA, valid from [`START`] to [`LONG_END`], so the OS clock is
/// inside its window.
const LONG: &[u8] = include_bytes!("validity/long.der");

/// Self-signed, not a CA, valid from the Unix epoch to one hour after it.
const EPOCH: &[u8] = include_bytes!("validity/epoch.der");

/// 2001-07-01.
const CA_END: Stamp = from_secs(993_945_600);

/// 2200-01-01.
const LONG_END: Stamp = from_secs(7_258_118_400);

/// 2001-03-01.
const MARCH: Stamp = from_secs(983_404_800);

/// 2001-08-01.
const AUGUST: Stamp = from_secs(996_624_000);

const fn from_secs(secs: i64) -> Stamp {
    Stamp::from_nanos(secs * Span::SECOND.nanos())
}

/// Gives the status of the check of `certificate` by a memory group that trusts
/// `trusted`, or nothing when it is `None`, on a loop whose wall time is `wall`.
pub(crate) fn verify(
    certificate: &[u8],
    trusted: Option<&[u8]>,
    wall: Stamp,
) -> Status {
    let mut sim = sim::Sim::new(sim::Config::default());
    let clock = sim.node(sim::node::Config::default()).clock();
    let time = event::Time {
        clock,
        wall: Box::new(move || wall),
    };
    let events = Loop::new(time, &mut Rng::from_seed(0));
    let trusted = trusted.map(Bytes::of);
    // SAFETY: both point at live bytes or are null, the call only reads them, and the
    // loop lives past the call.
    Status(unsafe {
        ffi::test::shim_verify_certificate(
            &Bytes::of(certificate),
            trusted
                .as_ref()
                .map_or(std::ptr::null(), std::ptr::from_ref),
            events.raw(),
        )
    })
}

/// Gives the name of the status of [`verify`] of `certificate` by a group that trusts
/// `trusted`.
fn name(certificate: &[u8], trusted: &[u8], wall: Stamp) -> &'static str {
    verify(certificate, Some(trusted), wall).name()
}

/// Each bound of the window of a leaf, `notBefore` and `notAfter` in
/// `openSSL_verifyChain`, follows the wall time of the loop.
#[test]
fn a_leaf_is_valid_only_inside_its_window_on_the_loop() {
    let at = [
        (START - Span::NANOSECOND, "BadCertificateTimeInvalid"),
        (START, "Good"),
        (END - Span::NANOSECOND, "Good"),
        (END, "BadCertificateTimeInvalid"),
    ];
    for (wall, expected) in at {
        assert_eq!(name(LEAF, LEAF, wall), expected, "{wall}");
    }
}

/// A certificate past its window on the loop is refused while the OS clock is inside
/// it.
#[test]
fn a_leaf_past_its_window_on_the_loop_is_refused_inside_it_on_the_os_clock() {
    assert_eq!(name(LONG, LONG, LONG_END), "BadCertificateTimeInvalid");
}

/// A wall time less than a second before the Unix epoch is in the second before it, so
/// it is before `notBefore` of [`EPOCH`].
#[test]
fn a_wall_just_before_the_epoch_is_before_a_window_from_the_epoch() {
    let second = Stamp::EPOCH - Span::SECOND;
    let before = [
        second - Span::NANOSECOND,
        second + Span::NANOSECOND,
        Stamp::EPOCH - Span::NANOSECOND,
    ];
    for wall in before {
        assert_eq!(
            name(EPOCH, EPOCH, wall),
            "BadCertificateTimeInvalid",
            "{wall}"
        );
    }
    assert_eq!(name(EPOCH, EPOCH, Stamp::EPOCH), "Good");
}

/// An issuer outside its window gives its own code, while its leaf is inside.
#[test]
fn an_issuer_past_its_window_on_the_loop_is_refused() {
    const { assert!(CA_END.nanos() < AUGUST.nanos() && AUGUST.nanos() < END.nanos()) };
    assert_eq!(name(ISSUED, CA, MARCH), "Good");
    assert_eq!(name(ISSUED, CA, AUGUST), "BadCertificateIssuerTimeInvalid");
}

/// The constructor of a memory group refuses a group with no loop, which has no time,
/// so no group exists that refuses only at its first check.
#[test]
fn a_group_with_no_loop_is_refused() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let clock = sim.node(sim::node::Config::default()).clock();
    let events = Loop::new(event::Time::monotonic(clock), &mut Rng::from_seed(0));
    // SAFETY: the loop lives past the call, and the call takes a null loop.
    let created = |el| Status(unsafe { ffi::test::shim_memory_group(el) }).name();
    assert_eq!(created(events.raw()), "Good");
    assert_eq!(created(std::ptr::null_mut()), "BadInternalError");
}
