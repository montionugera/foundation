use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use document::diagnostic::Code;
use env::paths::{self, Found, Paths};
use serde_json::json;

use super::{Shown, apply, most, plan};
use crate::common::{create_node, name, placed_site, solo};
use crate::error::{Error, Problem, Unread};
use crate::front_end::File;
use crate::operation::DATA;
use crate::{Failure, Node, Run, serve};

/// A failure of `reach`, or of a request.
fn lost() -> Failure {
    Failure {
        code: Code::new("node.not-running"),
        message: "no node runs in foundation-data".to_owned(),
        fix: "Start one".to_owned(),
    }
}

/// The paths of a host with `site.hcl`, which [`placed_site`] gives.
fn with_site(host: &sim::node::Node) -> Paths {
    let paths = host.paths();
    paths
        .write(Path::new("site.hcl"), placed_site().as_bytes())
        .expect("a write");
    paths
}

/// What a `reach` of [`Remote`] does.
enum Mode {
    /// Serves each request on the node as `@admin`.
    Serve,
    /// Gives this reply to each request.
    Reply(Vec<u8>),
    /// Fails with [`lost`].
    Unreached,
    /// Gives a request that fails with [`lost`].
    Unsent,
}

/// Runs `plan`, `apply`, and the command line with a `reach` to `node`, and counts
/// what each call of `reach` saw.
struct Remote {
    node: Node,
    mode: Mode,
    /// The data directory of each call of `reach`.
    reached: RefCell<Vec<PathBuf>>,
    requests: Cell<usize>,
}

impl Remote {
    fn new(node: Node) -> Self {
        Self::with(node, Mode::Serve)
    }

    fn with(node: Node, mode: Mode) -> Self {
        Self {
            node,
            mode,
            reached: RefCell::new(Vec::new()),
            requests: Cell::new(0),
        }
    }

    async fn send(&self, body: &[u8]) -> Result<Vec<u8>, Failure> {
        self.requests.set(self.requests.get() + 1);
        match &self.mode {
            Mode::Serve => Ok(serve::serve(&self.node, &name("@admin"), body).await),
            Mode::Reply(reply) => Ok(reply.clone()),
            Mode::Unreached | Mode::Unsent => Err(lost()),
        }
    }

    fn reached(&self, data: &Path) -> Result<(), Failure> {
        self.reached.borrow_mut().push(data.to_owned());
        match self.mode {
            Mode::Unreached => Err(lost()),
            Mode::Serve | Mode::Reply(_) | Mode::Unsent => Ok(()),
        }
    }

    async fn plan(
        &self,
        paths: &Paths,
        files: &[PathBuf],
        out: Option<&Path>,
    ) -> Result<Shown, Error> {
        let reach = async |data: &Path| {
            self.reached(data)?;
            Ok(async |body: &[u8]| self.send(body).await)
        };
        plan(files, out, paths, reach).await
    }

    async fn apply(&self, paths: &Paths, file: &Path) -> Result<Shown, Error> {
        let reach = async |data: &Path| {
            self.reached(data)?;
            Ok(async |body: &[u8]| self.send(body).await)
        };
        apply(file, paths, reach).await
    }

    /// The run, output, and errors of `foundation <args>`.
    async fn cli(&self, paths: &Paths, args: &[&str]) -> (Run, String, String) {
        let reach = async |data: &Path| {
            self.reached(data)?;
            Ok(async |body: &[u8]| self.send(body).await)
        };
        let args = ["foundation"].iter().chain(args).map(Into::into);
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let input = std::io::empty();
        let run = crate::cli(args, input, &mut output, &mut errors, paths, reach).await;
        let text = |bytes| String::from_utf8(bytes).expect("UTF-8");
        (run, text(output), text(errors))
    }

    /// The count of calls of `reach`, and of requests.
    fn calls(&self) -> (usize, usize) {
        (self.reached.borrow().len(), self.requests.get())
    }
}

/// The code and message of the one problem of `error`, and its status.
fn shown(error: &Error) -> (String, String, u8) {
    let problems = error.problems();
    let [problem] = &problems[..] else {
        panic!("one problem: {problems:?}");
    };
    (
        problem.code.clone(),
        problem.message.clone(),
        error.status(),
    )
}

fn read(code: &str, message: &str, status: u8) -> (String, String, u8) {
    (code.to_owned(), message.to_owned(), status)
}

