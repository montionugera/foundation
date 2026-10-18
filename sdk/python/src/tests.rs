use std::env;
use std::ffi::CStr;
use std::path::Path;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::wrap_pymodule;

const SUITE: &CStr = c"
import io, sys, unittest
sys.modules['foundation._native'] = native
sys.path.insert(0, str(package))
stream = io.StringIO()
suite = unittest.defaultTestLoader.discover(tests, pattern)
result = unittest.TextTestRunner(stream, verbosity=2).run(suite)
passed = result.wasSuccessful() and result.testsRun > 0
report = stream.getvalue()
";

/// The files of `sdk/python/tests`. `test_sim.py` runs only with the feature `sim`.
const FILES: [&str; 2] = ["test_client.py", "test_sim.py"];

/// Runs each file of the Python suite `sdk/python/tests` on its own in this process,
/// with this build as `foundation._native`. `FOUNDATION_NODE` is the path of the
/// `foundation` binary that the suite starts. Panics with the report when a Python
/// test fails or a file runs none, when the `test_*` files of the directory are
/// not `FILES`, and when `FOUNDATION_NODE` is not set.
#[test]
fn the_python_suite_passes() {
    #[expect(clippy::disallowed_methods, reason = "the suite reads the node's path")]
    let node = env::var_os("FOUNDATION_NODE");
    assert!(
        node.is_some(),
        "FOUNDATION_NODE is not set: give it the path of the foundation binary"
    );
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut found: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("test_"))
        .collect();
    found.sort();
    assert_eq!(
        found, FILES,
        "change `FILES` to the test files of the suite"
    );
    let files = if cfg!(feature = "sim") {
        &FILES[..]
    } else {
        &FILES[..1]
    };
    for file in files {
        let (passed, report) = suite(file);
        assert!(passed, "{file}: {report}");
    }
}

#[test]
fn a_pattern_that_matches_no_file_fails() {
    let (passed, report) = suite("test_none_*.py");
    assert!(!passed, "{report}");
    assert!(report.contains("Ran 0 tests"), "{report}");
}

/// Runs each test of `sdk/python/tests` whose file matches `pattern`, and gives
/// whether one ran and each passed, and the report.
fn suite(pattern: &str) -> (bool, String) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    Python::initialize();
    Python::attach(|py| -> PyResult<(bool, String)> {
        let globals = PyDict::new(py);
        globals.set_item("native", wrap_pymodule!(super::_native)(py))?;
        globals.set_item("package", root.join("python"))?;
        globals.set_item("tests", root.join("tests"))?;
        globals.set_item("pattern", pattern)?;
        py.run(SUITE, Some(&globals), None)?;
        Ok((
            globals.as_any().get_item("passed")?.extract()?,
            globals.as_any().get_item("report")?.extract()?,
        ))
    })
    .unwrap_or_else(|e| panic!("the suite did not run: {e}"))
}

/// Maps each variant to its class and text. No test of the suite reaches `Bind`: the
/// bind of an ephemeral port fails only when the process has no free file descriptor,
/// and the count of free ones changes with the process.
#[test]
fn error_raises_the_class_and_text_of_each_variant() {
    let cases = [
        (
            sdk::Error::Thread {
                reason: "the test".into(),
            },
            "RuntimeError",
            "cannot start the client thread: the test",
        ),
        (
            sdk::Error::Bind(transport::Error::TimedOut),
            "OSError",
            "cannot bind the local UDP socket: the peer stopped answering",
        ),
        (
            sdk::Error::Session(program::Error::Unanswered),
            "ConnectionError",
            "the node finished a stream with no answer",
        ),
        (
            sdk::Error::Panicked,
            "PanicException",
            "the client thread panicked",
        ),
    ];
    Python::initialize();
    Python::attach(|py| {
        for (variant, class, text) in cases {
            let error = super::error(&variant);
            assert_eq!(error.get_type(py).name().unwrap().to_string(), class);
            assert_eq!(error.value(py).to_string(), text);
        }
    });
}
