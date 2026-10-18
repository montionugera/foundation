//! open62541 reads any certificate that a server sends with no memory fault or leak
//! and no read of the console: the check of a certificate group that trusts nothing,
//! which never gives `Good`, the channel context of each policy, and the compare with
//! the certificate of a channel. It also reads the application URI.
//!
//! Input: the certificate, DER or PEM (`connector_opcua::fuzz::certificate`).

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut sim = sim::Sim::new(sim::Config::default());
    let clock = sim.node(sim::node::Config::default()).clock();
    connector_opcua::fuzz::certificate(clock, data);
});