#[test]
fn plans_writes_the_plan_file_and_applies_it() {
    solo(|host, mesh| async move {
        let paths = with_site(&host);
        let remote = Remote::new(create_node(mesh.clone()));
        let files = vec![(PathBuf::from("site.hcl"), placed_site())];
        let (expected, output) = remote.node.plan(files).await.expect("a plan");

        let out = Path::new("plans/site.plan");
        let files = [PathBuf::from("site.hcl")];
        let planned = remote.plan(&paths, &files, Some(out)).await;
        let Ok(Shown::Plan(planned)) = planned else {
            panic!("a plan");
        };
        assert_eq!(planned.json(), output);
        assert_eq!(paths.read(out, most()), Ok(Found::File(expected)));
        assert_eq!(remote.calls(), (1, 1));

        let applied = remote.apply(&paths, out).await;
        let Ok(Shown::Apply(applied)) = applied else {
            panic!("an apply");
        };
        assert_eq!(applied.file, "plans/site.plan");
        assert_eq!(applied.effect.counts.added, 3);
        assert_eq!(mesh.pointer().version, 1);
        let data = PathBuf::from(DATA);
        assert_eq!(*remote.reached.borrow(), [data.clone(), data]);
        assert_eq!(remote.requests.get(), 2);
    });
}

#[test]
fn plan_with_no_out_writes_no_file() {
    solo(|host, mesh| async move {
        let paths = with_site(&host);
        let remote = Remote::new(create_node(mesh));
        let before = paths.read(Path::new("/"), most());
        let files = [PathBuf::from("site.hcl")];
        let planned = remote.plan(&paths, &files, None).await;
        assert!(matches!(planned, Ok(Shown::Plan(_))));
        assert_eq!(paths.read(Path::new("/"), most()), before);
        assert_eq!(remote.calls(), (1, 1));
    });
}

#[test]
fn plan_gives_each_failed_read_by_its_cause_and_reaches_no_node() {
    solo(|host, mesh| async move {
        let paths = with_site(&host);
        paths
            .write(Path::new("config/a.hcl"), b"")
            .expect("a write");
        paths
            .write(Path::new("latin.hcl"), b"\xff")
            .expect("a write");
        host.fail_path(Path::new("site.hcl"), 5);
        let remote = Remote::new(create_node(mesh));
        let cases = [
            ("gone.hcl", Unread::NotFound, "no file is at gone.hcl", 2),
            (
                "config",
                Unread::Directory,
                "config is a directory, not a file",
                2,
            ),
            (
                "latin.hcl",
                Unread::NotUtf8,
                "the text of latin.hcl is not UTF-8",
                2,
            ),
            (
                "site.hcl",
                Unread::Io { code: 5 },
                "the read of site.hcl failed with OS error 5",
                1,
            ),
        ];
        for (path, cause, message, status) in cases {
            let files = [PathBuf::from(path)];
            let planned = remote.plan(&paths, &files, None).await;
            let Err(error) = planned else {
                panic!("{path} does not read");
            };
            let path = PathBuf::from(path);
            assert_eq!(error, Error::Read { path, cause });
            assert_eq!(shown(&error), read("ops.read", message, status));
        }
        assert_eq!(remote.calls(), (0, 0));
    });
}

#[test]
fn plan_of_a_request_one_byte_over_the_most_reaches_no_node() {
    solo(|host, mesh| async move {
        let paths = host.paths();
        let empty = [File {
            path: PathBuf::from("big.hcl"),
            text: String::new(),
        }];
        let around = serve::plan_request(&empty).len();
        let text = "#".repeat(most() - around + 1);
        paths
            .write(Path::new("big.hcl"), text.as_bytes())
            .expect("a write");
        let remote = Remote::new(create_node(mesh));
        let files = [PathBuf::from("big.hcl")];
        let planned = remote.plan(&paths, &files, None).await;
        let Err(error) = planned else {
            panic!("a request one byte over");
        };
        assert_eq!(error, Error::LargeRequest);
        let message = format!(
            "the request has more than {} bytes, the most that a request holds",
            most()
        );
        assert_eq!(shown(&error), read("ops.large-request", &message, 2));
        assert_eq!(remote.calls(), (0, 0));

        let text = "#".repeat(most() - around);
        paths
            .write(Path::new("big.hcl"), text.as_bytes())
            .expect("a write");
        let remote = Remote::with(remote.node, Mode::Unsent);
        let planned = remote.plan(&paths, &files, None).await;
        assert_eq!(
            planned.err(),
            Some(Error::Node(lost())),
            "a request of the most"
        );
    });
}

