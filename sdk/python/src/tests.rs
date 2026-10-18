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

/// Runs the Python suite `sdk/python/tests` in this process, with this build as
/// `foundation._native`: each `test_*.py` with the feature `sim`, else
/// `test_client.py`. `FOUNDATION_NODE` is the path of the `foundation` binary that the
/// suite starts. Panics with the report when a Python test fails or none runs, and
/// when `FOUNDATION_NODE` is not set.
#[test]
fn the_python_suite_passes() {
    #[expect(clippy::disallowed_methods, reason = "the suite reads the node's path")]
    let node = env::var_os("FOUNDATION_NODE");
    assert!(
        node.is_some(),
        "FOUNDATION_NODE is not set: give it the path of the foundation binary"
    );
    let pattern = if cfg!(feature = "sim") {
        "test_*.py"
    } else {
        "test_client.py"
    };
    let (passed, report) = suite(pattern);
    assert!(passed, "{report}");
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

/// Maps `Bind`, which no test of the suite reaches: the bind of an ephemeral port fails
/// only when the process has no free file descriptor, and the count of free ones
/// changes with the process.
#[test]
fn bind_raises_os_error() {
    Python::initialize();
    Python::attach(|py| {
        let error = super::error(&sdk::Error::Bind(transport::Error::TimedOut));
        assert_eq!(error.get_type(py).name().unwrap().to_string(), "OSError");
        assert_eq!(
            error.value(py).to_string(),
            "cannot bind the local UDP socket: the peer stopped answering"
        );
    });
}
