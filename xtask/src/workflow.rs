//! A reader of the lines of `.github/workflows/ci.yaml`: it refuses each node that can
//! run on into the next line, so that each check can read one line as one node.

/// A line of YAML: its text, and its code, the text before its comment, which a line
/// of the text of a block scalar does not have.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Line<'a> {
    pub(crate) text: &'a str,
    pub(crate) code: Option<&'a str>,
}

/// The lines of the YAML `text`, with no blank or comment line, which YAML skips.
///
/// # Errors
///
/// When `text` has a character that is not printable ASCII, LF, or the CR of a CR LF,
/// or a line outside the text of a block scalar that `code` refuses.
pub(crate) fn lines(text: &str) -> Result<Vec<Line<'_>>, String> {
    // YAML breaks a line at CR, NEL, LS, and PS too, and a parser can read other white
    // space as text, so `str::lines` and `str::trim` would drop a line that YAML reads.
    // A checkout on Windows gives CR LF.
    let mut chars = text.split("\r\n").flat_map(str::chars);
    if let Some(c) = chars.find(|c| *c != '\n' && !matches!(c, ' '..='~')) {
        return Err(format!(
            "ci.yaml has a character that is not printable ASCII, LF, or CR LF: {c:?}"
        ));
    }
    let lines: Vec<&str> = (text.lines())
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .collect();
    // The text of a block scalar is not YAML.
    let mut block = None;
    let mut read = Vec::with_capacity(lines.len());
    for text in lines {
        if block.is_some_and(|key| indent(text) > key) {
            read.push(Line { text, code: None });
            continue;
        }
        let code = code(text)?;
        block = opened(text, code)?;
        read.push(Line {
            text,
            code: Some(code),
        });
    }
    Ok(read)
}

/// The indent that each line of the text of a block scalar passes, when `code`, the
/// code of the YAML line `text`, ends in its header.
///
/// # Errors
///
/// When the header is the only node of `text`: its text can then start at its indent.
fn opened(text: &str, code: &str) -> Result<Option<usize>, String> {
    let Some(before) = (code.trim_end())
        .trim_end_matches(|c| matches!(c, '1'..='9' | '+' | '-'))
        .strip_suffix(['|', '>'])
    else {
        return Ok(None);
    };
    if before.trim().is_empty() {
        return Err(format!("ci.yaml has a node that can run on: `{text}`"));
    }
    let node = before.trim_end();
    Ok(if !before.ends_with(' ') {
        None
    } else if node.ends_with(':') {
        Some(text.len() - text.trim_start_matches([' ', '-']).len())
    } else if node.split_whitespace().all(|word| word == "-") {
        Some(node.len() - 1)
    } else {
        None
    })
}