/// `b.hcl` is not UTF-8, so [`Unread::NotUtf8`] shows that `plan` checked its text.
#[test]
fn plan_gives_large_request_once_the_files_that_it_read_pass_the_most() {
    solo(|host, mesh| async move {
        let paths = host.paths();
        let half = most() / 2 + 1;
        let b = vec![0xff; half];
        paths.write(Path::new("b.hcl"), &b).expect("a write");
        let remote = Remote::new(create_node(mesh));
        let files = [PathBuf::from("a.hcl"), PathBuf::from("b.hcl")];
        let a = "#".repeat(half);
        paths
            .write(Path::new("a.hcl"), a.as_bytes())
            .expect("a write");
        let planned = remote.plan(&paths, &files, None).await;
        assert_eq!(
            planned.err(),
            Some(Error::LargeRequest),
            "over the most in all"
        );

        paths.write(Path::new("a.hcl"), b"##").expect("a write");
        let planned = remote.plan(&paths, &files, None).await;
        let path = PathBuf::from("b.hcl");
        let cause = Unread::NotUtf8;
        assert_eq!(
            planned.err(),
            Some(Error::Read { path, cause }),
            "within it"
        );
        assert_eq!(remote.calls(), (0, 0));
    });
}

/// `a.hcl` alone fits a request, and leaves fewer bytes than the entry paths of
/// `config` take.
#[test]
fn plan_of_a_directory_after_a_large_file_gives_read() {
    solo(|host, mesh| async move {
        let paths = host.paths();
        for n in 0..10 {
            let path = PathBuf::from(format!("config/f{n}.hcl"));
            paths.write(&path, b"").expect("a write");
        }
        let a = "#".repeat(most() - 100);
        paths
            .write(Path::new("a.hcl"), a.as_bytes())
            .expect("a write");
        let remote = Remote::with(create_node(mesh), Mode::Unsent);
        let alone = remote.plan(&paths, &[PathBuf::from("a.hcl")], None).await;
        assert_eq!(alone.err(), Some(Error::Node(lost())), "a.hcl fits");
        let files = [PathBuf::from("a.hcl"), PathBuf::from("config")];
        let planned = remote.plan(&paths, &files, None).await;
        let path = PathBuf::from("config");
        let cause = Unread::Directory;
        assert_eq!(planned.err(), Some(Error::Read { path, cause }));
    });
}

#[test]
fn plan_whose_write_of_out_fails_gives_write_and_leaves_no_file() {
    solo(|host, mesh| async move {
        let paths = with_site(&host);
        let out = Path::new("site.plan");
        host.fail_path(out, 13);
        let remote = Remote::new(create_node(mesh));
        let files = [PathBuf::from("site.hcl")];
        let planned = remote.plan(&paths, &files, Some(out)).await;
        let Err(error) = planned else {
            panic!("the write fails");
        };
        let expected = Error::Write {
            path: out.to_owned(),
            error: paths::Error::Io { code: 13 },
        };
        assert_eq!(error, expected);
        let message = "the write of the plan file site.plan failed with OS error 13";
        assert_eq!(shown(&error), read("ops.write", message, 1));
        assert_eq!(paths.read(out, most()), Err(paths::Error::NotFound));
    });
}

#[test]
fn write_of_a_plan_file_whose_directory_is_not_there_names_the_directory() {
    let error = Error::Write {
        path: PathBuf::from("plans/site.plan"),
        error: paths::Error::NotFound,
    };
    let message = "the directory of the plan file plans/site.plan is not there";
    assert_eq!(shown(&error), read("ops.write", message, 1));
}

#[test]
fn a_failure_of_reach_or_of_the_request_is_the_error_of_the_command() {
    solo(|host, mesh| async move {
        let paths = with_site(&host);
        let files = [PathBuf::from("site.hcl")];
        let remote = Remote::with(create_node(mesh), Mode::Unreached);
        let planned = remote.plan(&paths, &files, None).await;
        let Err(error) = planned else {
            panic!("reach fails");
        };
        assert_eq!(error, Error::Node(lost()));
        let message = "no node runs in foundation-data";
        assert_eq!(shown(&error), read("node.not-running", message, 1));
        assert_eq!(remote.calls(), (1, 0));

        let remote = Remote::with(remote.node, Mode::Unsent);
        let planned = remote.plan(&paths, &files, None).await;
        assert_eq!(planned.err(), Some(Error::Node(lost())));
        assert_eq!(remote.calls(), (1, 1));
    });
}

