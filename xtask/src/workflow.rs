//! A reader of the lines of `.github/workflows/ci.yaml`: it refuses each node that can
//! run on into the next line, so that each check can read one line as one node.

/// The lines of the YAML `text`, with no blank or comment line, which YAML skips.
///
/// # Errors
///
/// When `text` has a character that is not printable ASCII or LF, or a line that `code`
/// refuses.
pub(crate) fn lines(text: &str) -> Result<Vec<&str>, String> {
    // YAML breaks a line at CR, NEL, LS, and PS too, and a parser can read other white
    // space as text, so `str::lines` and `str::trim` would drop a line that YAML reads.
    if let Some(c) = text.chars().find(|c| *c != '\n' && !matches!(c, ' '..='~')) {
        return Err(format!(
            "ci.yaml has a character that is not printable ASCII or LF: {c:?}"
        ));
    }
    let lines: Vec<&str> = (text.lines())
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .collect();
    // The text of a block scalar is not YAML.
    let mut block = None;
    for line in &lines {
        if block.is_some_and(|key| indent(line) > key) {
            continue;
        }
        let code = code(line)?;
        let header = code.trim_end_matches(['-', '+']);
        block = (header.ends_with(": |") || header.ends_with(": >"))
            .then(|| line.len() - line.trim_start_matches([' ', '-']).len());
    }
    Ok(lines)
}

/// The text of the YAML `line` before its comment.
///
/// # Errors
///
/// When a quoted scalar or a flow sequence of `line` does not close on it, a flow
/// sequence holds more than plain words, or `line` has a flow mapping, an anchor,
/// alias, or tag, a `?` or `:` at the start of a node, a `#` after a character that is
/// not a space, or a document marker, any of which can make a node run on.
pub(crate) fn code(line: &str) -> Result<&str, String> {
    let refused = || Err(format!("ci.yaml has a node that can run on: `{line}`"));
    if line.starts_with("---") || line.starts_with("...") {
        return refused();
    }
    // `start` holds where a node can start, the only place where a quote or a bracket
    // opens one.
    let (mut quote, mut escaped, mut flow, mut start) = (None, false, false, true);
    let mut prev = ' ';
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if let Some(open) = quote {
            let doubled = chars.peek().is_some_and(|(_, next)| *next == '\'');
            if escaped {
                escaped = false;
            } else if (open, c) == ('"', '\\')
                || (open, c, doubled) == ('\'', '\'', true)
            {
                escaped = true;
            } else if c == open {
                (quote, start) = (None, false);
            }
            prev = c;
            continue;
        }
        // A flow of plain words is the only flow ci.yaml needs, and the only one whose
        // end each parser finds at the same place.
        if flow {
            match c {
                ']' => (flow, start) = (false, false),
                ' ' | ',' | '-' | '_' | '.' | '/' => {}
                _ if c.is_ascii_alphanumeric() => {}
                _ => return refused(),
            }
            continue;
        }
        match c {
            ' ' if prev == ':' => start = true,
            // A `-` keeps a node start only before a space.
            '-' if chars.peek().is_some_and(|(_, next)| *next == ' ') => {}
            ' ' => {}
            '#' if prev == ' ' => return Ok(line[..i].trim_end()),
            // Parsers differ on whether such a `#` starts a comment.
            '#' => return refused(),
            '\'' | '"' if start => quote = Some(c),
            '[' if start => flow = true,
            '{' | '&' | '*' | '!' | '?' | ':' if start => return refused(),
            _ => start = false,
        }
        prev = c;
    }
    if quote.is_some() || flow {
        return refused();
    }
    Ok(line)
}

/// The spaces before the text of `line`.
pub(crate) fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The lines under the first line of `lines` that is `key`: each line after it up to
/// the first that is not indented further, which is the value of `key` in YAML.
///
/// # Errors
///
/// When no line is `key`.
pub(crate) fn under<'a>(lines: &[&'a str], key: &str) -> Result<Vec<&'a str>, String> {
    let start = (lines.iter().position(|line| *line == key))
        .ok_or_else(|| format!("ci.yaml has no `{}`", key.trim()))?;
    Ok((lines[start + 1..].iter().copied())
        .take_while(|line| indent(line) > indent(key))
        .collect())
}
