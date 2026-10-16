//! The hosts that [`Net::resolve`](super::Net::resolve) reads.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

/// A host that [`Net::resolve`](super::Net::resolve) reads: an IP address or a DNS
/// name. Parse one with [`str::parse`]. This grammar alone decides which text is an
/// address and which is a name, on every driver.
///
/// - An IPv4 address in four decimal parts, with no leading zeros.
/// - An IPv6 address, bare or in brackets, with an optional zone of a decimal
///   interface number that fits in `u32`: `fe80::1%2` or `[fe80::1%2]`.
/// - A name: parts of ASCII letters, digits, `-`, and `_`, between dots. Each part is
///   1 to 63 bytes and does not start or end with `-`. The name is at most 253 bytes
///   without one optional final dot, and its last part is not a number. Letters go to
///   lowercase.
///
/// ```
/// let host: env::net::host::Host = "Historian.local".parse().expect("a name");
/// assert_eq!(host.to_string(), "historian.local");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Host(pub(super) Kind);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Kind {
    Ip(IpAddr),
    Scoped(Ipv6Addr, u32),
    Name(Box<str>),
}

impl Host {
    /// The name, in lowercase with its final dot if it had one, or `None` for an IP
    /// address.
    ///
    /// ```
    /// let name: env::net::host::Host = "plc.local.".parse().expect("a name");
    /// assert_eq!(name.name(), Some("plc.local."));
    /// let ip: env::net::host::Host = "10.0.0.2".parse().expect("an address");
    /// assert_eq!(ip.name(), None);
    /// ```
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match &self.0 {
            Kind::Name(name) => Some(name),
            Kind::Ip(_) | Kind::Scoped(..) => None,
        }
    }
}

impl FromStr for Host {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        if text.is_empty() {
            return Err(Error::Empty);
        }
        let bracketed = text.strip_prefix('[').and_then(|t| t.strip_suffix(']'));
        if let Some(v6) = bracketed.or_else(|| text.contains(':').then_some(text)) {
            return ipv6(v6).map(Self);
        }
        if let Ok(v4) = text.parse::<Ipv4Addr>() {
            return Ok(Self(Kind::Ip(v4.into())));
        }
        name(text).map(Self)
    }
}

/// Shows an IPv4 address or a name as parsed, and an IPv6 address with no brackets,
/// with `%` and its zone when it has one.
impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::Ip(ip) => write!(f, "{ip}"),
            Kind::Scoped(ip, zone) => write!(f, "{ip}%{zone}"),
            Kind::Name(name) => f.write_str(name),
        }
    }
}

/// Why a text is not a [`Host`]. Its `Display` is a phrase that follows "has", for a
/// message such as `the address "pl c" has a host with a character other than ...`.
///
/// ```
/// let refused = "127.1".parse::<env::net::host::Host>();
/// assert_eq!(refused, Err(env::net::host::Error::Number));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The text is empty.
    Empty,
    /// The text has a `:` or is in brackets, but is not an IPv6 address.
    Ipv6,
    /// The zone after `%` is not a decimal number that fits in `u32`.
    Zone,
    /// A name holds a character other than an ASCII letter, a digit, `-`, `.`, or
    /// `_`.
    Character,
    /// A part between dots is empty, longer than 63 bytes, or starts or ends with
    /// `-`.
    Part,
    /// A name is longer than 253 bytes without its final dot.
    Long,
    /// The last part is a number, but the text is not a plain IPv4 address, as
    /// `127.1`, `0x7f.0.0.1`, or `10.0.0.2.`. A system resolver reads these as
    /// addresses in old forms.
    Number,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "no host",
            Self::Ipv6 => "a host with `:` or brackets that is not an IPv6 address",
            Self::Zone => "a zone that is not an interface number from 0 to 4294967295",
            Self::Character => {
                "a host with a character other than a letter, a digit, `-`, `.`, or `_`"
            }
            Self::Part => {
                "a host with a part between dots that is empty, longer than 63 \
                 characters, or starts or ends with `-`"
            }
            Self::Long => "a host longer than 253 characters",
            Self::Number => "a host with a number last that is not an IPv4 address",
        })
    }
}

impl std::error::Error for Error {}