#[test]
fn a_reply_that_is_no_reply_gives_bad_reply() {
    solo(|host, mesh| async move {
        let node = create_node(mesh);
        let paths = with_site(&host);
        let files = [PathBuf::from("site.hcl")];
        let cases: [(&[u8], &str); 5] = [
            (b"{\"output\":{}}", "no newline ends the first line"),
            (
                b"{\"errors\":[],\"status\":1}\n",
                "an error has the exit status 0, or no problem",
            ),
            (
                b"{\"errors\":[{\"code\":\"a\",\"message\":\"b\",\"fix\":\"c\",\
                  \"notes\":[]}],\"status\":0}\n",
                "an error has the exit status 0, or no problem",
            ),
            (
                b"{}\n",
                "data did not match any variant of untagged enum Answer",
            ),
            (b"{\"output\":{}}\n", "missing field `base`"),
        ];
        let mut remote = Remote::new(node);
        for (reply, cause) in cases {
            remote = Remote::with(remote.node, Mode::Reply(reply.to_vec()));
            let planned = remote.plan(&paths, &files, None).await;
            let Err(error) = planned else {
                panic!("no reply");
            };
            let message = format!("the reply of the node is no reply: {cause}");
            assert_eq!(shown(&error), read("ops.bad-reply", &message, 1));
        }
    });
}

#[test]
fn plan_whose_reply_holds_no_plan_gives_bad_reply_and_writes_no_file() {
    solo(|host, mesh| async move {
        let paths = with_site(&host);
        let node = create_node(mesh);
        let files = vec![(PathBuf::from("site.hcl"), placed_site())];
        let (_, output) = node.plan(files).await.expect("a plan");
        let reply = format!("{}\n", json!({ "output": output }));
        let remote = Remote::with(node, Mode::Reply(reply.into_bytes()));
        let out = Path::new("site.plan");
        let files = [PathBuf::from("site.hcl")];
        let planned = remote.plan(&paths, &files, Some(out)).await;
        let message =
            "no plan follows the first line of the reply of `plan`".to_owned();
        assert_eq!(planned.err(), Some(Error::Reply { message }));
        assert_eq!(paths.read(out, most()), Err(paths::Error::NotFound));
    });
}

#[test]
fn apply_whose_reply_has_bytes_after_its_first_line_gives_bad_reply() {
    solo(|host, mesh| async move {
        let paths = host.paths();
        paths.write(Path::new("site.plan"), b"x").expect("a write");
        let reply = b"{\"output\":{\"pointer\":{\"version\":1,\"root\":\"00\"},\
                      \"added\":0,\"changed\":0,\"removed\":0,\"homes\":0}}\nx";
        let remote = Remote::with(create_node(mesh), Mode::Reply(reply.to_vec()));
        let applied = remote.apply(&paths, Path::new("site.plan")).await;
        let message = "bytes follow the first line of the reply of `apply`".to_owned();
        assert_eq!(applied.err(), Some(Error::Reply { message }));
    });
}

#[test]
fn apply_of_a_plan_file_that_does_not_fit_a_request_reaches_no_node() {
    solo(|host, mesh| async move {
        let paths = host.paths();
        let around = serve::apply_request(b"").len();
        let plan = vec![b'x'; most() - around + 1];
        paths.write(Path::new("big.plan"), &plan).expect("a write");
        let remote = Remote::new(create_node(mesh));
        let applied = remote.apply(&paths, Path::new("big.plan")).await;
        assert_eq!(applied.err(), Some(Error::LargeRequest));
        let plan = [plan, b"x".to_vec()].concat();
        paths.write(Path::new("big.plan"), &plan).expect("a write");
        let applied = remote.apply(&paths, Path::new("big.plan")).await;
        assert_eq!(applied.err(), Some(Error::LargeRequest));
        let applied = remote.apply(&paths, Path::new("gone.plan")).await;
        let path = PathBuf::from("gone.plan");
        let cause = Unread::NotFound;
        assert_eq!(applied.err(), Some(Error::Read { path, cause }));
        assert_eq!(remote.calls(), (0, 0));
    });
}

