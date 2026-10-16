use ::connector::import::{self, Fill};
use document::Document;
use document::diagnostic::{Code, Diagnostic};

use crate::walk::{self, Part};

const UNFILLED: Code = Code::new("config.unfilled");
const BAD_FILL: Code = Code::new("config.bad-fill");

/// `config.unfilled` at each `fill("<what>")` of `documents`, and `config.bad-fill` at
/// each other call `fill`, in any value at any depth. They are in the order of their
/// [`document::Source`], then in source order. A call `fill` inside a call `fill` gives
/// no problem of its own.
pub(crate) fn problems(documents: &[Document]) -> Vec<Diagnostic> {
    let mut problems = Vec::new();
    for document in documents {
        walk::each(document, &mut |part| {
            let Part::Value(value) = part else {
                return true;
            };
            let (code, message) = match import::unfilled(value) {
                None => return true,
                Some(Fill::Of(what)) => {
                    (UNFILLED, format!("the import found no value for {what}"))
                }
                Some(Fill::Bad) => (BAD_FILL, "`fill` takes one string".into()),
            };
            let fix = "Write the value in place of `fill`".into();
            problems.push(Diagnostic::new(code, value.span, message, fix));
            false
        });
    }
    crate::sort(&mut problems);
    problems
}
