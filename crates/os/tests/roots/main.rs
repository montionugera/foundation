//! `os::roots::read` reads the CA certificates of the OS store, or of the PEM file
//! that `SSL_CERT_FILE` names and the directories that `SSL_CERT_DIR` names. This
//! binary has no test harness: each case runs it again as a child with its own
//! environment, since a set of the environment races the threads of a harness. With
//! the argument `child`, it writes what `os::roots::read` gives: a line for its
//! source, then one for each certificate and for each failure.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

fn main() {
    #[expect(clippy::disallowed_methods, reason = "the child reads its role")]
    let role = std::env::args_os().nth(1);
    if role.is_some_and(|arg| arg == "child") {
        let mut output = std::io::stdout();
        write!(output, "{}", lines(&os::roots::read())).expect("a write to the test");
        return;
    }
    reads_the_certificates_of_the_pem_file_of_ssl_cert_file();
    gives_the_failure_of_a_pem_file_that_is_not_there();
    reads_the_certificates_of_the_os_store();
    reads_the_os_store_when_ssl_cert_dir_holds_no_path();
    reads_only_the_files_of_the_directory_of_ssl_cert_dir();
    gives_only_the_failure_of_a_path_that_is_not_there_or_no_directory();
    gives_the_failures_of_both_paths_when_both_are_set();
}

fn reads_the_certificates_of_the_pem_file_of_ssl_cert_file() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roots/ca.pem");
    let der = include_bytes!("ca.der").to_vec();
    assert_eq!(
        child(&[("SSL_CERT_FILE", file.as_os_str())]),
        lines(&os::roots::Roots {
            certificates: vec![der],
            failures: vec![],
            source: os::roots::Source::Paths {
                file: Some(file.clone()),
                dirs: vec![],
            },
        }),
        "the certificate of the PEM file"
    );
}

fn gives_the_failure_of_a_pem_file_that_is_not_there() {
    let file = Path::new("/nonexistent/roots.pem");
    assert_eq!(
        child(&[("SSL_CERT_FILE", file.as_os_str())]),
        "source SSL_CERT_FILE=/nonexistent/roots.pem\nfailure failed to read PEM from \
         file: No such file or directory (os error 2) at '/nonexistent/roots.pem'\n",
        "the failure of a missing file"
    );
}

fn reads_the_certificates_of_the_os_store() {
    let text = child(&[]);
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("source the OS store"),
        "the source: {text}"
    );
    assert!(
        lines.all(|line| line.starts_with("certificate ")),
        "the OS store gave a failure: {text}"
    );
    assert!(text.lines().count() > 1, "the OS store gave no certificate");
}

fn reads_the_os_store_when_ssl_cert_dir_holds_no_path() {
    // The keychain of macOS gives its certificates in a different order on each read.
    let sorted = |text: String| {
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        lines.sort_unstable();
        lines
    };
    let store = sorted(child(&[]));
    for dirs in ["", ":"] {
        assert_eq!(
            sorted(child(&[("SSL_CERT_DIR", OsStr::new(dirs))])),
            store,
            "the roots with SSL_CERT_DIR={dirs:?}"
        );
    }
}

fn reads_only_the_files_of_the_directory_of_ssl_cert_dir() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roots");
    assert_eq!(
        child(&[("SSL_CERT_DIR", dir.as_os_str())]),
        lines(&os::roots::Roots {
            certificates: vec![include_bytes!("ca.der").to_vec()],
            failures: vec![],
            source: os::roots::Source::Paths {
                file: None,
                dirs: vec![dir.clone()],
            },
        }),
        "the certificate of the directory"
    );
}

fn gives_only_the_failure_of_a_path_that_is_not_there_or_no_directory() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roots/ca.pem");
    let cases = [
        (
            "SSL_CERT_FILE",
            OsStr::new(""),
            "source SSL_CERT_FILE=\nfailure failed to read PEM from file: No such file or \
             directory (os error 2) at ''\n"
                .to_owned(),
        ),
        (
            "SSL_CERT_DIR",
            OsStr::new("/nonexistent"),
            "source SSL_CERT_DIR=/nonexistent\nfailure opening directory: No such file or \
             directory (os error 2) at '/nonexistent'\n"
                .to_owned(),
        ),
        (
            "SSL_CERT_DIR",
            file.as_os_str(),
            format!(
                "source SSL_CERT_DIR={0}\nfailure opening directory: Not a directory (os \
                 error 20) at '{0}'\n",
                file.display()
            ),
        ),
    ];
    for (var, value, failure) in cases {
        assert_eq!(
            child(&[(var, value)]),
            failure,
            "the roots with {var}={}",
            value.display()
        );
    }
}

fn gives_the_failures_of_both_paths_when_both_are_set() {
    assert_eq!(
        child(&[
            ("SSL_CERT_FILE", OsStr::new("/nonexistent/roots.pem")),
            ("SSL_CERT_DIR", OsStr::new("/nonexistent")),
        ]),
        "source SSL_CERT_FILE=/nonexistent/roots.pem SSL_CERT_DIR=/nonexistent\n\
         failure failed to read PEM from file: No such file or directory (os error 2) \
         at '/nonexistent/roots.pem'\nfailure opening directory: No such file or \
         directory (os error 2) at '/nonexistent'\n",
        "the roots with both set"
    );
}

/// The lines that the child writes for `roots`.
fn lines(roots: &os::roots::Roots) -> String {
    let mut text = format!("source {}\n", roots.source);
    for der in &roots.certificates {
        text.push_str("certificate ");
        for byte in der {
            write!(text, "{byte:02x}").expect("a write to a string");
        }
        text.push('\n');
    }
    for failure in &roots.failures {
        writeln!(text, "failure {failure}").expect("a write to a string");
    }
    text
}

/// What the child writes with `SSL_CERT_FILE` and `SSL_CERT_DIR` removed, and then
/// each of `vars` set.
fn child(vars: &[(&str, &OsStr)]) -> String {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .arg("child")
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR");
    command.envs(vars.iter().copied());
    let output = command.output().expect("the child runs");
    assert!(output.status.success(), "the child failed: {output:?}");
    String::from_utf8(output.stdout).expect("the child writes UTF-8")
}