#[test]
fn an_error_of_the_node_keeps_its_problems_and_status() {
    solo(|host, mesh| async move {
        let paths = host.paths();
        paths.write(Path::new("bad.hcl"), b"{").expect("a write");
        let remote = Remote::new(create_node(mesh));
        let files = [PathBuf::from("bad.hcl")];
        let planned = remote.plan(&paths, &files, None).await;
        let body = serve::plan_request(&[File {
            path: PathBuf::from("bad.hcl"),
            text: "{".to_owned(),
        }]);
        let reply = serve::serve(&remote.node, &name("@admin"), &body).await;
        let expected = serve::answer::<serde_json::Value>(&reply).map(drop);
        let Err(Error::Remote { problems, status }) = expected.clone() else {
            panic!("the node refuses the file: {expected:?}");
        };
        assert_eq!(status, 2);
        assert_eq!(planned.err(), Some(Error::Remote { problems, status }));

        let problem = Problem {
            code: "ops.stopped".to_owned(),
            message: "a\u{1b}b\\c".to_owned(),
            fix: "c".to_owned(),
            place: None,
            notes: Vec::new(),
        };
        let reply = format!(
            "{}\n",
            json!({ "errors": [problem.clone(), problem.clone()], "status": 7 })
        );
        let remote = Remote::with(remote.node, Mode::Reply(reply.into_bytes()));
        let planned = remote.plan(&paths, &files, None).await;
        let Err(error) = planned else {
            panic!("an error of the node");
        };
        assert_eq!(error.status(), 7);
        assert_eq!(*error.problems(), [problem.clone(), problem.clone()]);
        let shown = "error[ops.stopped]: a\\u{1b}b\\c\nfix: c\n";
        assert_eq!(error.text(), format!("{shown}\n{shown}"));
        assert_eq!(error.to_string(), "the node gave 2 problems");

        let reply = format!("{}\n", json!({ "errors": [problem], "status": 7 }));
        let remote = Remote::with(remote.node, Mode::Reply(reply.into_bytes()));
        let planned = remote.plan(&paths, &files, None).await;
        let shown = planned.err().map(|error| error.to_string());
        assert_eq!(shown.as_deref(), Some("a\u{1b}b\\c"), "one problem");
    });
}

#[test]
fn plan_and_apply_of_a_path_that_is_not_utf8_reach_no_node() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    solo(|host, mesh| async move {
        let paths = host.paths();
        let remote = Remote::new(create_node(mesh));
        let path = PathBuf::from(OsStr::from_bytes(b"a\xff.hcl"));
        let planned = remote.plan(&paths, std::slice::from_ref(&path), None).await;
        let Err(error) = planned else {
            panic!("a path that is not UTF-8");
        };
        assert_eq!(error.problems()[0].code, "ops.path-not-utf8");
        let applied = remote.apply(&paths, &path).await;
        let Err(error) = applied else {
            panic!("a path that is not UTF-8");
        };
        assert_eq!(error.problems()[0].code, "ops.path-not-utf8");
        assert_eq!(remote.calls(), (0, 0));
    });
}

#[test]
fn the_command_line_plans_and_applies_on_the_node() {
    solo(|host, mesh| async move {
        let paths = with_site(&host);
        let remote = Remote::new(create_node(mesh));
        let (status, output, errors) = remote
            .cli(&paths, &["plan", "site.hcl", "--out", "site.plan"])
            .await;
        assert_eq!((status, errors.as_str()), (Run::Exit(0), ""));
        assert!(output.contains("3 to add"), "{output}");
        let (status, output, errors) =
            remote.cli(&paths, &["apply", "site.plan", "--json"]).await;
        assert_eq!((status, errors.as_str()), (Run::Exit(0), ""));
        let output: serde_json::Value = serde_json::from_str(&output).expect("JSON");
        assert_eq!(output["file"], "site.plan");
        let (status, output, errors) = remote.cli(&paths, &["plan", "gone.hcl"]).await;
        assert_eq!((status, output.as_str()), (Run::Exit(2), ""));
        assert!(errors.contains("no file is at gone.hcl"), "{errors}");
        assert_eq!(remote.calls(), (2, 2));
    });
}