/// An IPv6 address with an optional zone, as `fe80::1%2`.
fn ipv6(text: &str) -> Result<Kind, Error> {
    let (ip, zone) = match text.split_once('%') {
        Some((ip, zone)) => (ip, Some(zone)),
        None => (text, None),
    };
    let ip = ip.parse::<Ipv6Addr>().map_err(|_invalid| Error::Ipv6)?;
    let Some(zone) = zone else {
        return Ok(Kind::Ip(ip.into()));
    };
    // `u32::from_str` takes a leading `+`.
    if !zone.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Zone);
    }
    let zone = zone.parse().map_err(|_invalid| Error::Zone)?;
    Ok(Kind::Scoped(ip, zone))
}

/// A name that a resolver looks up, and never reads as an address.
fn name(text: &str) -> Result<Kind, Error> {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || b"-._".contains(&byte);
    if !text.bytes().all(allowed) {
        return Err(Error::Character);
    }
    let bare = text.strip_suffix('.').unwrap_or(text);
    let bad = |part: &str| {
        part.is_empty()
            || part.len() > 63
            || part.starts_with('-')
            || part.ends_with('-')
    };
    if bare.split('.').any(bad) {
        return Err(Error::Part);
    }
    if bare.len() > 253 {
        return Err(Error::Long);
    }
    let last = bare.rsplit('.').next().unwrap_or(bare);
    let number = match last.strip_prefix("0x").or_else(|| last.strip_prefix("0X")) {
        Some(digits) => digits.bytes().all(|byte| byte.is_ascii_hexdigit()),
        None => last.bytes().all(|byte| byte.is_ascii_digit()),
    };
    if number {
        return Err(Error::Number);
    }
    Ok(Kind::Name(text.to_ascii_lowercase().into()))
}

#[cfg(test)]
mod tests {
    use proptest::strategy::Strategy;

    use super::*;

    fn parse(text: &str) -> Result<Host, Error> {
        text.parse()
    }

    #[test]
    fn reads_each_form_and_shows_it() {
        for (text, shown) in [
            ("10.0.0.2", "10.0.0.2"),
            ("0.0.0.0", "0.0.0.0"),
            ("fd00::1", "fd00::1"),
            ("[fd00::1]", "fd00::1"),
            ("[FD00:0::1]", "fd00::1"),
            ("::ffff:10.0.0.1", "::ffff:10.0.0.1"),
            ("fe80::1%2", "fe80::1%2"),
            ("[fe80::1%2]", "fe80::1%2"),
            ("fd00::1%1", "fd00::1%1"),
            ("fe80::1%0", "fe80::1%0"),
            ("fe80::1%4294967295", "fe80::1%4294967295"),
            ("fe80::1%007", "fe80::1%7"),
            ("plc", "plc"),
            ("PLC.Local", "plc.local"),
            ("plc.local.", "plc.local."),
            ("plc_1.site-a", "plc_1.site-a"),
            ("_modbus._tcp.plc", "_modbus._tcp.plc"),
            ("10.0.0.plc", "10.0.0.plc"),
            ("1e5", "1e5"),
            ("0x1g", "0x1g"),
            ("123abc.local", "123abc.local"),
        ] {
            let host = parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!(host.to_string(), shown, "{text:?}");
        }
    }

    #[test]
    fn refuses_each_text_outside_the_grammar() {
        for (text, error) in [
            ("", Error::Empty),
            ("[]", Error::Ipv6),
            ("[plc]", Error::Ipv6),
            ("[10.0.0.2]", Error::Ipv6),
            ("[fd00::1", Error::Ipv6),
            ("[[fd00::1]]", Error::Ipv6),
            ("fd00::1]", Error::Ipv6),
            ("plc:502", Error::Ipv6),
            ("fd00::zz", Error::Ipv6),
            ("plc%eth0", Error::Character),
            ("10.0.0.2%1", Error::Character),
            ("fe80::1%", Error::Zone),
            ("fe80::1%eth0", Error::Zone),
            ("[fe80::1%lo]", Error::Zone),
            ("fd00::1%eth0", Error::Zone),
            ("fe80::1%4294967296", Error::Zone),
            ("fe80::1%+1", Error::Zone),
            ("fe80::1%-1", Error::Zone),
            ("fe80::1%1%2", Error::Zone),
            ("pl c", Error::Character),
            ("plc/x", Error::Character),
            ("plc\0", Error::Character),
            ("pläc", Error::Character),
            ("plc]", Error::Character),
            (".", Error::Part),
            ("..", Error::Part),
            (".plc", Error::Part),
            ("plc..local", Error::Part),
            ("plc.local..", Error::Part),
            ("-plc", Error::Part),
            ("plc-.local", Error::Part),
            ("127.1", Error::Number),
            ("0x7f.0.0.1", Error::Number),
            ("0x7f000001", Error::Number),
            ("0X7F", Error::Number),
            ("0x", Error::Number),
            ("2130706433", Error::Number),
            ("010.0.0.1", Error::Number),
            ("10.0.0.2.", Error::Number),
            ("10.0.0.256", Error::Number),
            ("plc.10", Error::Number),
        ] {
            assert_eq!(parse(text), Err(error), "{text:?}");
        }
    }