/// The text of the YAML `line` before its comment.
///
/// # Errors
///
/// When a quoted scalar or a flow sequence of `line` does not close on it, a flow
/// sequence holds more than plain words, or `line` has a flow mapping, an anchor,
/// alias, or tag, a `?` or `:` at the start of a node, a `#` after a character that is
/// not a space, or a document marker, any of which can make a node run on.
fn code(line: &str) -> Result<&str, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(text: &str) -> Vec<&str> {
        lines(text)
            .unwrap()
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    fn refused(line: &str) -> Result<Vec<Line<'_>>, String> {
        Err(format!("ci.yaml has a node that can run on: `{line}`"))
    }

    #[test]
    fn lines_skips_each_blank_and_comment_line() {
        let text = "a:\n\n  \n  # x\n  b: 1 # c\n";
        assert_eq!(
            lines(text),
            Ok(vec![
                Line {
                    text: "a:",
                    code: Some("a:"),
                },
                Line {
                    text: "  b: 1 # c",
                    code: Some("  b: 1"),
                },
            ])
        );
        let printable: String = (' '..='~').collect();
        let text = format!("a: |\n  {printable}");
        assert_eq!(texts(&text), ["a: |", &format!("  {printable}")]);
    }

    #[test]
    fn lines_gives_no_code_to_the_text_of_a_block_scalar() {
        let text = concat!(
            "a: |\n",
            "  [ \"$A\" = b ] # c\n",
            "  - b: >-\n",
            "      echo \"a#b\"\n",
            "c: d\n",
        );
        let codes: Vec<_> = lines(text).unwrap().into_iter().map(|l| l.code).collect();
        assert_eq!(codes, [Some("a: |"), None, None, None, Some("c: d")]);
    }

    #[test]
    fn lines_gives_no_code_to_the_text_after_each_form_of_block_header() {
        for header in [
            "a: | ",
            "a: |2",
            "a: >1-",
            "a: |-2 # c",
            "a: > # c",
            "a:  |+",
            "a:\n- |",
            "a:\n- - >",
        ] {
            let text = format!("{header}\n    [ \"$A\" = b ]\nc: d\n");
            let codes: Vec<_> =
                lines(&text).unwrap().into_iter().map(|l| l.code).collect();
            assert_eq!(codes[codes.len() - 2..], [None, Some("c: d")], "{header:?}");
        }
    }

    #[test]
    fn lines_reads_the_text_of_a_block_in_a_sequence_from_one_column_past_its_dash() {
        for text in ["a:\n- |\n x\nc: d\n", "a:\n- - |\n   x\nc: d\n"] {
            let codes: Vec<_> =
                lines(text).unwrap().into_iter().map(|l| l.code).collect();
            assert_eq!(codes[2..], [None, Some("c: d")], "{text:?}");
        }
    }

    #[test]
    fn lines_reads_a_plain_scalar_that_ends_in_a_bar_as_code() {
        let text = "a: b - |\nc: d |\n";
        assert_eq!(texts(text), ["a: b - |", "c: d |"]);
        let codes: Vec<_> = lines(text).unwrap().into_iter().map(|l| l.code).collect();
        assert_eq!(codes, [Some("a: b - |"), Some("c: d |")]);
    }

    #[test]
    fn lines_refuses_a_block_header_on_a_line_of_its_own() {
        assert_eq!(lines("a:\n  |\n  b\n"), refused("  |"));
    }

    #[test]
    fn lines_keeps_each_node_that_closes_on_its_line() {
        let text = concat!(
            "a: 'b''c'\n",
            "b: \"c\\\"d\" # e\n",
            "c: it's a b # c\n",
            "d: [e, f-g, h_i.j/k] # it's\n",
            "e: \"!cancelled()\"\n",
            "f: |\n",
            "  [echo \"it's\n",
            "  - g: >-\n",
            "      [h\n",
            "i: 'j\\'\n",
            "a: b[c\n",
            "d: e,'f\n",
            "g: |+\n",
            "  'h\n",
            "[i]: j\n",
            "k: -'l\n",
        );
        assert_eq!(texts(text), text.lines().collect::<Vec<_>>());
    }

    #[test]
    fn lines_keeps_each_quoted_key() {
        let text = concat!(
            "env:\n",
            "  \"A\": b\n",
            "  'C': d # e\n",
            "steps:\n",
            "  - \"f\": |\n",
            "      g\n",
        );
        assert_eq!(texts(text), text.lines().collect::<Vec<_>>());
    }
    #[test]
    fn lines_refuses_each_node_that_can_run_on() {
        for line in [
            "    runs-on: \"${{ 'ubuntu-latest' || '",
            "      [{name: Miri, run: \"true",
            "  - [x, [y]",
            "          key: 'x",
            "      - name: 'Miri",
            "     note: \"",
            "a'b: 'c",
            "a: {\"b\":'c}",
            "a: \"b\\\"",
            "a: [b, # c",
            "a: &b 'c'",
            "a: !b c",
            "a: *b",
            "  ? a",
            "  : b: |",
            "a: [? b]",
            "a: [b:'c, [d']",
            "a: [b:'#, [c']",
            "a: [b, [c]]",
            "a: [b, c",
            "--- 'a",
            "... [a",
            "a: {b: c}",
            "a: 'b''c",
            "a: b#c",
            "a: [\"b\"#c]",
            "a: ['b'#]",
            "a: ['b',#]",
            "a: {'b':#}",
            "a: [b - 'c, [d']",
            "a: [b - '#, [d']",
            "a: [-'b, [c']",
        ] {
            let text = format!("x: |\n  y\nz:\n{line}\n");
            assert_eq!(lines(&text), refused(line), "{line}");
        }
        let text = "  - a: |\n      b\n    c: 'd\n";
        assert_eq!(lines(text), refused("    c: 'd"));
    }

    #[test]
    fn lines_reads_cr_lf_as_lf() {
        assert_eq!(
            lines("a:\r\n  b: |\r\n    c\r\n"),
            lines("a:\n  b: |\n    c\n")
        );
    }

    #[test]
    fn lines_refuses_each_character_that_is_not_printable_ascii_or_lf() {
        for c in [
            '\r', '\t', '\u{85}', '\u{a0}', '\u{2028}', '\u{2029}', '\u{7f}',
        ] {
            let text = format!("a:\r\n  #{c}  b:\n\r");
            assert_eq!(
                lines(&text),
                Err(format!(
                    "ci.yaml has a character that is not printable ASCII, LF, or CR \
                     LF: {c:?}"
                )),
            );
        }
    }
}
