use std::path::PathBuf;

use super::*;

/// A git repository in a fresh temporary directory. Each test ends with `remove`, so
/// a test that fails leaves its directory.
pub(in crate::review) struct Repo {
    pub(in crate::review) dir: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir()
            .join(format!("xtask-history-{name}-{}", std::process::id()));
        // A failed or killed run with the same PID can leave the directory behind.
        match std::fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{e}"),
            _ => {}
        }
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Self { dir };
        repo.git(&["init", "--quiet", "--initial-branch", "main"]);
        repo
    }

    /// A `git` command in the repository as `History` makes it, with an identity and
    /// no global ignore file, so the machine's settings cannot change a test.
    fn command(&self) -> Command {
        let mut command = command(&self.dir);
        command
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "core.excludesFile=/dev/null"]);
        command
    }

    pub(in crate::review) fn git(&self, args: &[&str]) -> String {
        let output = self.command().args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Writes `text` to `file` and commits it on the current branch.
    pub(in crate::review) fn commit(&self, file: &str, text: &str) -> String {
        let path = self.dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        self.git(&["add", file]);
        self.git(&["commit", "--quiet", "-m", file]);
        self.head()
    }

    pub(in crate::review) fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// Commits `text` to `file` on `main` and moves `origin/main` to it.
    fn advance_main(&self, file: &str, text: &str) -> String {
        self.git(&["switch", "--quiet", "main"]);
        let sha = self.commit(file, text);
        self.git(&["update-ref", "refs/remotes/origin/main", &sha]);
        self.git(&["switch", "--quiet", "pr"]);
        sha
    }

    /// A repository with `main` at one commit and the branch `pr` one commit after
    /// it. Returns the repository and the commit on `pr`.
    pub(in crate::review) fn with_pr(name: &str) -> (Self, String) {
        let repo = Self::new(name);
        let base = repo.commit("a.txt", "base\n");
        repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.git(&["switch", "--quiet", "-c", "pr"]);
        let end = repo.commit("b.txt", "pr\n");
        (repo, end)
    }

    /// Commits a tree of `files`, each a name of any bytes and its text, with
    /// `parents`, through plumbing, since a work tree on macOS refuses a name that is
    /// not UTF-8. Returns the commit.
    #[cfg(unix)]
    fn commit_tree(&self, files: &[(&[u8], &str)], parents: &[&str]) -> String {
        use std::os::unix::ffi::OsStrExt;
        self.git(&["read-tree", "--empty"]);
        for (name, text) in files {
            std::fs::write(self.dir.join("blob"), text).unwrap();
            let blob = self.git(&["hash-object", "-w", "blob"]);
            let info = [b"100644,", blob.as_bytes(), b",", name].concat();
            let status = self
                .command()
                .args(["update-index", "--add", "--cacheinfo"])
                .arg(std::ffi::OsStr::from_bytes(&info))
                .status()
                .unwrap();
            assert!(status.success());
        }
        let tree = self.git(&["write-tree"]);
        let parents = parents.iter().flat_map(|p| ["-p", p]);
        let args = [
            &["commit-tree", &tree, "-m", "c"][..],
            &parents.collect::<Vec<_>>(),
        ];
        self.git(&args.concat())
    }

    fn place(&self, end: &str, head: &str) -> Result<Place, String> {
        History::new(&self.dir, "main").place(end, head)
    }

    /// Whether a last round with a `Text fixes:` line that ends at `end` passes at
    /// `head`.
    fn passes(&self, end: &str, head: &str) -> Result<bool, String> {
        Ok(self.place(end, head)? != Place::Off
            && self.code_change(end, head)?.is_none())
    }

    fn code_change(&self, from: &str, end: &str) -> Result<Option<String>, String> {
        History::new(&self.dir, "main").code_change(from, end)
    }

    fn decision_change(&self, from: &str, end: &str) -> Result<Option<String>, String> {
        History::new(&self.dir, "main").decision_change(from, end)
    }

    fn surface_change(&self, from: &str, end: &str) -> Result<Option<String>, String> {
        History::new(&self.dir, "main").surface_change(from, end)
    }

    /// Removes the directory.
    ///
    /// # Panics
    ///
    /// When the removal fails.
    pub(in crate::review) fn remove(self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            panic!("remove {}: {e}", self.dir.display());
        }
    }
}

#[test]
fn passes_itself_by_full_sha_and_by_prefix() {
    let (repo, end) = Repo::with_pr("itself");
    assert_eq!(repo.passes(&end, &end), Ok(true));
    assert_eq!(repo.passes(&end[..7], &end), Ok(true));
    let child = repo.commit("a.md", "text\n");
    assert_eq!(repo.passes(&format!("{child}^"), &end), Ok(false));
    repo.remove();
}

#[test]
fn passes_through_clean_merges_of_main() {
    let (repo, end) = Repo::with_pr("clean");
    let main = repo.advance_main("c.txt", "main\n");
    repo.git(&["merge", "--quiet", "--no-edit", &main]);
    repo.advance_main("d.txt", "main again\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(repo.place(&end, &repo.head()), Ok(Place::Head));
    assert_eq!(repo.passes(&end, &repo.head()), Ok(true));
    repo.remove();
}

#[test]
fn refuses_a_commit_after_the_end() {
    let (repo, end) = Repo::with_pr("after");
    let head = repo.commit("b.txt", "changed\n");
    assert_eq!(repo.passes(&end, &head), Ok(false));
    repo.remove();
}

#[test]
fn refuses_a_merge_that_resolves_a_conflict() {
    let (repo, _) = Repo::with_pr("conflict");
    let end = repo.commit("a.txt", "pr side\n");
    repo.advance_main("a.txt", "main side\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&merge.stdout);
    assert!(
        out.contains("CONFLICT (content): Merge conflict in a.txt"),
        "{merge:?}"
    );
    std::fs::write(repo.dir.join("a.txt"), "resolved\n").unwrap();
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "--quiet", "--no-edit"]);
    assert_eq!(repo.place(&end, &repo.head()), Ok(Place::Off));
    repo.remove();
}

#[test]
fn refuses_a_clean_merge_with_an_added_change() {
    let (repo, end) = Repo::with_pr("amended");
    repo.advance_main("c.txt", "main\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    std::fs::write(repo.dir.join("b.txt"), "slipped in\n").unwrap();
    repo.git(&["commit", "--quiet", "--all", "--amend", "--no-edit"]);
    assert_eq!(repo.place(&end, &repo.head()), Ok(Place::Off));
    repo.remove();
}

#[test]
fn refuses_a_merge_of_a_branch_that_is_not_main() {
    let (repo, end) = Repo::with_pr("other");
    repo.git(&["switch", "--quiet", "-c", "other", "main"]);
    let other = repo.commit("c.md", "other\n");
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", &other]);
    assert_eq!(repo.code_change(&end, &repo.head()), Ok(None));
    assert_eq!(repo.place(&end, &repo.head()), Ok(Place::Off));
    repo.remove();
}

#[test]
fn text_that_names_no_commit_does_not_pass() {
    let (repo, end) = Repo::with_pr("unknown");
    assert_eq!(repo.passes("abc12", &end), Ok(false));
    assert_eq!(repo.passes("HEAD~1x", &end), Ok(false));
    assert_eq!(repo.passes("deadbeef", &end), Ok(false));
    assert_eq!(repo.passes(&end[..6], &end), Ok(false));
    assert_eq!(repo.passes(&format!("{end}^{{commit}}"), &end), Ok(false));
    repo.remove();
}

#[test]
fn places_on_the_first_parent_chain_only() {
    let (repo, end) = Repo::with_pr("place");
    let main = repo.advance_main("c.md", "main\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    let head = repo.commit("a.md", "text\n");
    assert_eq!(repo.place(&head, &head), Ok(Place::Head));
    assert_eq!(repo.place(&merge, &head), Ok(Place::Before));
    assert_eq!(repo.place(&end, &merge), Ok(Place::Head));
    assert_eq!(repo.place(&end[..7], &head), Ok(Place::Before));
    assert_eq!(repo.place(&main, &head), Ok(Place::Off));
    assert_eq!(repo.passes(&main, &head), Ok(false));
    assert_eq!(repo.place(&head, &end), Ok(Place::Off));
    assert_eq!(repo.place("deadbeef", &head), Ok(Place::Off));
    let tree = format!("{end}^{{tree}}");
    let second = repo.git(&["commit-tree", &tree, "-p", "main", "-p", &end, "-m", "m"]);
    assert_eq!(repo.place(&end, &second), Ok(Place::Off));
    repo.remove();
}

#[test]
fn refuses_an_end_off_the_chain_after_clean_merges_of_the_base() {
    let (repo, _) = Repo::with_pr("off");
    let old = repo.advance_main("c.md", "one\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let main = repo.advance_main("c.md", "two\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("a.md", "text\n");
    let head = repo.commit("b.md", "text\n");
    assert_eq!(repo.place(&end, &head), Ok(Place::Before));
    assert_eq!(repo.place(&main, &head), Ok(Place::Off));
    assert_eq!(repo.place(&old, &head), Ok(Place::Off));
    repo.remove();
}

#[test]
fn places_an_end_off_the_chain_before_reading_the_base() {
    let repo = Repo::new("unrelated");
    repo.commit("a.txt", "base\n");
    repo.git(&["switch", "--quiet", "--orphan", "other"]);
    repo.commit("c.txt", "other\n");
    repo.git(&["switch", "--quiet", "main"]);
    let unrelated = [
        "merge",
        "--quiet",
        "--no-edit",
        "--allow-unrelated-histories",
    ];
    repo.git(&[&unrelated[..], &["other"]].concat());
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "-c", "rebased"]);
    let gone = repo.commit("b.md", "one\n");
    repo.git(&["switch", "--quiet", "-c", "new", "main"]);
    let head = repo.commit("b.md", "two\n");
    assert_eq!(repo.place(&gone, &head), Ok(Place::Off));
    repo.remove();
}

#[test]
fn places_an_end_before_a_merge_of_an_unrelated_base_commit() {
    let (repo, end) = Repo::with_pr("unrelated-chain");
    repo.git(&["switch", "--quiet", "--orphan", "other"]);
    let other = repo.commit("c.txt", "other\n");
    repo.git(&["switch", "--quiet", "main"]);
    let unrelated = [
        "merge",
        "--quiet",
        "--no-edit",
        "--allow-unrelated-histories",
    ];
    repo.git(&[&unrelated[..], &["other"]].concat());
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&[&unrelated[..], &[other.as_str()]].concat());
    let head = repo.head();
    assert_eq!(repo.place(&end, &head), Ok(Place::Head));
    // The head holds both roots of the base, so it fails closed.
    let roots = repo.git(&["merge-base", "--all", &head, "origin/main"]);
    let mut roots: Vec<_> = roots.lines().collect();
    roots.sort_unstable();
    assert_eq!(
        repo.code_change(&end, &head),
        Ok(Some(format!(
            "holds the base at more than one newest commit: `{}`",
            roots.join("`, `")
        )))
    );
    repo.remove();
}