    #[test]
    fn bounds_a_part_at_63_bytes() {
        let part = "a".repeat(63);
        assert_eq!(parse(&part).map(|h| h.to_string()), Ok(part.clone()));
        assert_eq!(parse(&format!("{part}a")), Err(Error::Part));
    }

    #[test]
    fn bounds_a_name_at_253_bytes_without_its_final_dot() {
        let part = "a".repeat(63);
        let longest = format!("{part}.{part}.{part}.{}", "a".repeat(61));
        assert_eq!(longest.len(), 253);
        let shown = |text: &str| parse(text).map(|host| host.to_string());
        assert_eq!(shown(&longest), Ok(longest.clone()));
        assert_eq!(shown(&format!("{longest}.")), Ok(format!("{longest}.")));
        assert_eq!(parse(&format!("{longest}a")), Err(Error::Long));
        assert_eq!(parse(&format!("{longest}a.")), Err(Error::Long));
    }

    #[test]
    fn gives_the_name_of_a_name_only() {
        assert_eq!(parse("Plc.Local.").unwrap().name(), Some("plc.local."));
        assert_eq!(parse("10.0.0.2").unwrap().name(), None);
        assert_eq!(parse("fd00::1").unwrap().name(), None);
        assert_eq!(parse("fe80::1%2").unwrap().name(), None);
    }

    #[test]
    fn shows_each_error_as_a_phrase() {
        for (error, phrase) in [
            (Error::Empty, "no host"),
            (
                Error::Ipv6,
                "a host with `:` or brackets that is not an IPv6 address",
            ),
            (
                Error::Zone,
                "a zone that is not an interface number from 0 to 4294967295",
            ),
            (
                Error::Character,
                "a host with a character other than a letter, a digit, `-`, `.`, or \
                 `_`",
            ),
            (
                Error::Part,
                "a host with a part between dots that is empty, longer than 63 \
                 characters, or starts or ends with `-`",
            ),
            (Error::Long, "a host longer than 253 characters"),
            (
                Error::Number,
                "a host with a number last that is not an IPv4 address",
            ),
        ] {
            assert_eq!(error.to_string(), phrase);
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: if cfg!(miri) { 16 } else { 256 },
            failure_persistence: None,
            ..proptest::test_runner::Config::default()
        })]

        #[test]
        fn a_host_reads_back_from_its_display(
            text in proptest::prop_oneof![
                proptest::arbitrary::any::<Ipv4Addr>().prop_map(|ip| ip.to_string()),
                proptest::arbitrary::any::<Ipv6Addr>().prop_map(|ip| format!("[{ip}]")),
                (proptest::arbitrary::any::<Ipv6Addr>(), proptest::arbitrary::any::<u32>())
                    .prop_map(|(ip, zone)| format!("{ip}%{zone}")),
                "([A-Za-z0-9_]{1,8}\\.){0,3}[A-Za-z_][A-Za-z0-9_]{0,8}\\.?",
            ],
        ) {
            let host = parse(&text);
            proptest::prop_assert!(host.is_ok(), "{text:?}: {host:?}");
            let host = host.unwrap();
            proptest::prop_assert_eq!(parse(&host.to_string()), Ok(host));
        }

        #[test]
        fn any_host_that_parses_reads_back_from_its_display(
            text in "[a-f0-9:.%\\[\\]xX_-]{0,24}",
        ) {
            if let Ok(host) = parse(&text) {
                proptest::prop_assert_eq!(parse(&host.to_string()), Ok(host));
            }
        }
    }
}
