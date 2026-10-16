//! What an import writes that a kind and `config` share: the channel names of the
//! points, and the value of a fact that the device does not give.

use document::value::{Call, Kind, Value};
use types::hash::Set;
use types::name::Name;

use crate::browse::Path;

/// `fill("<what>")`: the value of a fact that the device does not give, such as
/// `fill("the sample type")`. `plan` refuses it.
#[must_use]
pub fn fill(what: &str) -> Value {
    let what = Value {
        kind: Kind::String(what.into()),
        span: None,
    };
    let call = Call {
        function: "fill".into(),
        function_span: None,
        arguments: vec![what],
    };
    Value {
        kind: Kind::Call(call),
        span: None,
    }
}

/// The channel name of each pick, in the order of `picks`. A pick is the path of a
/// point and the name that the import gives it, if any. A given name is kept as it
/// is. Each other pick gets the default `<connector>.<path>`: each name of its path
/// below `start` is one segment, with `_` in place of each character other than an
/// ASCII letter, a digit, `_`, and `-`. A default that differs only in case, or not
/// at all, from a name of `taken`, a given name, or an earlier default gets the
/// first free suffix of `_2`, `_3`, and on. `None` for a default, with its suffix,
/// longer than [`Name::MAX_BYTES`]. `taken` holds each name that the files define
/// or imply, such as the status channels of a connector.
///
/// # Panics
///
/// When the path of a pick with no given name is not below `start`.
#[must_use]
pub fn names<'a>(
    connector: &Name,
    start: &Path,
    picks: &[(&Path, Option<&Name>)],
    taken: impl IntoIterator<Item = &'a Name>,
) -> Vec<Option<Name>> {
    let mut used: Set<String> = taken.into_iter().map(folded).collect();
    used.extend(picks.iter().filter_map(|(_, given)| given.map(folded)));
    picks
        .iter()
        .map(|(path, given)| match given {
            Some(name) => Some((*name).clone()),
            None => default(connector, start, path, &mut used),
        })
        .collect()
}

/// The default name of the point at `path`, which it adds to `used`.
fn default(
    connector: &Name,
    start: &Path,
    path: &Path,
    used: &mut Set<String>,
) -> Option<Name> {
    let mut names = path.names();
    let below = start.names().all(|name| names.next() == Some(name)) && names.len() > 0;
    assert!(below, "the path {path:?} is not below the start {start:?}");
    let mut base = connector.as_str().to_owned();
    for name in names {
        base.push('.');
        base.extend(name.chars().map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        }));
    }
    let (mut text, mut suffix) = (base.clone(), 1);
    while used.contains(&text.to_ascii_lowercase()) {
        suffix += 1;
        text = format!("{base}_{suffix}");
    }
    if text.len() > Name::MAX_BYTES {
        return None;
    }
    used.insert(text.to_ascii_lowercase());
    Some(
        text.parse()
            .expect("invariant: each segment of a default is valid"),
    )
}

/// `name` in lower case, so that two names that differ only in case are one.
fn folded(name: &Name) -> String {
    name.as_str().to_ascii_lowercase()
}

#[cfg(test)]
mod tests;