#[test]
fn passes_commits_that_change_no_code_after_the_end() {
    let (repo, _) = Repo::with_pr("text");
    let end = repo.commit("a.rs", "fn a() {}\n");
    let text = |n: &str| {
        std::fs::write(repo.dir.join("a.md"), n).unwrap();
        let rust = std::fs::read_to_string(repo.dir.join("a.rs")).unwrap();
        std::fs::write(repo.dir.join("a.rs"), format!("// {n}\n{rust}")).unwrap();
        repo.git(&["add", "a.md", "a.rs"]);
        repo.git(&["commit", "--quiet", "-m", n]);
        repo.head()
    };
    let first = text("first");
    assert_eq!(repo.place(&end, &first), Ok(Place::Before));
    assert_eq!(repo.code_change(&end, &first), Ok(None));
    assert_eq!(repo.passes(&end, &first), Ok(true));
    repo.advance_main("c.txt", "main\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let second = text("second");
    assert_eq!(repo.place(&end, &second), Ok(Place::Before));
    assert_eq!(repo.code_change(&end, &second), Ok(None));
    assert_eq!(repo.passes(&end, &second), Ok(true));
    repo.remove();
}

#[test]
fn refuses_a_commit_that_changes_code_after_the_end() {
    let (repo, _) = Repo::with_pr("code-after");
    let end = repo.commit("a.rs", "// A.\n");
    let rust = repo.commit("a.rs", "// A.\nfn a() {}\n");
    assert_eq!(
        repo.code_change(&end, &rust),
        Ok(Some("changes code at `a.rs:2`".to_string()))
    );
    assert_eq!(repo.passes(&end, &rust), Ok(false));
    let yaml = repo.commit("a.yaml", "# A comment.\n");
    assert_eq!(
        repo.code_change(&rust, &yaml),
        Ok(Some("changes code at `a.yaml:1`".to_string()))
    );
    assert_eq!(repo.passes(&rust, &yaml), Ok(false));
    repo.commit("a.sh", "echo a\n");
    let script = repo.head();
    repo.git(&["update-index", "--chmod=+x", "a.sh"]);
    repo.git(&["commit", "--quiet", "-m", "mode"]);
    let mode = repo.head();
    assert_eq!(
        repo.code_change(&script, &mode),
        Ok(Some("changes code at `a.sh`".to_string()))
    );
    assert_eq!(repo.passes(&script, &mode), Ok(false));
    std::fs::write(repo.dir.join("a.bin"), b"\x00\x01\xff").unwrap();
    repo.git(&["add", "a.bin"]);
    repo.git(&["commit", "--quiet", "-m", "bin"]);
    let binary = repo.head();
    assert_eq!(
        repo.code_change(&mode, &binary),
        Ok(Some("changes code at `a.bin:1`".to_string()))
    );
    assert_eq!(repo.passes(&mode, &binary), Ok(false));
    repo.remove();
}

#[test]
fn a_shell_script_line_is_code() {
    let (repo, from) = Repo::with_pr("script");
    let end = repo.commit("a.sh", "# A comment.\n");
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `a.sh:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn an_extension_in_another_case_is_code() {
    let (repo, from) = Repo::with_pr("case");
    let text = repo.commit("A.MD", "text\n");
    assert_eq!(
        repo.code_change(&from, &text),
        Ok(Some("changes code at `A.MD:1`".to_string()))
    );
    let rust = repo.commit("a.RS", "// A.\n");
    assert_eq!(
        repo.code_change(&text, &rust),
        Ok(Some("changes code at `a.RS:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_change_of_mode_or_an_empty_file_is_code_except_in_text() {
    let (repo, from) = Repo::with_pr("mode");
    let empty = repo.commit("e.yaml", "");
    assert_eq!(
        repo.code_change(&from, &empty),
        Ok(Some("changes code at `e.yaml`".to_string()))
    );
    let rust = repo.commit("e.rs", "");
    assert_eq!(repo.code_change(&empty, &rust), Ok(None));
    let text = repo.commit("a.md", "text\n");
    for file in ["e.rs", "a.md"] {
        repo.git(&["update-index", "--chmod=+x", file]);
    }
    repo.git(&["commit", "--quiet", "-m", "mode"]);
    let mode = repo.head();
    assert_eq!(
        repo.code_change(&text, &mode),
        Ok(Some("changes code at `e.rs`".to_string()))
    );
    repo.git(&["update-index", "--chmod=-x", "a.md"]);
    repo.git(&["commit", "--quiet", "-m", "mode"]);
    assert_eq!(repo.code_change(&mode, &repo.head()), Ok(None));
    repo.remove();
}

#[test]
fn a_symlink_is_code() {
    for path in ["build.rs", "a.md"] {
        let (repo, from) = Repo::with_pr(&format!("symlink-{path}"));
        // A target that starts with `//` reads as a comment line of a `.rs` file.
        let link = |target: &str| {
            std::fs::write(repo.dir.join("blob"), target).unwrap();
            let blob = repo.git(&["hash-object", "-w", "blob"]);
            let info = format!("120000,{blob},{path}");
            repo.git(&["update-index", "--add", "--cacheinfo", &info]);
            repo.git(&["commit", "--quiet", "-m", "link"]);
            repo.head()
        };
        let added = link("//tmp/a.md");
        assert_eq!(
            repo.code_change(&from, &added),
            Ok(Some(format!("changes code at `{path}`"))),
            "{path}"
        );
        let moved = link("//tmp/b.md");
        assert_eq!(
            repo.code_change(&added, &moved),
            Ok(Some(format!("changes code at `{path}`"))),
            "{path}"
        );
        repo.remove();
    }
}

#[test]
fn a_submodule_is_code() {
    for path in ["sub", "sub.rs", "sub.md"] {
        let (repo, from) = Repo::with_pr(&format!("submodule-{path}"));
        let link = |commit: &str| {
            let info = format!("160000,{commit},{path}");
            repo.git(&["update-index", "--add", "--cacheinfo", &info]);
            repo.git(&["commit", "--quiet", "-m", "link"]);
            repo.head()
        };
        let added = link(&from);
        assert_eq!(
            repo.code_change(&from, &added),
            Ok(Some(format!("changes code at `{path}`"))),
            "{path}"
        );
        let moved = link(&added);
        assert_eq!(
            repo.code_change(&added, &moved),
            Ok(Some(format!("changes code at `{path}`"))),
            "{path}"
        );
        repo.remove();
    }
}

#[test]
fn refuses_a_merge_with_a_conflict_in_a_file_that_is_not_text() {
    let (repo, _) = Repo::with_pr("yaml-conflict");
    let end = repo.commit("a.yaml", "pr: 1\n");
    repo.advance_main("a.yaml", "main: 1\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1), "{merge:?}");
    std::fs::write(repo.dir.join("a.yaml"), "pr: 1\n").unwrap();
    repo.git(&["commit", "--quiet", "--all", "--no-edit"]);
    let merge = repo.head();
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some(format!(
            "resolves a conflict in `a.yaml` in `{merge}`"
        )))
    );
    assert_eq!(repo.passes(&end, &merge), Ok(false));
    repo.remove();
}

#[test]
fn names_the_git_failure_for_an_unknown_head() {
    let (repo, end) = Repo::with_pr("head");
    assert_eq!(
        repo.passes(&end, "deadbeef"),
        Err(
            "git rev-parse --verify deadbeef^{commit}: fatal: Needed a single revision"
                .to_string()
        )
    );
    repo.remove();
}

#[test]
fn a_range_of_comment_and_blank_lines_changes_no_code() {
    let (repo, from) = Repo::with_pr("comments");
    let code = repo.commit("a.rs", "/// A.\nfn a() {}\n");
    let docs = repo.commit("a.rs", "/// A, wrapped.\n\n  // B.\nfn a() {}\n\n");
    assert_eq!(repo.code_change(&code, &docs), Ok(None));
    let text = repo.commit("b.md", "fn b() {}\n");
    assert_eq!(repo.code_change(&code[..7], &text[..7]), Ok(None));
    let removed = repo.commit("a.rs", "fn a() {}\n");
    assert_eq!(repo.code_change(&text, &removed), Ok(None));
    assert_eq!(repo.code_change(&removed, &removed), Ok(None));
    assert_eq!(
        repo.code_change(&from, &code),
        Ok(Some("changes code at `a.rs:2`".to_string()))
    );
    repo.remove();
}

#[test]
fn names_the_first_line_of_code_that_a_range_changes() {
    let (repo, _) = Repo::with_pr("code");
    let from = repo.commit("a.rs", "// A.\nfn a() {}\n\nfn b() {}\n");
    repo.commit("c.rs", "fn c() {}\n");
    let end = repo.commit("a.rs", "// A, B.\nfn a() {}\n\nfn d() {}\n");
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `a.rs:4`".to_string()))
    );
    let deleted = repo.commit("a.rs", "// A, B.\nfn a() {}\n\n");
    assert_eq!(
        repo.code_change(&end, &deleted),
        Ok(Some("changes code at `a.rs:4`".to_string()))
    );
    std::fs::remove_file(repo.dir.join("c.rs")).unwrap();
    repo.git(&["commit", "--quiet", "-am", "rm"]);
    assert_eq!(
        repo.code_change(&deleted, &repo.head()),
        Ok(Some("changes code at `c.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_range_that_names_no_commit_has_that_text() {
    let (repo, end) = Repo::with_pr("unnamed");
    assert_eq!(
        repo.code_change("deadbeef", &end),
        Ok(Some("has `deadbeef`, which names no commit".to_string()))
    );
    assert_eq!(
        repo.code_change(&end, "HEAD"),
        Ok(Some("has `HEAD`, which names no commit".to_string()))
    );
    repo.remove();
}

#[test]
fn a_range_may_start_at_the_parent_of_a_commit() {
    let (repo, end) = Repo::with_pr("parent");
    let fix = repo.commit("c.rs", "fn c() {}\n");
    let head = repo.commit("d.md", "text\n");
    let short = &fix[..8];
    assert_eq!(
        repo.code_change(&format!("{short}^"), &head),
        Ok(Some("changes code at `c.rs:1`".to_string()))
    );
    assert_eq!(
        repo.code_change(&format!("{}^", &head[..8]), &head),
        Ok(None)
    );
    assert_eq!(repo.code_change(&format!("{fix}^"), &end), Ok(None));
    assert_eq!(
        repo.code_change(&end, &format!("{fix}^")),
        Ok(Some(format!("has `{fix}^`, which names no commit")))
    );
    assert_eq!(
        repo.code_change(&format!("{short}^^"), &head),
        Ok(Some(format!("has `{short}^^`, which names no commit")))
    );
    let root = repo.git(&["rev-list", "--max-parents=0", "HEAD"]);
    assert_eq!(
        repo.code_change(&format!("{root}^"), &head),
        Ok(Some(format!("has `{root}^`, which names no commit")))
    );
    assert_eq!(
        repo.code_change(&format!("{}^", &head[..6]), &head),
        Ok(Some(format!(
            "has `{}^`, which names no commit",
            &head[..6]
        )))
    );
    repo.remove();
}

#[test]
fn counts_lines_to_the_first_line_of_code() {
    let (repo, _) = Repo::with_pr("count");
    let empty = repo.commit("a.rs", "");
    let added = repo.commit("a.rs", "// A.\n\nfn a() {}\n");
    assert_eq!(
        repo.code_change(&empty, &added),
        Ok(Some("changes code at `a.rs:3`".to_string()))
    );
    assert_eq!(
        repo.code_change(&added, &empty),
        Ok(Some("changes code at `a.rs:3`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_moved_file_and_a_manifest_change_code() {
    let (repo, _) = Repo::with_pr("moved");
    std::fs::create_dir_all(repo.dir.join("tests/data")).unwrap();
    let from = repo.commit("tests/oracle.rs", "#[test]\nfn holds() {}\n");
    repo.git(&["mv", "tests/oracle.rs", "tests/data/oracle.rs"]);
    repo.git(&["commit", "--quiet", "-m", "move"]);
    assert_eq!(
        repo.code_change(&from, &repo.head()),
        Ok(Some("changes code at `tests/data/oracle.rs:1`".to_string()))
    );
    let from = repo.head();
    std::fs::create_dir_all(repo.dir.join("crates/a")).unwrap();
    let toml = repo.commit("crates/a/Cargo.toml", "# A comment.\n");
    assert_eq!(
        repo.code_change(&from, &toml),
        Ok(Some("changes code at `crates/a/Cargo.toml:1`".to_string()))
    );
    let lock = repo.commit("Cargo.lock", "\n");
    assert_eq!(
        repo.code_change(&toml, &lock),
        Ok(Some("changes code at `Cargo.lock:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_removed_line_that_looks_like_a_header_is_code() {
    let (repo, _) = Repo::with_pr("header");
    let from = repo.commit("a.rs", "const Q: &str = \"\n-- a\n++ b\n\";\n");
    let end = repo.commit("a.rs", "const Q: &str = \"\n\";\n");
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `a.rs:2`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_nul_byte_does_not_hide_code() {
    let (repo, from) = Repo::with_pr("nul");
    let added = repo.commit("a.rs", "// A \0 byte.\nfn a() {}\n");
    let changed = repo.commit("a.rs", "// A \0 byte.\nfn b() {}\n");
    let found = Ok(Some("changes code at `a.rs:2`".to_string()));
    assert_eq!(repo.code_change(&from, &added), found);
    assert_eq!(repo.code_change(&added, &changed), found);
    repo.remove();
}

#[test]
fn reads_a_name_that_git_quotes() {
    for name in ["caf\u{e9}.rs", "a b.rs", "a\tb.rs", "\"a\".rs"] {
        let (repo, _) = Repo::with_pr("quoted");
        let from = repo.commit(name, "fn a() {}\n");
        let docs = repo.commit(name, "// A.\nfn a() {}\n");
        assert_eq!(repo.code_change(&from, &docs), Ok(None), "{name:?}");
        let code = repo.commit(name, "// A.\nfn b() {}\n");
        assert_eq!(
            repo.code_change(&docs, &code),
            Ok(Some(format!("changes code at `{name}:2`"))),
            "{name:?}"
        );
        repo.remove();
    }
}

#[test]
fn a_move_does_not_hide_a_removed_line_of_code() {
    let (repo, _) = Repo::with_pr("rename");
    let comments = "// c\n".repeat(10);
    let from = repo.commit("a.rs", &format!("{comments}fn a() {{}}\n"));
    repo.git(&["rm", "--quiet", "a.rs"]);
    let end = repo.commit("b.rs", &comments);
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `a.rs:11`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_diff_driver_config_does_not_hide_code() {
    for config in [["diff.x.textconv", "true"], ["diff.external", "true"]] {
        let (repo, _) = Repo::with_pr("driver");
        repo.git(&["config", config[0], config[1]]);
        repo.commit(".gitattributes", "* diff=x\n");
        let from = repo.commit("a.rs", "// a\n");
        let code = repo.commit("a.rs", "fn a() {}\n");
        assert_eq!(
            repo.code_change(&from, &code),
            Ok(Some("changes code at `a.rs:1`".to_string())),
            "{config:?}"
        );
        repo.remove();
    }
}

#[test]
fn a_colored_diff_config_does_not_hide_code() {
    let (repo, _) = Repo::with_pr("color");
    repo.git(&["config", "color.diff", "always"]);
    repo.git(&["config", "diff.noprefix", "false"]);
    repo.git(&["config", "diff.renames", "copies"]);
    let from = repo.commit("a.rs", "// a\n");
    let code = repo.commit("a.rs", "fn a() {}\n");
    assert_eq!(
        repo.code_change(&from, &code),
        Ok(Some("changes code at `a.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn an_inter_hunk_context_config_keeps_the_line_number() {
    let (repo, _) = Repo::with_pr("context");
    repo.git(&["config", "diff.interHunkContext", "10"]);
    let from = repo.commit("a.rs", "// a\nfn keep() {}\n// b\n");
    let end = repo.commit("a.rs", "// A\nfn keep() {}\n// b\nfn c() {}\n");
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `a.rs:4`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_merge_round_with_no_resolution_skips_the_breaker() {
    let (repo, end) = Repo::with_pr("merge-none");
    repo.advance_main("a.rs", "fn a() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let docs = repo.commit("b.rs", "// B.\n");
    repo.advance_main("c.rs", "fn c() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(repo.code_change(&end, &repo.head()), Ok(None));
    assert_eq!(repo.code_change(&docs, &repo.head()), Ok(None));
    repo.remove();
}

#[test]
fn a_merge_round_whose_resolution_changes_code_needs_the_breaker() {
    let (repo, end) = Repo::with_pr("merge-code");
    repo.advance_main("a.rs", "fn a() {}\n");
    repo.git(&["merge", "--quiet", "--no-commit", "origin/main"]);
    std::fs::write(repo.dir.join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    repo.git(&["commit", "--quiet", "--all", "--no-edit"]);
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some("changes code at `a.rs:2`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_merge_round_whose_resolution_resolves_a_conflict_needs_the_breaker() {
    let (repo, _) = Repo::with_pr("merge-conflict");
    let end = repo.commit("a.rs", "// PR.\n");
    repo.advance_main("a.rs", "// Main.\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1), "{merge:?}");
    std::fs::write(repo.dir.join("a.rs"), "// PR.\n// Main.\n").unwrap();
    repo.git(&["commit", "--quiet", "--all", "--no-edit"]);
    let merge = repo.head();
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some(format!("resolves a conflict in `a.rs` in `{merge}`")))
    );
    repo.remove();
}

#[test]
fn a_merge_round_that_keeps_a_file_one_side_deleted_needs_the_breaker() {
    for pr_deletes in [false, true] {
        let (repo, _) = Repo::with_pr(&format!("merge-kept-{pr_deletes}"));
        repo.advance_main("a.rs", "fn a() {}\n");
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let changed = "fn a() {}\nfn b() {}\n";
        let end = if pr_deletes {
            repo.git(&["rm", "--quiet", "a.rs"]);
            repo.git(&["commit", "--quiet", "-m", "rm a.rs"]);
            repo.advance_main("a.rs", changed);
            repo.head()
        } else {
            let end = repo.commit("a.rs", changed);
            repo.git(&["switch", "--quiet", "main"]);
            repo.git(&["rm", "--quiet", "a.rs"]);
            repo.git(&["commit", "--quiet", "-m", "rm a.rs"]);
            repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
            repo.git(&["switch", "--quiet", "pr"]);
            end
        };
        let merge = repo
            .command()
            .args(["merge", "--no-edit", "origin/main"])
            .output()
            .unwrap();
        assert_eq!(merge.status.code(), Some(1), "{merge:?}");
        std::fs::write(repo.dir.join("a.rs"), changed).unwrap();
        repo.git(&["add", "a.rs"]);
        repo.git(&["commit", "--quiet", "--no-edit"]);
        let merge = repo.head();
        assert_eq!(
            repo.code_change(&end, &merge),
            Ok(Some(format!("resolves a conflict in `a.rs` in `{merge}`"))),
            "{pr_deletes}"
        );
        repo.remove();
    }
}

#[test]
fn a_conflict_counts_only_in_a_code_file() {
    let files = [
        (".rs", true),
        ("Cargo.toml", true),
        ("Cargo.lock", true),
        ("a.md", false),
    ];
    for (file, code) in files {
        let (repo, _) = Repo::with_pr(&format!("merge-file-{file}"));
        let end = repo.commit(file, "pr\n");
        repo.advance_main(file, "main\n");
        let merge = repo
            .command()
            .args(["merge", "--no-edit", "origin/main"])
            .output()
            .unwrap();
        assert_eq!(merge.status.code(), Some(1), "{merge:?}");
        std::fs::write(repo.dir.join(file), "pr\n").unwrap();
        repo.git(&["commit", "--quiet", "--all", "--no-edit"]);
        let merge = repo.head();
        let found =
            code.then(|| format!("resolves a conflict in `{file}` in `{merge}`"));
        assert_eq!(repo.code_change(&end, &merge), Ok(found), "{file}");
        repo.remove();
    }
}

#[test]
fn a_conflict_in_a_file_named_with_a_tilde_counts() {
    for file in ["Cargo.toml~", "a.md~old"] {
        let (repo, _) = Repo::with_pr(&format!("merge-tilde-{file}"));
        let end = repo.commit(file, "pr\n");
        repo.advance_main(file, "main\n");
        let merge = repo
            .command()
            .args(["merge", "--no-edit", "origin/main"])
            .output()
            .unwrap();
        assert_eq!(merge.status.code(), Some(1), "{merge:?}");
        std::fs::write(repo.dir.join(file), "pr\n").unwrap();
        repo.git(&["commit", "--quiet", "--all", "--no-edit"]);
        let merge = repo.head();
        assert_eq!(
            repo.code_change(&end, &merge),
            Ok(Some(format!(
                "resolves a conflict in `{file}` in `{merge}`"
            ))),
            "{file}"
        );
        repo.remove();
    }
}

#[test]
fn a_start_base_conflict_in_a_file_named_with_a_tilde_counts() {
    let (repo, _) = Repo::with_pr("merge-tilde-start");
    let end = repo.commit("a.md~", "pr\n");
    repo.git(&["rm", "--quiet", "a.md~"]);
    repo.git(&["commit", "--quiet", "-m", "rm"]);
    repo.advance_main("a.md~", "main\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some(
            "has a conflict in `a.md~` between its start and the base".to_string()
        ))
    );
    repo.remove();
}

#[test]
fn a_conflict_in_code_after_a_conflict_in_text_counts() {
    let (repo, _) = Repo::with_pr("merge-two");
    repo.commit("a.md", "base\n");
    repo.advance_main("b.rs", "fn b() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.commit("a.md", "pr\n");
    let end = repo.commit("b.rs", "fn b() {}\nfn c() {}\n");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-edit", "pr~2"]);
    repo.commit("a.md", "main\n");
    repo.git(&["rm", "--quiet", "b.rs"]);
    repo.git(&["commit", "--quiet", "-m", "rm b.rs"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1), "{merge:?}");
    std::fs::write(repo.dir.join("a.md"), "pr\n").unwrap();
    repo.git(&["add", "a.md", "b.rs"]);
    repo.git(&["commit", "--quiet", "--no-edit"]);
    let merge = repo.head();
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some(format!("resolves a conflict in `b.rs` in `{merge}`")))
    );
    repo.remove();
}

#[test]
fn the_file_named_dot_rs_is_rust() {
    let (repo, end) = Repo::with_pr("dot-rs");
    let docs = repo.commit(".rs", "// A.\n");
    assert_eq!(repo.code_change(&end, &docs), Ok(None));
    repo.commit(".rs", "// A.\nfn a() {}\n");
    assert_eq!(
        repo.code_change(&docs, &repo.head()),
        Ok(Some("changes code at `.rs:2`".to_string()))
    );
    repo.remove();
}

#[test]
fn an_added_line_is_numbered_in_the_end() {
    let (repo, end) = Repo::with_pr("merge-line");
    repo.commit("a.rs", "fn a() {}\n");
    repo.advance_main("c.txt", "main\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.commit("a.rs", "// 1\n// 2\n// 3\nfn a() {}\n");
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some("changes code at `a.rs:4`".to_string()))
    );
    repo.remove();
}

#[cfg(unix)]
#[test]
fn reads_a_merge_whose_messages_are_not_utf8() {
    let repo = Repo::new("merge-bytes");
    let commit =
        |files: [(&[u8], &str); 2], parents: &[&str]| repo.commit_tree(&files, parents);
    let lines = |first: &str, last: &str| format!("{first}\n2\n3\n4\n5\n{last}\n");
    let base = commit([(b"\xff.md", &lines("1", "6")), (b"y.md", "y\n")], &[]);
    let main = commit([(b"\xff.md", &lines("1", "m")), (b"y.md", "m\n")], &[&base]);
    repo.git(&["update-ref", "refs/remotes/origin/main", &main]);
    let end = commit([(b"\xff.md", &lines("p", "6")), (b"y.md", "p\n")], &[&base]);
    let resolved = lines("p", "m");
    let merge = commit([(b"\xff.md", &resolved), (b"y.md", "p\n")], &[&end, &main]);
    assert_eq!(repo.code_change(&end, &merge), Ok(None));
    assert_eq!(repo.passes(&end, &merge), Ok(false));
    repo.remove();
}

#[cfg(unix)]
#[test]
fn a_conflict_in_a_code_path_that_is_not_utf8_counts() {
    let repo = Repo::new("merge-bytes-code");
    let base = repo.commit_tree(&[(b"\xff.rs", "fn a() {}\n")], &[]);
    let main = repo.commit_tree(&[(b"a.md", "main\n")], &[&base]);
    repo.git(&["update-ref", "refs/remotes/origin/main", &main]);
    let kept: [(&[u8], &str); 2] =
        [(b"\xff.rs", "fn a() {}\nfn b() {}\n"), (b"a.md", "main\n")];
    let end = repo.commit_tree(&kept[..1], &[&base]);
    let merge = repo.commit_tree(&kept, &[&end, &main]);
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some(format!(
            "resolves a conflict in `\u{fffd}.rs` in `{merge}`"
        )))
    );
    repo.remove();
}

#[test]
fn text_of_the_pr_that_the_base_moves_into_a_code_file_counts() {
    for earlier in [false, true] {
        let (repo, _) = Repo::with_pr(&format!("merge-rename-{earlier}"));
        let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        repo.advance_main("a.md", text);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let end = repo.head();
        repo.commit("a.md", &format!("fn unreviewed() {{}}\n{text}"));
        if earlier {
            repo.advance_main("c.txt", "c\n");
            repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        }
        repo.git(&["switch", "--quiet", "main"]);
        repo.git(&["mv", "a.md", "a.rs"]);
        repo.git(&["commit", "--quiet", "-m", "mv"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["switch", "--quiet", "pr"]);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        assert_eq!(
            repo.git(&["show", "HEAD:a.rs"]).lines().next(),
            Some("fn unreviewed() {}")
        );
        assert_eq!(
            repo.code_change(&end, &repo.head()),
            Ok(Some("changes code at `a.rs:1`".to_string())),
            "{earlier}"
        );
        repo.remove();
    }
}

#[test]
fn text_of_an_earlier_round_that_the_base_moves_into_a_code_file_counts() {
    let (repo, _) = Repo::with_pr("move-earlier");
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    repo.advance_main("a.md", text);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("a.md", &format!("fn unreviewed() {{}}\n{text}"));
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["mv", "a.md", "a.rs"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    assert_eq!(
        repo.git(&["show", "HEAD:a.rs"]).lines().next(),
        Some("fn unreviewed() {}")
    );
    assert_eq!(repo.passes(&end, &merge), Ok(false));
    assert_eq!(repo.place(&end, &merge), Ok(Place::Off));
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some(
            "has `a.md`, which the PR changes and the base moves into the code file \
             `a.rs`"
                .to_string()
        ))
    );
    repo.remove();
}

#[test]
fn a_base_move_counts_by_the_source_that_the_merge_pairs() {
    let a = (1..=10)
        .map(|i| format!("a line {i:02}\n"))
        .collect::<Vec<_>>()
        .concat();
    let x = (1..=8)
        .map(|i| format!("x line {i:02}\n"))
        .collect::<Vec<_>>()
        .concat();
    let near = format!("{a}{}", x.replace("x line 08", "y line 08"));
    let (repo, _) = Repo::with_pr("move-pairing");
    repo.advance_main("a.md", &a);
    repo.advance_main("b.md", &near);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("a.md", &format!("fn unreviewed() {{}}\n{a}"));
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["rm", "--quiet", "a.md", "b.md"]);
    repo.commit("c.rs", &format!("{a}{x}"));
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    assert_eq!(
        repo.git(&["show", "HEAD:c.rs"]).lines().next(),
        Some("fn unreviewed() {}")
    );
    assert_eq!(repo.passes(&end, &merge), Ok(false));
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some(
            "has `a.md`, which the PR changes and the base moves into the code file \
             `c.rs`"
                .to_string()
        ))
    );
    repo.remove();
}

#[test]
fn a_base_move_that_a_later_commit_undoes_does_not_count() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let (repo, _) = Repo::with_pr("move-undone");
    repo.advance_main("a.md", text);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.head();
    repo.commit("a.md", &format!("fn unreviewed() {{}}\n{text}"));
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["mv", "a.md", "a.rs"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let undone = repo.commit("a.rs", text);
    assert_eq!(repo.code_change(&end, &undone), Ok(None));
    repo.remove();
}

#[test]
fn a_base_move_before_a_later_merge_of_the_base_does_not_count() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let (repo, _) = Repo::with_pr("move-then-merge");
    repo.advance_main("a.md", text);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.head();
    repo.commit("a.md", &format!("fn unreviewed() {{}}\n{text}"));
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["mv", "a.md", "a.rs"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.commit("a.rs", text);
    repo.advance_main("c.txt", "c\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(repo.code_change(&end, &repo.head()), Ok(None));
    repo.remove();
}

#[test]
fn a_base_move_names_the_path_that_it_moves() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let (repo, _) = Repo::with_pr("move-second");
    repo.advance_main("a.md", text);
    repo.advance_main("b.md", text);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.commit("a.md", &format!("{text}11\n"));
    let end = repo.commit("b.md", &format!("fn unreviewed() {{}}\n{text}"));
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["mv", "b.md", "b.rs"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some(
            "has `b.md`, which the PR changes and the base moves into the code file \
             `b.rs`"
                .to_string()
        ))
    );
    let probes = std::fs::read_dir(repo.dir.join(".git"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("xtask-review-probe"))
        .collect::<Vec<_>>();
    assert_eq!(probes, Vec::<String>::new());
    repo.remove();
}

#[test]
fn a_base_move_that_comes_in_by_another_merge_counts() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    for reversed in [false, true] {
        let (repo, _) = Repo::with_pr(&format!("move-shape-{reversed}"));
        repo.advance_main("a.md", text);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let end = repo.commit("a.md", &format!("fn unreviewed() {{}}\n{text}"));
        repo.git(&["switch", "--quiet", "main"]);
        repo.git(&["mv", "a.md", "a.rs"]);
        repo.git(&["commit", "--quiet", "-m", "mv"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        if reversed {
            repo.git(&["merge", "--quiet", "--no-edit", "pr"]);
            let merge = repo.head();
            repo.git(&["switch", "--quiet", "pr"]);
            repo.git(&["merge", "--quiet", "--ff-only", &merge]);
        } else {
            repo.git(&["switch", "--quiet", "-c", "local"]);
            repo.git(&["commit", "--quiet", "--allow-empty", "-m", "empty"]);
            repo.git(&["switch", "--quiet", "pr"]);
            repo.git(&["merge", "--quiet", "--no-edit", "local"]);
        }
        assert_eq!(
            repo.git(&["show", "HEAD:a.rs"]).lines().next(),
            Some("fn unreviewed() {}")
        );
        assert_eq!(
            repo.code_change(&end, &repo.head()),
            Ok(Some(
                "has `a.md`, which the PR changes and the base moves into the code file \
                 `a.rs`"
                    .to_string()
            )),
            "{reversed}"
        );
        repo.remove();
    }
}

#[test]
fn a_start_with_two_merge_bases_with_the_base_reads_both() {
    let (repo, pr) = Repo::with_pr("criss-cross");
    let main = repo.advance_main("m.txt", "m\n");
    repo.git(&["merge", "--quiet", "--no-edit", &main]);
    let end = repo.head();
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-edit", &pr]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    assert_eq!(repo.passes(&end, &merge), Ok(true));
    assert_eq!(repo.code_change(&end, &merge), Ok(None));
    repo.remove();
}

#[test]
fn a_base_move_after_two_merge_bases_with_the_base_counts() {
    let (repo, _) = Repo::with_pr("criss-cross-move");
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    repo.advance_main("a.md", text);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let main = repo.advance_main("m.txt", "m\n");
    let pr = repo.commit("c.txt", "c\n");
    repo.git(&["merge", "--quiet", "--no-edit", &main]);
    let end = repo.commit("a.md", &format!("fn unreviewed() {{}}\n{text}"));
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-edit", &pr]);
    repo.git(&["mv", "a.md", "a.rs"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    let base = repo.head();
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(
        repo.git(&["merge-base", "--all", &end, &base])
            .lines()
            .count(),
        2
    );
    assert_eq!(repo.passes(&end, &repo.head()), Ok(false));
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some(
            "has `a.md`, which the PR changes and the base moves into the code file \
             `a.rs`"
                .to_string()
        ))
    );
    repo.remove();
}

#[test]
fn a_base_move_that_only_one_of_two_merge_bases_shows_counts() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let unreviewed = format!("fn unreviewed() {{}}\n{text}");
    // `git merge-base --all` lists the bases by commit date, so each order is run.
    for main_first in [false, true] {
        let (repo, _) = Repo::with_pr(&format!("criss-cross-one-{main_first}"));
        let commit_at = |date: &str, file: &str, text: &str| {
            std::fs::write(repo.dir.join(file), text).unwrap();
            repo.git(&["add", file]);
            let status = repo
                .command()
                .env("GIT_COMMITTER_DATE", date)
                .args(["commit", "--quiet", "-m", file])
                .status()
                .unwrap();
            assert!(status.success());
            repo.head()
        };
        let (early, late) = ("@946684800 +0000", "@946771200 +0000");
        let (main_date, pr_date) = if main_first {
            (early, late)
        } else {
            (late, early)
        };
        repo.advance_main("a.md", &unreviewed);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let pr = commit_at(pr_date, "c.txt", "c\n");
        repo.git(&["switch", "--quiet", "main"]);
        let main = commit_at(main_date, "a.md", &format!("{text}main\n"));
        repo.git(&["merge", "--quiet", "--no-edit", &pr]);
        repo.git(&["mv", "a.md", "a.rs"]);
        repo.git(&["commit", "--quiet", "-m", "mv"]);
        let base = repo.head();
        repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.git(&["switch", "--quiet", "pr"]);
        repo.git(&["merge", "--quiet", "--no-edit", &main]);
        let end = repo.commit("a.md", &unreviewed);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        assert_eq!(
            repo.git(&["merge-base", "--all", &end, &base])
                .lines()
                .count(),
            2
        );
        assert_eq!(
            repo.code_change(&end, &repo.head()),
            Ok(Some(
                "has `a.md`, which the PR changes and the base moves into the code file \
                 `a.rs`"
                    .to_string()
            )),
            "{main_first}"
        );
        repo.remove();
    }
}

#[test]
fn a_base_move_of_a_path_that_only_a_merged_branch_changes_does_not_count() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let (repo, _) = Repo::with_pr("move-stacked");
    let old = repo.advance_main("a.md", text);
    // The branch `x` changes `a.md`; the PR is stacked on it.
    repo.git(&["switch", "--quiet", "-c", "x", &old]);
    let x = repo.commit("a.md", &format!("{text}x\n"));
    repo.git(&["switch", "--quiet", "-C", "pr", &x]);
    let from = repo.commit("b.txt", "pr\n");
    // The PR merges main before `x` lands.
    let m0 = repo.advance_main("c.txt", "main\n");
    repo.git(&["merge", "--quiet", "--no-edit", &m0]);
    let first = repo.head();
    // `x` lands on main, and then main moves `a.md` to `a.rs`.
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-edit", &x]);
    repo.git(&["mv", "a.md", "a.rs"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    let base = repo.head();
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.head();
    // The PR's tree is the base's tree and `b.txt`: the PR changes no code.
    assert_eq!(
        repo.git(&["diff", "--name-only", &base, &end]),
        "b.txt".to_string()
    );
    assert_eq!(
        repo.git(&["merge-base", "--all", &first, &base])
            .lines()
            .count(),
        2
    );
    assert_eq!(repo.code_change(&from, &end), Ok(None));
    repo.remove();
}

#[test]
fn a_base_move_after_a_modify_delete_of_two_merge_bases_counts() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let unreviewed = format!("fn unreviewed() {{}}\n{text}");
    let repo = Repo::new("move-modify-delete");
    let o = repo.commit_tree(&[(b"a.txt", "base\n"), (b"a.md", text)], &[]);
    // The PR changes `a.md`; main removes it.
    let b1 = repo.commit_tree(&[(b"a.txt", "base\n"), (b"a.md", &unreviewed)], &[&o]);
    let b2 = repo.commit_tree(&[(b"a.txt", "base\n"), (b"m.txt", "m\n")], &[&o]);
    // The PR merges main and keeps its `a.md`.
    let first = repo.commit_tree(
        &[
            (b"a.txt", "base\n"),
            (b"a.md", &unreviewed),
            (b"m.txt", "m\n"),
        ],
        &[&b1, &b2],
    );
    // Main merges the PR's commit and keeps `a.md` removed, then adds `a.rs`.
    let m = repo.commit_tree(&[(b"a.txt", "base\n"), (b"m.txt", "m\n")], &[&b2, &b1]);
    let base = repo.commit_tree(
        &[(b"a.txt", "base\n"), (b"m.txt", "m\n"), (b"a.rs", text)],
        &[&m],
    );
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["reset", "--quiet", "--hard", &first]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.head();
    assert_eq!(
        repo.git(&["merge-base", "--all", &first, &base])
            .lines()
            .count(),
        2
    );
    // The merge moves the PR's text of `a.md` into the code file `a.rs`.
    assert_eq!(
        repo.git(&["show", "HEAD:a.rs"]).lines().next(),
        Some("fn unreviewed() {}")
    );
    assert_eq!(
        repo.code_change(&first, &end),
        Ok(Some(
            "has `a.md`, which the PR changes and the base moves into the code file \
             `a.rs`"
                .to_string()
        ))
    );
    assert_eq!(repo.passes(&first, &end), Ok(false));
    repo.remove();
}

/// A commit of `files` with `parents`, at one fixed time for every commit.
fn at_one_time(repo: &Repo, files: &[(&[u8], &str)], parents: &[&str]) -> String {
    let loose = repo.commit_tree(files, &[]);
    let tree = repo.git(&["rev-parse", &format!("{loose}^{{tree}}")]);
    let mut command = repo.command();
    command
        .env("GIT_AUTHOR_DATE", "@1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "@1700000000 +0000")
        .args(["commit-tree", &tree, "-m", "c"]);
    for parent in parents {
        command.args(["-p", parent]);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn a_base_move_of_a_file_that_the_bases_move_aside_is_a_move_of_code() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let unreviewed = format!("fn unreviewed() {{}}\n{text}");
    let aside: &[u8] = b"d~Temporary merge branch 1";
    for graph in [false, true] {
        let repo = Repo::new(&format!("move-aside-{graph}"));
        let o = at_one_time(&repo, &[(b"a.txt", "base\n")], &[]);
        // One base adds the file `d`, the other the directory `d`: the merge of the
        // two bases moves the file aside, to a path that the order of the bases picks.
        let b1 = at_one_time(&repo, &[(b"a.txt", "base\n"), (b"d", text)], &[&o]);
        let b2 = at_one_time(&repo, &[(b"a.txt", "base\n"), (b"d/k", "k\n")], &[&o]);
        let first = at_one_time(
            &repo,
            &[(b"a.txt", "base\n"), (aside, &unreviewed), (b"d/k", "k\n")],
            &[&b1, &b2],
        );
        let m = at_one_time(
            &repo,
            &[(b"a.txt", "base\n"), (aside, text), (b"d/k", "k\n")],
            &[&b2, &b1],
        );
        let base = at_one_time(
            &repo,
            &[(b"a.txt", "base\n"), (b"x.rs", text), (b"d/k", "k\n")],
            &[&m],
        );
        repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.git(&["reset", "--quiet", "--hard", &first]);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let end = repo.head();
        if graph {
            repo.git(&["commit-graph", "write", "--reachable"]);
        }
        assert_eq!(
            repo.git(&["show", "HEAD:x.rs"]).lines().next(),
            Some("fn unreviewed() {}")
        );
        assert_eq!(repo.code_change(&first, &end), Ok(None), "{graph}");
        assert_eq!(repo.passes(&first, &end), Ok(true), "{graph}");
        repo.remove();
    }
}

#[test]
fn a_base_move_from_code_or_to_text_does_not_count() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    for (old, new) in [("a.md", "b.md"), ("a.rs", "b.rs")] {
        let (repo, _) = Repo::with_pr(&format!("move-{new}"));
        repo.advance_main(old, text);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let end = repo.commit(old, &format!("// pr\n{text}"));
        repo.git(&["switch", "--quiet", "main"]);
        repo.git(&["mv", old, new]);
        repo.git(&["commit", "--quiet", "-m", "mv"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["switch", "--quiet", "pr"]);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let merge = repo.head();
        assert_eq!(
            repo.git(&["show", &format!("HEAD:{new}")]).lines().next(),
            Some("// pr")
        );
        assert_eq!(repo.passes(&end, &merge), Ok(true), "{new}");
        assert_eq!(repo.code_change(&end, &merge), Ok(None), "{new}");
        repo.remove();
    }
}

#[test]
fn a_base_move_of_text_that_the_pr_does_not_change_does_not_count() {
    let (repo, _) = Repo::with_pr("move-unchanged");
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    repo.advance_main("a.md", text);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("b.md", "b\n");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["mv", "a.md", "a.rs"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    assert_eq!(repo.passes(&end, &merge), Ok(true));
    assert_eq!(repo.code_change(&end, &merge), Ok(None));
    repo.remove();
}

#[test]
fn a_conflict_of_the_start_and_the_base_fails_closed() {
    let (repo, _) = Repo::with_pr("merge-closed");
    repo.advance_main("x.rs", "fn a() {}\n// old\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("x.rs", "fn a() {}\n// pr\n");
    repo.commit("x.rs", "fn a() {}\n// old\n");
    repo.advance_main("x.rs", "fn a() {}\n// base\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    let first = repo.git(&["rev-parse", "HEAD^1"]);
    assert_eq!(repo.passes(&first, &merge), Ok(true));
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some(
            "has a conflict in `x.rs` between its start and the base".to_string()
        ))
    );
    repo.remove();
}

#[test]
fn the_base_after_the_last_merge_does_not_count() {
    let (repo, end) = Repo::with_pr("merge-later-base");
    repo.advance_main("a.rs", "fn a() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.advance_main("c.rs", "fn c() {}\n");
    assert_eq!(repo.code_change(&end, &repo.head()), Ok(None));
    repo.remove();
}

#[test]
fn a_modify_delete_conflict_of_the_start_and_the_base_fails_closed() {
    let (repo, _) = Repo::with_pr("merge-moddel");
    repo.advance_main("x.rs", "fn a() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("x.rs", "fn a() {}\nfn pr() {}\n");
    repo.commit("x.rs", "fn a() {}\n");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["rm", "--quiet", "x.rs"]);
    repo.git(&["commit", "--quiet", "-m", "rm"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    let first = repo.git(&["rev-parse", "HEAD^1"]);
    assert_eq!(repo.passes(&first, &merge), Ok(true));
    // The PR brings back the file that the base deleted.
    repo.commit("x.rs", "fn a() {}\nfn pr() {}\n");
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some(
            "has a conflict in `x.rs` between its start and the base".to_string()
        ))
    );
    repo.remove();
}

#[test]
fn a_binary_conflict_of_the_start_and_the_base_fails_closed() {
    let (repo, _) = Repo::with_pr("merge-binary");
    repo.advance_main("x.rs", "// \0\nfn old() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("x.rs", "// \0\nfn pr() {}\n");
    repo.commit("x.rs", "// \0\nfn old() {}\n");
    repo.advance_main("x.rs", "// \0\nfn base() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    let first = repo.git(&["rev-parse", "HEAD^1"]);
    assert_eq!(repo.passes(&first, &merge), Ok(true));
    // The PR replaces the base's `fn base` with its own text.
    repo.commit("x.rs", "// \0\nfn pr() {}\n");
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some(
            "has a conflict in `x.rs` between its start and the base".to_string()
        ))
    );
    repo.remove();
}

#[test]
fn a_merge_of_a_main_made_of_merges_counts_by_its_resolution() {
    let (repo, end) = Repo::with_pr("merge-merges");
    for file in ["f.rs", "g.rs"] {
        repo.git(&["switch", "--quiet", "-c", file, "main"]);
        repo.commit(file, "fn f() {}\n");
        repo.git(&["switch", "--quiet", "main"]);
        repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", file]);
    }
    repo.git(&["update-ref", "refs/remotes/origin/main", "main"]);
    repo.git(&["switch", "--quiet", "pr"]);
    let docs = repo.commit("h.rs", "// H.\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(repo.code_change(&end, &repo.head()), Ok(None));
    assert_eq!(repo.code_change(&docs, &repo.head()), Ok(None));
    repo.commit("i.rs", "fn i() {}\n");
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some("changes code at `i.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_range_needs_the_base_ref() {
    let (repo, end) = Repo::with_pr("merge-no-base");
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    assert_eq!(
        repo.code_change(&format!("{end}^"), &end),
        Err(
            "git show-ref --verify --hash refs/remotes/origin/main: fatal: \
             'refs/remotes/origin/main' - not a valid ref"
                .to_string()
        )
    );
    repo.remove();
}

#[test]
fn a_merge_round_finds_code_before_and_after_the_merge() {
    let (repo, end) = Repo::with_pr("merge-around");
    repo.commit("b.rs", "fn b() {}\n");
    repo.advance_main("a.rs", "fn a() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merge = repo.head();
    repo.commit("c.rs", "// C.\nfn c() {}\n");
    assert_eq!(
        repo.code_change(&end, &merge),
        Ok(Some("changes code at `b.rs:1`".to_string()))
    );
    assert_eq!(
        repo.code_change(&merge, &repo.head()),
        Ok(Some("changes code at `c.rs:2`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_merge_of_a_branch_that_is_not_main_counts_whole() {
    let (repo, end) = Repo::with_pr("merge-other");
    repo.git(&["switch", "--quiet", "-c", "other", "main"]);
    let other = repo.commit("a.rs", "fn a() {}\n");
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", &other]);
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some("changes code at `a.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn reads_only_the_file_that_a_directory_of_the_same_name_replaces() {
    let (repo, _) = Repo::with_pr("dir");
    let from = repo.commit("x.rs", "// a\n");
    repo.git(&["rm", "--quiet", "x.rs"]);
    std::fs::create_dir(repo.dir.join("x.rs")).unwrap();
    let docs = repo.commit("x.rs/b.rs", "// b\n");
    assert_eq!(repo.code_change(&from, &docs), Ok(None));
    let code = repo.commit("x.rs/b.rs", "fn b() {}\n");
    assert_eq!(
        repo.code_change(&from, &code),
        Ok(Some("changes code at `x.rs/b.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn names_the_file_when_a_file_and_a_directory_of_its_name_trade_places() {
    let (repo, _) = Repo::with_pr("swap");
    let comments = "// c\n".repeat(10);
    let from = repo.commit("x.rs", &comments);
    repo.git(&["rm", "--quiet", "x.rs"]);
    std::fs::create_dir(repo.dir.join("x.rs")).unwrap();
    let dir = repo.commit("x.rs/b.rs", &format!("{comments}fn b() {{}}\n"));
    let named = Ok(Some("changes code at `x.rs/b.rs:11`".to_string()));
    assert_eq!(repo.code_change(&from, &dir), named);
    repo.git(&["rm", "--quiet", "-r", "x.rs"]);
    let file = repo.commit("x.rs", &comments);
    assert_eq!(repo.code_change(&dir, &file), named);
    repo.remove();
}

#[cfg(unix)]
#[test]
fn reads_a_link_that_a_file_replaces_as_code() {
    let (repo, _) = Repo::with_pr("link");
    std::os::unix::fs::symlink("// a", repo.dir.join("x.rs")).unwrap();
    repo.git(&["add", "x.rs"]);
    repo.git(&["commit", "--quiet", "-m", "link"]);
    let from = repo.head();
    std::fs::remove_file(repo.dir.join("x.rs")).unwrap();
    let end = repo.commit("x.rs", "// a\n");
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `x.rs`".to_string()))
    );
    repo.remove();
}

#[test]
fn an_order_file_config_does_not_hide_code() {
    let (repo, _) = Repo::with_pr("order");
    std::fs::write(repo.dir.join("order"), "x.rs/*\n*\n").unwrap();
    repo.git(&["config", "diff.orderFile", "order"]);
    let from = repo.commit("x.rs", "fn a() {}\n");
    repo.git(&["rm", "--quiet", "x.rs"]);
    std::fs::create_dir(repo.dir.join("x.rs")).unwrap();
    let end = repo.commit("x.rs/b.rs", "// b\n");
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `x.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_decomposed_name_does_not_hide_code() {
    let (repo, _) = Repo::with_pr("nfd");
    repo.git(&["config", "core.precomposeUnicode", "false"]);
    let name = "cafe\u{301}.rs";
    let from = repo.commit(name, "fn a() {}\n");
    let end = repo.commit(name, "fn b() {}\n");
    repo.git(&["config", "core.precomposeUnicode", "true"]);
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some(format!("changes code at `{name}:1`")))
    );
    repo.remove();
}

#[cfg(unix)]
#[test]
fn names_a_path_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let (repo, from) = Repo::with_pr("bytes");
    let blob = repo.git(&["rev-parse", "HEAD:b.txt"]);
    let info = [b"100644,", blob.as_bytes(), b",a\xff.rs"].concat();
    let status = repo
        .command()
        .args(["update-index", "--add", "--cacheinfo"])
        .arg(std::ffi::OsStr::from_bytes(&info))
        .status()
        .unwrap();
    assert!(status.success());
    repo.git(&["commit", "--quiet", "-m", "bytes"]);
    assert_eq!(
        repo.code_change(&from, &repo.head()),
        Err(
            "git diff: the path `a\u{fffd}.rs` is not UTF-8: invalid utf-8 sequence \
             of 1 bytes from index 1"
                .to_string()
        )
    );
    repo.remove();
}

#[test]
fn a_ref_named_like_a_short_blob_does_not_hide_code() {
    let (repo, _) = Repo::with_pr("short");
    let from = repo.commit("a.rs", "// a\n");
    let code = repo.commit("a.rs", "fn a() {}\n");
    let raw = repo.git(&["diff", "--raw", &from, &code]);
    let short = raw.split(' ').nth(3).unwrap().to_string();
    repo.git(&["tag", &short, &format!("{from}:a.rs")]);
    assert_eq!(
        repo.code_change(&from, &code),
        Ok(Some("changes code at `a.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_tag_named_like_a_commit_prefix_does_not_hide_code() {
    let (repo, _) = Repo::with_pr("tagfrom");
    let from = repo.commit("a.rs", "// a\n");
    let end = repo.commit("a.rs", "fn a() {}\n");
    repo.git(&["tag", &from[..8], &end]);
    assert_eq!(
        repo.code_change(&from[..8], &end[..8]),
        Ok(Some("changes code at `a.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_tag_named_like_an_end_prefix_does_not_pass() {
    let (repo, end) = Repo::with_pr("tagend");
    let head = repo.commit("a.rs", "fn a() {}\n");
    repo.git(&["tag", &end[..8], &head]);
    assert_eq!(repo.passes(&end[..8], &head), Ok(false));
    repo.remove();
}

#[test]
fn a_tag_named_like_the_base_does_not_put_a_commit_on_it() {
    let (repo, end) = Repo::with_pr("tagbase");
    repo.git(&["switch", "--quiet", "-c", "side", "origin/main"]);
    let side = repo.commit("a.rs", "fn unreviewed() {}\n");
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["tag", "origin/main", &side]);
    repo.git(&["merge", "--quiet", "--no-edit", &side]);
    assert_eq!(repo.passes(&end, &repo.head()), Ok(false));
    repo.remove();
}

#[test]
fn hex_after_a_full_sha_names_no_commit() {
    let (repo, end) = Repo::with_pr("longhex");
    for zeros in [1, 25] {
        let long = format!("{end}{}", "0".repeat(zeros));
        assert_eq!(repo.passes(&long, &end), Ok(false));
        assert_eq!(
            repo.code_change(&long, &end),
            Ok(Some(format!("has `{long}`, which names no commit")))
        );
    }
    repo.remove();
}

#[test]
fn a_prefix_of_two_commits_names_no_commit() {
    let (repo, _) = Repo::with_pr("twins");
    let tree = repo.git(&["hash-object", "-t", "tree", "/dev/null"]);
    let shas = [17902, 26499].map(|n| {
        let body = format!(
            "tree {tree}\nauthor t <t@t> 0 +0000\ncommitter t <t@t> 0 +0000\n\n{n}\n"
        );
        std::fs::write(repo.dir.join("body"), body).unwrap();
        repo.git(&["hash-object", "-t", "commit", "-w", "body"])
    });
    for sha in &shas {
        assert_eq!(&sha[..7], "303b29b");
        assert_eq!(repo.passes(&sha[..7], sha), Ok(false));
        assert_eq!(repo.passes(sha, sha), Ok(true));
    }
    repo.remove();
}

#[test]
fn a_local_main_does_not_put_a_commit_on_the_base() {
    let (repo, end) = Repo::with_pr("localmain");
    repo.git(&["switch", "--quiet", "-c", "side", "origin/main"]);
    let side = repo.commit("a.rs", "fn unreviewed() {}\n");
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["branch", "--force", "main", &side]);
    repo.git(&["merge", "--quiet", "--no-edit", &side]);
    assert_eq!(repo.passes(&end, &repo.head()), Ok(false));
    repo.remove();
}

#[test]
fn a_tag_does_not_stand_in_for_a_missing_base() {
    let (repo, end) = Repo::with_pr("nobase");
    repo.git(&["switch", "--quiet", "-c", "side", "main"]);
    let side = repo.commit("a.rs", "fn unreviewed() {}\n");
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", &side]);
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    repo.git(&["tag", "refs/remotes/origin/main", &side]);
    assert_eq!(
        repo.passes(&end, &repo.head()),
        Err(
            "git show-ref --verify --hash refs/remotes/origin/main: fatal: \
             'refs/remotes/origin/main' - not a valid ref"
                .to_string()
        )
    );
    repo.remove();
}

#[test]
fn a_directory_left_by_a_killed_run_does_not_break_a_test() {
    Repo::with_pr("stale");
    let (repo, end) = Repo::with_pr("stale");
    assert_eq!(repo.passes(&end, &end), Ok(true));
    let dir = repo.dir.clone();
    repo.remove();
    assert!(!dir.exists(), "remove left the directory {}", dir.display());
}

#[test]
fn a_failed_removal_fails_the_test() {
    let (repo, _) = Repo::with_pr("removed-twice");
    let dir = repo.dir.clone();
    std::fs::remove_dir_all(&dir).unwrap();
    let missing = std::fs::remove_dir_all(&dir).unwrap_err();
    let panic =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| repo.remove()))
            .unwrap_err();
    assert_eq!(
        panic.downcast_ref::<String>(),
        Some(&format!("remove {}: {missing}", dir.display()))
    );
}

#[test]
fn an_inherited_git_environment_does_not_reach_another_repository() {
    let other = Repo::new("inherited");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "review::history::tests::passes_through_clean_merges_of_main",
        ])
        .env("GIT_DIR", other.dir.join(".git"))
        .env("GIT_CONFIG_PARAMETERS", "'commit.gpgsign'='true'")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1 passed"), "{output:?}");
    let refs = other.command().arg("show-ref").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&refs.stdout), "");
    other.remove();
}

#[test]
fn a_union_resolution_of_a_conflict_does_not_pass() {
    let (repo, _) = Repo::with_pr("union");
    let end = repo.commit("a.txt", "pr side\n");
    repo.advance_main("a.txt", "main side\n");
    repo.command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    std::fs::write(repo.dir.join("a.txt"), "pr side\nmain side\n").unwrap();
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "--quiet", "--no-edit"]);
    assert_eq!(repo.passes(&end, &repo.head()), Ok(false));
    repo.remove();
}

#[test]
fn a_merge_driver_in_the_tree_does_not_hide_a_conflict() {
    let (repo, _) = Repo::with_pr("tree-driver");
    repo.commit(".gitattributes", "* merge=union\n");
    let end = repo.commit("a.txt", "pr side\n");
    repo.advance_main("a.txt", "main side\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let merged = std::fs::read_to_string(repo.dir.join("a.txt")).unwrap();
    assert_eq!(merged, "pr side\nmain side\n");
    assert_eq!(repo.passes(&end, &repo.head()), Ok(false));
    repo.remove();
}

#[test]
fn passes_through_a_clean_merge_of_a_rename() {
    let (repo, _) = Repo::with_pr("reach-rename");
    let lines = (1..=10)
        .map(|n| format!("line {n}\n"))
        .collect::<Vec<_>>()
        .concat();
    let main = repo.advance_main("a.txt", &lines);
    repo.git(&["merge", "--quiet", "--no-edit", &main]);
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["mv", "a.txt", "moved.txt"]);
    repo.git(&["commit", "--quiet", "-m", "move"]);
    let moved = repo.head();
    repo.git(&["update-ref", "refs/remotes/origin/main", &moved]);
    repo.git(&["switch", "--quiet", "pr"]);
    let end = repo.commit("a.txt", &lines.replace("line 10", "line ten"));
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(repo.passes(&end, &repo.head()), Ok(true));
    repo.remove();
}

#[test]
fn the_machine_git_settings_do_not_change_a_test() {
    let home =
        std::env::temp_dir().join(format!("xtask-history-home-{}", std::process::id()));
    std::fs::create_dir_all(home.join("git")).unwrap();
    std::fs::write(home.join(".gitconfig"), "[diff]\n\trenames = false\n").unwrap();
    std::fs::write(home.join("git/ignore"), "Cargo.lock\n").unwrap();
    std::fs::write(home.join("git/attributes"), "* merge=union\n").unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "review::history::tests::a_union_resolution_of_a_conflict_does_not_pass",
            "review::history::tests::passes_through_a_clean_merge_of_a_rename",
            "review::history::tests::a_moved_file_and_a_manifest_change_code",
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&home).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("3 passed"), "{output:?}");
}

#[test]
fn reads_no_system_config_or_attributes_file() {
    let repo = Repo::new("system");
    for name in ["GIT_CONFIG_SYSTEM", "GIT_ATTR_SYSTEM"] {
        let output = command(&repo.dir).args(["var", name]).output().unwrap();
        // Exit 1 with no output is a known variable with no value; an unknown one
        // exits 129.
        assert_eq!(output.status.code(), Some(1), "{name}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "", "{name}");
    }
    repo.remove();
}

#[test]
fn a_last_merge_of_an_older_base_commit_reads_from_the_newest() {
    let (repo, from) = Repo::with_pr("merge-older-last");
    let older = repo.advance_main("c.rs", "fn c() {}\n");
    repo.advance_main("x.rs", "fn check() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.git(&["rm", "--quiet", "x.rs"]);
    repo.git(&["commit", "--quiet", "-m", "rm"]);
    let end = repo.git(&[
        "commit-tree",
        "HEAD^{tree}",
        "-p",
        "HEAD",
        "-p",
        &older,
        "-m",
        "merge",
    ]);
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `x.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_merge_of_the_pr_into_main_reads_from_main() {
    let (repo, from) = Repo::with_pr("merge-foxtrot");
    repo.git(&["switch", "--quiet", "-c", "q", "main"]);
    let q = repo.commit("q.rs", "fn q() {}\n");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", &q]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    let main = repo.advance_main("x.rs", "fn check() {}\n");
    // The head merges the PR into main, and its tree drops the base's `x.rs`.
    let tree = repo.git(&["merge-tree", "--write-tree", &from, &q]);
    let end = repo.git(&[
        "commit-tree",
        &tree,
        "-p",
        &main,
        "-p",
        &from,
        "-m",
        "merge",
    ]);
    assert_eq!(
        repo.code_change(&from, &end),
        Ok(Some("changes code at `x.rs:1`".to_string()))
    );
    repo.remove();
}

#[test]
fn a_file_directory_conflict_of_the_start_and_the_base_counts() {
    // A `~` in a directory name is not the one that `git` adds.
    for file in ["x.rs", "a~b/x.rs"] {
        let (repo, _) = Repo::with_pr(&format!("merge-dirfile-{}", file.len()));
        std::fs::create_dir_all(repo.dir.join(file)).unwrap();
        let end = repo.commit(&format!("{file}/a.txt"), "pr\n");
        repo.git(&["rm", "--quiet", "-r", file]);
        repo.git(&["commit", "--quiet", "-m", "rm dir"]);
        std::fs::create_dir_all(repo.dir.join(file).parent().unwrap()).unwrap();
        let main = repo.advance_main(file, "fn base() {}\n");
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        repo.git(&["rm", "--quiet", file]);
        repo.git(&["commit", "--quiet", "-m", "rm file"]);
        assert_eq!(
            repo.code_change(&end, &repo.head()),
            Ok(Some(format!(
                "has a conflict in `{file}~{main}` between its start and the base"
            ))),
            "{file}"
        );
        repo.remove();
    }
}

#[test]
fn a_file_moved_aside_to_a_taken_name_counts() {
    let (repo, _) = Repo::with_pr("merge-dirfile-taken");
    let base = repo.advance_main("x.rs", "fn base() {}\n");
    std::fs::create_dir(repo.dir.join("x.rs")).unwrap();
    repo.commit(&format!("x.rs~{base}"), "taken\n");
    let end = repo.commit("x.rs/a.txt", "pr\n");
    repo.git(&["rm", "--quiet", "-r", "x.rs", &format!("x.rs~{base}")]);
    repo.git(&["commit", "--quiet", "-m", "rm"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.git(&["rm", "--quiet", "x.rs"]);
    repo.git(&["commit", "--quiet", "-m", "rm x.rs"]);
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some(format!(
            "has a conflict in `x.rs~{base}_0` between its start and the base"
        )))
    );
    repo.remove();
}

#[test]
fn a_start_file_moved_aside_by_a_base_directory_counts() {
    let (repo, _) = Repo::with_pr("merge-dirfile-start");
    let end = repo.commit("x.rs", "fn pr() {}\n");
    repo.git(&["rm", "--quiet", "x.rs"]);
    repo.git(&["commit", "--quiet", "-m", "rm x.rs"]);
    std::fs::create_dir(repo.dir.join("x.rs")).unwrap();
    repo.advance_main("x.rs/a.txt", "base\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(
        repo.code_change(&end, &repo.head()),
        Ok(Some(format!(
            "has a conflict in `x.rs~{end}` between its start and the base"
        )))
    );
    repo.remove();
}

#[test]
fn a_merge_that_moves_the_pr_file_aside_counts() {
    let (repo, from) = Repo::with_pr("merge-dirfile-chain");
    let pr = repo.commit("x.rs", "fn pr() {}\n");
    repo.git(&["switch", "--quiet", "main"]);
    std::fs::create_dir(repo.dir.join("x.rs")).unwrap();
    let main = repo.commit("x.rs/a.txt", "base\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &main]);
    repo.git(&["switch", "--quiet", "pr"]);
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1), "{merge:?}");
    for entry in std::fs::read_dir(&repo.dir).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("x.rs~")
        {
            std::fs::remove_file(path).unwrap();
        }
    }
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "--quiet", "--no-edit"]);
    let merge = repo.head();
    assert_eq!(
        repo.code_change(&from, &merge),
        Ok(Some(format!(
            "resolves a conflict in `x.rs~{pr}` in `{merge}`"
        )))
    );
    repo.remove();
}

#[test]
fn the_base_merged_through_a_side_branch_does_not_count() {
    let (repo, from) = Repo::with_pr("merge-side");
    repo.advance_main("y.rs", "fn y() {}\n");
    repo.git(&["switch", "--quiet", "-c", "s", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", "s"]);
    assert_eq!(repo.code_change(&from, &repo.head()), Ok(None));
    repo.remove();
}

#[test]
fn a_range_that_holds_two_newest_base_commits_counts() {
    let (repo, from) = Repo::with_pr("merge-two-newest");
    repo.git(&["switch", "--quiet", "-c", "q", "main"]);
    let q = repo.commit("q.rs", "fn q() {}\n");
    repo.git(&["switch", "--quiet", "main"]);
    let older = repo.commit("c.txt", "c\n");
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", &q]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", &older]);
    repo.git(&["merge", "--quiet", "--no-edit", &q]);
    let mut newest = [older, q];
    newest.sort_unstable();
    assert_eq!(
        repo.code_change(&from, &repo.head()),
        Ok(Some(format!(
            "holds the base at more than one newest commit: `{}`",
            newest.join("`, `")
        )))
    );
    repo.remove();
}

#[test]
fn a_start_with_an_early_commit_time_reads() {
    let (repo, _) = Repo::with_pr("early-time");
    std::fs::write(repo.dir.join("a.txt"), "base, changed\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let output = repo
        .command()
        .env("GIT_COMMITTER_DATE", "@50 +0000")
        .args(["commit", "--quiet", "-m", "early"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let from = repo.head();
    repo.advance_main("c.txt", "c\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.head();
    assert_eq!(repo.code_change(&from, &end), Ok(None));
    assert_eq!(repo.passes(&from, &end), Ok(true));
    repo.remove();
}

#[test]
fn a_range_that_holds_two_newest_base_commits_has_that_phrase_before_a_conflict() {
    let (repo, _) = Repo::with_pr("two-newest-conflict");
    let from = repo.commit("a.rs", "// PR.\n");
    repo.git(&["switch", "--quiet", "-c", "q", "main"]);
    let q = repo.commit("q.txt", "q\n");
    repo.git(&["switch", "--quiet", "main"]);
    let older = repo.commit("a.rs", "// Main.\n");
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", &q]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    let merge = repo
        .command()
        .args(["merge", "--no-edit", &older])
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1), "{merge:?}");
    std::fs::write(repo.dir.join("a.rs"), "// PR.\n// Main.\n").unwrap();
    repo.git(&["commit", "--quiet", "--all", "--no-edit"]);
    repo.git(&["merge", "--quiet", "--no-edit", &q]);
    let mut newest = [older, q];
    newest.sort_unstable();
    assert_eq!(
        repo.code_change(&from, &repo.head()),
        Ok(Some(format!(
            "holds the base at more than one newest commit: `{}`",
            newest.join("`, `")
        )))
    );
    repo.remove();
}

/// The phrase of a change to the decision `x.md`.
const DECISION: &str = "changes the decision `docs/decisions/x.md`";

/// The part of each conflict phrase that names the decision `x.md`.
const CONFLICT: &str = "conflict in `docs/decisions/x.md`";

#[test]
fn a_range_that_adds_changes_or_removes_a_decision_is_a_decision_change() {
    let (repo, start) = Repo::with_pr("decision-decision");
    let added = repo.commit("docs/decisions/x.md", "a rule\n");
    assert_eq!(
        repo.decision_change(&start, &added),
        Ok(Some(DECISION.to_string()))
    );
    let changed = repo.commit("docs/decisions/x.md", "another rule\n");
    assert_eq!(
        repo.decision_change(&added, &changed),
        Ok(Some(DECISION.to_string()))
    );
    repo.git(&["rm", "--quiet", "docs/decisions/x.md"]);
    repo.git(&["commit", "--quiet", "-m", "remove"]);
    assert_eq!(
        repo.decision_change(&changed, &repo.head()),
        Ok(Some(DECISION.to_string()))
    );
    assert_eq!(
        repo.decision_change(&format!("{added}^"), &added),
        Ok(Some(DECISION.to_string()))
    );
    repo.remove();
}

#[test]
fn a_decision_path_in_another_case_is_a_decision_change() {
    let paths = [
        "docs/Decisions/x.md",
        "Docs/decisions/x.md",
        "DOCS/DECISIONS/x.md",
    ];
    for (n, path) in paths.into_iter().enumerate() {
        let (repo, start) = Repo::with_pr(&format!("decision-case-{n}"));
        let end = repo.commit(path, "a rule\n");
        assert_eq!(
            repo.decision_change(&start, &end),
            Ok(Some(DECISION.replace("docs/decisions/x.md", path)))
        );
        repo.remove();
    }
}

#[test]
fn a_path_with_a_letter_that_only_a_file_system_folds_is_no_decision_change() {
    // U+017F, the long s, which APFS reads as `s`.
    let paths = ["docs/deci\u{17f}ions/x.md", "doc\u{17f}/decisions/x.md"];
    for (n, path) in paths.into_iter().enumerate() {
        let (repo, start) = Repo::with_pr(&format!("decision-fold-{n}"));
        let end = repo.commit(path, "a rule\n");
        assert_eq!(repo.decision_change(&start, &end), Ok(None), "{path}");
        repo.remove();
    }
}

#[test]
fn a_range_that_changes_no_decision_is_no_decision_change() {
    let (repo, start) = Repo::with_pr("decision-none");
    repo.commit("docs/decisions.md", "a list\n");
    repo.commit("docs/x.md", "a doc\n");
    repo.commit("notes/docs/decisions/x.md", "a note\n");
    let end = repo.commit("a.rs", "fn a() {}\n");
    assert_eq!(repo.decision_change(&start, &end), Ok(None));
    repo.remove();
}

#[test]
fn an_empty_range_is_no_decision_change() {
    let (repo, _) = Repo::with_pr("decision-empty");
    let end = repo.commit("docs/decisions/x.md", "a rule\n");
    assert_eq!(repo.decision_change(&end, &end[..7]), Ok(None));
    repo.remove();
}

#[test]
fn a_decision_that_only_the_base_changes_is_no_decision_change() {
    let (repo, start) = Repo::with_pr("decision-base");
    repo.advance_main("docs/decisions/x.md", "a rule\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("b.txt", "changed\n");
    assert_eq!(repo.decision_change(&start, &end), Ok(None));
    repo.remove();
}

#[test]
fn a_merge_that_resolves_a_conflict_in_a_decision_is_a_decision_change() {
    let repo = Repo::new("decision-conflict");
    let base = repo.commit("docs/decisions/x.md", "a rule\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    let start = repo.commit("docs/decisions/x.md", "the rule of the PR\n");
    repo.advance_main("docs/decisions/x.md", "the rule of the base\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit("docs/decisions/x.md", "the rule of the base\n");
    assert_eq!(
        repo.decision_change(&start, &end),
        Ok(Some(format!("resolves a {CONFLICT} in `{end}`")))
    );
    repo.remove();
}

#[test]
fn a_decision_that_the_base_removes_and_the_range_keeps_is_a_decision_change() {
    let repo = Repo::new("decision-moddel");
    let base = repo.commit("docs/decisions/x.md", "a rule\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    let start = repo.commit("docs/decisions/x.md", "the rule of the PR\n");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["rm", "--quiet", "docs/decisions/x.md"]);
    repo.git(&["commit", "--quiet", "-m", "rm"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit("docs/decisions/x.md", "the rule of the PR\n");
    assert_eq!(
        repo.decision_change(&start, &end),
        Ok(Some(format!("resolves a {CONFLICT} in `{end}`")))
    );
    repo.remove();
}

#[test]
fn a_decision_that_the_range_removes_and_the_base_changes_is_a_decision_change() {
    let repo = Repo::new("decision-delmod");
    let base = repo.commit("docs/decisions/x.md", "a rule\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    repo.git(&["rm", "--quiet", "docs/decisions/x.md"]);
    repo.git(&["commit", "--quiet", "-m", "rm"]);
    let start = repo.head();
    repo.advance_main("docs/decisions/x.md", "the rule of the base\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit("docs/decisions/x.md", "the rule of the base\n");
    assert_eq!(
        repo.decision_change(&start, &end),
        Ok(Some(format!("resolves a {CONFLICT} in `{end}`")))
    );
    repo.remove();
}

#[test]
fn a_file_of_the_range_that_the_base_moves_into_the_decisions_is_a_decision_change() {
    let repo = Repo::new("decision-location");
    repo.commit("docs/drafts/a.md", "a draft\n");
    let base = repo.commit("docs/drafts/b.md", "another draft\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    let start = repo.commit("docs/drafts/x.md", "a rule of the PR\n");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["mv", "docs/drafts", "docs/decisions"]);
    repo.git(&["commit", "--quiet", "-m", "mv"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit("docs/decisions/x.md", "a rule of the PR\n");
    assert_eq!(
        repo.decision_change(&start, &end),
        Ok(Some(format!("resolves a {CONFLICT} in `{end}`")))
    );
    repo.remove();
}

#[test]
fn a_merge_that_drops_the_decision_change_of_the_range_is_a_decision_change() {
    let repo = Repo::new("decision-dropped");
    let base = repo.commit("docs/decisions/x.md", "a rule\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    let start = repo.commit("b.txt", "pr\n");
    repo.commit("docs/decisions/x.md", "the rule of the PR\n");
    repo.advance_main("docs/decisions/x.md", "the rule of the base\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit("docs/decisions/x.md", "the rule of the base\n");
    assert_eq!(
        repo.decision_change(&start, &end),
        Ok(Some(format!("resolves a {CONFLICT} in `{end}`")))
    );
    repo.remove();
}

#[test]
fn a_decision_conflict_of_the_start_and_the_base_is_a_decision_change() {
    let (repo, _) = Repo::with_pr("decision-closed");
    repo.advance_main("docs/decisions/x.md", "a rule\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let start = repo.commit("docs/decisions/x.md", "the rule of the PR\n");
    repo.commit("docs/decisions/x.md", "a rule\n");
    repo.advance_main("docs/decisions/x.md", "the rule of the base\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(
        repo.decision_change(&start, &repo.head()),
        Ok(Some(format!(
            "has a {CONFLICT} between its start and the base"
        )))
    );
    repo.remove();
}

#[test]
fn an_empty_range_at_two_newest_base_commits_is_no_decision_change() {
    let (repo, _) = Repo::with_pr("decision-empty-two-newest");
    repo.git(&["switch", "--quiet", "-c", "q", "main"]);
    let q = repo.commit("q.txt", "q\n");
    repo.git(&["switch", "--quiet", "main"]);
    let older = repo.commit("c.txt", "c\n");
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", &q]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", &older]);
    repo.git(&["merge", "--quiet", "--no-edit", &q]);
    let end = repo.head();
    assert_eq!(repo.decision_change(&end, &end), Ok(None));
    assert_eq!(repo.code_change(&end, &end), Ok(None));
    repo.remove();
}

#[test]
fn a_decision_range_end_that_names_no_commit_counts() {
    let (repo, end) = Repo::with_pr("decision-unnamed");
    assert_eq!(
        repo.decision_change("abc1234", &end),
        Ok(Some("has `abc1234`, which names no commit".to_string()))
    );
    assert_eq!(
        repo.decision_change(&end, "abc1234"),
        Ok(Some("has `abc1234`, which names no commit".to_string()))
    );
    repo.remove();
}

#[test]
fn a_decision_range_that_holds_two_newest_base_commits_counts() {
    let (repo, from) = Repo::with_pr("decision-two-newest");
    repo.git(&["switch", "--quiet", "-c", "q", "main"]);
    let q = repo.commit("q.txt", "q\n");
    repo.git(&["switch", "--quiet", "main"]);
    let older = repo.commit("c.txt", "c\n");
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", &q]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    repo.git(&["merge", "--quiet", "--no-edit", &older]);
    repo.git(&["merge", "--quiet", "--no-edit", &q]);
    let mut newest = [older, q];
    newest.sort_unstable();
    assert_eq!(
        repo.decision_change(&from, &repo.head()),
        Ok(Some(format!(
            "holds the base at more than one newest commit: `{}`",
            newest.join("`, `")
        )))
    );
    repo.remove();
}

/// A text long enough that `git` pairs a moved file that has it with its source.
const DRAFT: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";

impl Repo {
    /// Commits `DRAFT` to `old` on `main`, merges it into `pr`, changes it there,
    /// moves it to `new` on `main`, and merges `main` into `pr` again. Returns the
    /// merge that holds `old`, the commit that changes it, and the last merge.
    pub(in crate::review) fn move_on_main(
        &self,
        old: &str,
        new: &str,
    ) -> (String, String, String) {
        self.advance_main(old, DRAFT);
        self.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let held = self.head();
        let changed = self.commit(old, &format!("{DRAFT}the rule of the PR\n"));
        self.git(&["switch", "--quiet", "main"]);
        let path = self.dir.join(new);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        self.git(&["mv", old, new]);
        self.git(&["commit", "--quiet", "-m", "mv"]);
        self.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        self.git(&["switch", "--quiet", "pr"]);
        self.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        assert_eq!(
            self.git(&["show", &format!("HEAD:{new}")]),
            format!("{DRAFT}the rule of the PR")
        );
        (held, changed, self.head())
    }
}

#[test]
fn text_of_the_pr_that_the_base_moves_into_a_decision_is_a_decision_change() {
    let (repo, _) = Repo::with_pr("decision-move");
    let (held, changed, end) =
        repo.move_on_main("docs/drafts/x.md", "docs/decisions/x.md");
    assert_eq!(repo.git(&["show", "--remerge-diff", "--format=", &end]), "");
    assert_eq!(
        repo.decision_change(&changed, &end),
        Ok(Some(
            "has `docs/drafts/x.md`, which the PR changes and the base moves into the \
             decision `docs/decisions/x.md`"
                .to_string()
        ))
    );
    assert_eq!(
        repo.decision_change(&held, &end),
        Ok(Some(DECISION.to_string()))
    );
    repo.remove();
}

#[test]
fn a_base_move_is_a_decision_change_only_from_outside_the_decisions_into_them() {
    for (name, old, new) in [
        ("within", "docs/decisions/w.md", "docs/decisions/x.md"),
        ("outside", "docs/drafts/x.md", "docs/drafts/y.md"),
        ("out", "docs/decisions/x.md", "docs/drafts/x.md"),
    ] {
        let (repo, _) = Repo::with_pr(&format!("decision-move-{name}"));
        let (_, changed, end) = repo.move_on_main(old, new);
        assert_eq!(repo.decision_change(&changed, &end), Ok(None), "{name}");
        repo.remove();
    }
}

#[test]
fn the_directory_rename_setting_of_the_clone_changes_no_decision_change() {
    for value in ["conflict", "false", "true"] {
        let repo = Repo::new(&format!("decision-directory-{value}"));
        repo.commit("docs/drafts/a.md", "a draft\n");
        let base = repo.commit("docs/drafts/b.md", "another draft\n");
        repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.git(&["switch", "--quiet", "-c", "pr"]);
        let start = repo.commit("docs/drafts/x.md", "a rule of the PR\n");
        repo.git(&["switch", "--quiet", "main"]);
        repo.git(&["mv", "docs/drafts", "docs/decisions"]);
        repo.git(&["commit", "--quiet", "-m", "mv"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["switch", "--quiet", "pr"]);
        // With `true`, the merge moves the file of the PR with no conflict.
        let merge = ["merge", "--quiet", "--no-edit", "origin/main"];
        repo.git(&[&["-c", "merge.directoryRenames=true"][..], &merge].concat());
        let end = repo.head();
        assert_eq!(
            repo.git(&["show", &format!("{end}:docs/decisions/x.md")]),
            "a rule of the PR"
        );
        repo.git(&["config", "merge.directoryRenames", value]);
        assert_eq!(
            repo.decision_change(&start, &end),
            Ok(Some(format!("resolves a {CONFLICT} in `{end}`"))),
            "{value}"
        );
        repo.remove();
    }
}

#[test]
fn a_clone_with_rename_detection_off_reads_a_base_move_into_a_decision() {
    for key in ["merge.renames", "diff.renames"] {
        let (repo, _) = Repo::with_pr(&format!("decision-{key}"));
        let (_, changed, end) =
            repo.move_on_main("docs/drafts/x.md", "docs/decisions/x.md");
        repo.git(&["config", key, "false"]);
        assert_eq!(
            repo.decision_change(&changed, &end),
            Ok(Some(
                "has `docs/drafts/x.md`, which the PR changes and the base moves into \
                 the decision `docs/decisions/x.md`"
                    .to_string()
            )),
            "{key}"
        );
        repo.remove();
    }
}

#[test]
fn the_rename_limit_of_the_clone_changes_no_decision_change() {
    let phrase = "has `docs/drafts/x.md`, which the PR changes and the base moves into \
                  the decision `docs/decisions/y.md`";
    for key in ["merge.renameLimit", "diff.renameLimit"] {
        let (repo, _) = Repo::with_pr(&format!("decision-{key}"));
        let draft = DRAFT.repeat(4);
        let moved = draft.replacen("one\n", "ONE\n", 1);
        repo.advance_main("docs/drafts/x.md", &draft);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let changed =
            repo.commit("docs/drafts/x.md", &format!("{draft}the rule of the PR\n"));
        repo.git(&["switch", "--quiet", "main"]);
        std::fs::create_dir_all(repo.dir.join("docs/decisions")).unwrap();
        // Another file name: `git` pairs files of one name before it reads the limit.
        repo.git(&["mv", "docs/drafts/x.md", "docs/decisions/y.md"]);
        std::fs::write(repo.dir.join("docs/decisions/y.md"), &moved).unwrap();
        std::fs::write(repo.dir.join("c.txt"), "c\n").unwrap();
        repo.git(&["add", "docs/decisions/y.md", "c.txt"]);
        repo.git(&["commit", "--quiet", "-m", "mv"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["switch", "--quiet", "pr"]);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        assert_eq!(
            repo.git(&["show", "HEAD:docs/decisions/y.md"]),
            format!("{moved}the rule of the PR")
        );
        let end = repo.commit("docs/decisions/y.md", &moved);
        repo.git(&["config", key, "1"]);
        assert_eq!(
            repo.decision_change(&changed, &end),
            Ok(Some(phrase.to_string())),
            "{key}"
        );
        repo.remove();
    }
}

#[test]
fn a_probe_line_in_a_decision_of_the_base_is_no_move_into_the_decisions() {
    for (name, line) in [
        ("plain", "eleven"),
        ("probe", "xtask-review-probe 0"),
        ("start", "xtask-review-probe START 0"),
    ] {
        let (repo, _) = Repo::with_pr(&format!("decision-probe-{name}"));
        repo.advance_main("docs/decisions/x.md", DRAFT);
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        std::fs::create_dir_all(repo.dir.join("docs/drafts")).unwrap();
        repo.git(&["mv", "docs/decisions/x.md", "docs/drafts/x.md"]);
        repo.git(&["commit", "--quiet", "-m", "mv"]);
        let start = repo.head();
        let line = line.replace("START", &start);
        repo.advance_main("docs/decisions/x.md", &format!("{DRAFT}{line}\n"));
        repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
        let end = repo.head();
        assert_eq!(
            repo.git(&["ls-tree", "-r", "--name-only", &end]),
            "a.txt\nb.txt\ndocs/drafts/x.md"
        );
        assert_eq!(repo.decision_change(&start, &end), Ok(None), "{name}");
        repo.remove();
    }
}

#[test]
fn a_probe_line_in_a_decision_of_the_pr_is_no_move_into_the_decisions() {
    let (repo, _) = Repo::with_pr("decision-probe-first");
    let draft = DRAFT.repeat(4);
    repo.advance_main("docs/drafts/x.md", &draft);
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let second =
        repo.advance_main("docs/drafts/x.md", &draft.replacen("one\n", "ONE\n", 1));
    repo.git(&["mv", "docs/drafts/x.md", "docs/drafts/y.md"]);
    let start = repo.commit(
        "docs/decisions/z.md",
        &format!("{draft}xtask-review-probe {second} 0\n"),
    );
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.head();
    assert_eq!(
        repo.git(&["diff", "--name-only", &start, &end]),
        "docs/drafts/y.md"
    );
    assert_eq!(repo.decision_change(&start, &end), Ok(None));
    repo.remove();
}

/// The phrase of a change to the public surface of `path`.
fn item(path: &str) -> String {
    format!("changes a `pub` item or its doc in `{path}`")
}

#[test]
fn a_range_that_changes_a_pub_item_or_its_doc_changes_the_surface() {
    let (repo, start) = Repo::with_pr("surface-item");
    let lib = "crates/a/src/lib.rs";
    let added = repo.commit(lib, "/// A.\npub fn a() {}\n");
    assert_eq!(repo.surface_change(&start, &added), Ok(Some(item(lib))));
    let doc = repo.commit(lib, "/// A, changed.\npub fn a() {}\n");
    assert_eq!(repo.surface_change(&added, &doc), Ok(Some(item(lib))));
    let body = repo.commit(lib, "/// A, changed.\npub fn a() {\n    b();\n}\n");
    assert_eq!(repo.surface_change(&doc, &body), Ok(None));
    let private =
        repo.commit(lib, "/// A, changed.\npub fn a() {}\n\n/// B.\nfn b() {}\n");
    assert_eq!(repo.surface_change(&body, &private), Ok(None));
    let inner = "crates/a/src/b/c.rs";
    let nested = repo.commit(inner, "pub struct C;\n");
    assert_eq!(
        repo.surface_change(&private, &nested),
        Ok(Some(item(inner)))
    );
    repo.git(&["rm", "--quiet", lib]);
    repo.git(&["commit", "--quiet", "-m", "remove"]);
    assert_eq!(
        repo.surface_change(&nested, &repo.head()),
        Ok(Some(item(lib)))
    );
    repo.remove();
}

#[test]
fn a_pub_item_outside_the_source_of_a_crate_changes_no_surface() {
    let (repo, start) = Repo::with_pr("surface-outside");
    for path in [
        "crates/a/tests/it.rs",
        "crates/a/benches/b.rs",
        "crates/a/build.rs",
        "crates/src/a.rs",
        "crates/a/b/src/c.rs",
        "crates/a/src/lib.txt",
        "xtask/src/a.rs",
        "src/a.rs",
    ] {
        repo.commit(path, "pub fn a() {}\n");
    }
    assert_eq!(repo.surface_change(&start, &repo.head()), Ok(None));
    repo.remove();
}

#[test]
fn a_source_file_that_is_not_rust_changes_the_surface() {
    let (repo, start) = Repo::with_pr("surface-not-rust");
    let lib = "crates/a/src/lib.rs";
    let broken = repo.commit(lib, "fn a( {}\n");
    assert_eq!(repo.surface_change(&start, &broken), Ok(Some(item(lib))));
    let fixed = repo.commit(lib, "fn a() {}\n");
    assert_eq!(repo.surface_change(&broken, &fixed), Ok(Some(item(lib))));
    repo.remove();
}

#[test]
fn a_symlink_or_a_submodule_at_a_source_file_changes_the_surface() {
    let lib = "crates/a/src/lib.rs";
    for mode in ["120000", "160000"] {
        let (repo, from) = Repo::with_pr(&format!("surface-link-{mode}"));
        let link = |object: &str| {
            let info = format!("{mode},{object},{lib}");
            repo.git(&["update-index", "--add", "--cacheinfo", &info]);
            repo.git(&["commit", "--quiet", "-m", "link"]);
            repo.head()
        };
        // A target that is a Rust file with no `pub` item.
        std::fs::write(repo.dir.join("blob"), "fn a() {}").unwrap();
        let first = repo.git(&["hash-object", "-w", "blob"]);
        std::fs::write(repo.dir.join("blob"), "fn b() {}").unwrap();
        let second = repo.git(&["hash-object", "-w", "blob"]);
        let (first, second) = match mode {
            "120000" => (first, second),
            _ => (from.clone(), repo.git(&["rev-parse", "main"])),
        };
        let added = link(&first);
        assert_eq!(
            repo.surface_change(&from, &added),
            Ok(Some(item(lib))),
            "{mode}"
        );
        let moved = link(&second);
        assert_eq!(
            repo.surface_change(&added, &moved),
            Ok(Some(item(lib))),
            "{mode}"
        );
        repo.remove();
    }
}

#[test]
fn a_change_of_mode_alone_changes_no_surface() {
    let (repo, _) = Repo::with_pr("surface-mode");
    let lib = "crates/a/src/lib.rs";
    let added = repo.commit(lib, "pub fn a() {}\n");
    repo.git(&["update-index", "--chmod=+x", lib]);
    repo.git(&["commit", "--quiet", "-m", "mode"]);
    assert_eq!(repo.surface_change(&added, &repo.head()), Ok(None));
    repo.remove();
}

#[test]
fn a_merge_that_resolves_a_conflict_in_a_source_file_changes_the_surface() {
    let repo = Repo::new("surface-item-conflict");
    let lib = "crates/a/src/lib.rs";
    let base = repo.commit(lib, "fn a() {}\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    let start = repo.commit(lib, "fn a() { pr(); }\n");
    repo.advance_main(lib, "fn a() { base(); }\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit(lib, "fn a() { base(); }\n");
    assert_eq!(
        repo.surface_change(&start, &end),
        Ok(Some(format!("resolves a conflict in `{lib}` in `{end}`")))
    );
    repo.remove();
}

#[test]
fn a_source_file_that_the_base_removes_and_the_range_keeps_changes_the_surface() {
    let repo = Repo::new("surface-item-moddel");
    let lib = "crates/a/src/lib.rs";
    let base = repo.commit(lib, "pub fn a() {}\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    let start = repo.commit(lib, "pub fn a() {}\npub fn b() {}\n");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["rm", "--quiet", lib]);
    repo.git(&["commit", "--quiet", "-m", "rm"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["switch", "--quiet", "pr"]);
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit(lib, "pub fn a() {}\npub fn b() {}\n");
    assert_eq!(
        repo.surface_change(&start, &end),
        Ok(Some(format!("resolves a conflict in `{lib}` in `{end}`")))
    );
    repo.remove();
}

#[test]
fn a_source_file_conflict_of_the_start_and_the_base_changes_the_surface() {
    let (repo, _) = Repo::with_pr("surface-item-closed");
    let lib = "crates/a/src/lib.rs";
    repo.advance_main(lib, "fn a() {}\n// old\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let start = repo.commit(lib, "fn a() {}\n// pr\n");
    repo.commit(lib, "fn a() {}\n// old\n");
    repo.advance_main(lib, "fn a() {}\n// base\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    assert_eq!(
        repo.surface_change(&start, &repo.head()),
        Ok(Some(format!(
            "has a conflict in `{lib}` between its start and the base"
        )))
    );
    repo.remove();
}

#[test]
fn a_conflict_outside_a_source_file_changes_no_surface() {
    let repo = Repo::new("surface-conflict-other");
    let base = repo.commit("docs/x.md", "a doc\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &base]);
    repo.git(&["switch", "--quiet", "-c", "pr"]);
    let start = repo.commit("docs/x.md", "the doc of the PR\n");
    repo.advance_main("docs/x.md", "the doc of the base\n");
    let merge = repo
        .command()
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    let end = repo.commit("docs/x.md", "the doc of the base\n");
    assert_eq!(repo.surface_change(&start, &end), Ok(None));
    repo.remove();
}

#[test]
fn a_pub_item_that_only_the_base_changes_changes_no_surface() {
    let (repo, start) = Repo::with_pr("surface-item-base");
    repo.advance_main("crates/a/src/lib.rs", "pub fn a() {}\n");
    repo.git(&["merge", "--quiet", "--no-edit", "origin/main"]);
    let end = repo.commit("crates/b/src/lib.rs", "fn b() {}\n");
    assert_eq!(repo.surface_change(&start, &end), Ok(None));
    repo.remove();
}

#[test]
fn names_the_first_path_of_two_surface_changes() {
    let (repo, start) = Repo::with_pr("surface-first");
    repo.commit("docs/decisions/x.md", "a rule\n");
    repo.commit("crates/c/src/lib.rs", "pub fn c() {}\n");
    repo.commit("crates/b/src/lib.rs", "pub fn b() {}\n");
    let end = repo.commit("crates/a/src/lib.rs", "fn a() {}\n");
    assert_eq!(
        repo.surface_change(&start, &end),
        Ok(Some(item("crates/b/src/lib.rs")))
    );
    repo.remove();
}

#[test]
fn a_source_path_in_another_case_changes_the_surface() {
    let (repo, start) = Repo::with_pr("surface-case");
    let lib = "Crates/A/SRC/Lib.RS";
    let end = repo.commit(lib, "pub fn a() {}\n");
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", &end]),
        format!("{lib}\na.txt\nb.txt")
    );
    assert_eq!(repo.surface_change(&start, &end), Ok(Some(item(lib))));
    repo.remove();
}

#[test]
fn an_empty_range_changes_no_surface() {
    let (repo, _) = Repo::with_pr("surface-empty");
    let end = repo.commit("crates/a/src/lib.rs", "pub fn a() {}\n");
    assert_eq!(repo.surface_change(&end, &end[..7]), Ok(None));
    repo.remove();
}

#[test]
fn a_surface_range_that_names_no_commit_counts() {
    let (repo, end) = Repo::with_pr("surface-unnamed");
    assert_eq!(
        repo.surface_change("abc1234", &end),
        Ok(Some("has `abc1234`, which names no commit".to_string()))
    );
    assert_eq!(
        repo.surface_change(&end, "abc1234"),
        Ok(Some("has `abc1234`, which names no commit".to_string()))
    );
    repo.remove();
}

#[test]
fn text_of_the_pr_that_the_base_moves_into_a_source_file_changes_the_surface() {
    let (repo, _) = Repo::with_pr("surface-move");
    let (_, changed, end) = repo.move_on_main("docs/drafts/x.md", "crates/a/src/x.rs");
    assert_eq!(repo.git(&["show", "--remerge-diff", "--format=", &end]), "");
    assert_eq!(
        repo.surface_change(&changed, &end),
        Ok(Some(
            "has `docs/drafts/x.md`, which the PR changes and the base moves into the \
             source file `crates/a/src/x.rs`"
                .to_string()
        ))
    );
    repo.remove();
}
