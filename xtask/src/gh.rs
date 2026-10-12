//! Reads the GitHub API through the `gh` command.

use std::process::Command;

use serde_json::Value;

/// Each object that `gh api --paginate` returns for `path`: the items of each page
/// of a list, or the one object.
pub(crate) fn api(path: &str) -> Result<Vec<Value>, String> {
    let output = Command::new("gh")
        .args(["api", "--paginate", path])
        .output()
        .map_err(|e| format!("gh api {path}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "gh api {path}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    items(&output.stdout).map_err(|e| format!("gh api {path}: {e}"))
}

/// The items of each JSON array in `pages`, and each object that is not in an array.
fn items(pages: &[u8]) -> Result<Vec<Value>, serde_json::Error> {
    let mut items = Vec::new();
    for page in serde_json::Deserializer::from_slice(pages).into_iter() {
        match page? {
            Value::Array(page) => items.extend(page),
            object => items.push(object),
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_items_of_each_page_and_a_single_object() {
        assert_eq!(
            items(b"[{\"a\":1},{\"a\":2}]\n[{\"a\":3}]").unwrap(),
            vec![json!({"a": 1}), json!({"a": 2}), json!({"a": 3})]
        );
        assert_eq!(items(b"{\"head\":{}}").unwrap(), vec![json!({"head": {}})]);
        assert_eq!(items(b"").unwrap(), Vec::<Value>::new());
        assert_eq!(
            items(b"[1,").unwrap_err().to_string(),
            "EOF while parsing a value at line 1 column 3"
        );
    }
}
