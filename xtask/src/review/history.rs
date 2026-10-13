//! Which commits a review round covers, read from the history with `git`.

use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// The empty tree, the source of attributes for a merge, so no `.gitattributes` can
/// pick a merge driver that hides a conflict.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// The history of a git repository, and the branch that PRs in it merge into.
pub(crate) struct History<'a> {
    root: &'a Path,
    /// The full ref of the base, read only with `show-ref --verify`, so no other ref
    /// can take its place.
    base: String,
}

impl<'a> History<'a> {
    /// The history of the repository at `root`, where PRs merge into the branch
    /// `base` (for example `main`) of the remote `origin`.
    pub(crate) fn new(root: &'a Path, base: &str) -> Self {
        let base = format!("refs/remotes/origin/{base}");
        Self { root, base }
    }

    /// Where `end` (a SHA or a prefix of at least 7 digits) is on the first-parent
    /// chain of `head`. Each merge between them must be a merge of a commit on the
    /// base, else, and for text that names no single commit, it is [`Place::Off`]. Each
    /// such merge must also be clean, else it is [`Place::Unclean`]: its tree is the
    /// tree that `git merge-tree` makes of its parents, and its base side moves no text
    /// of its first parent into a code file, as in `code_change`.
    ///
    /// # Errors
    ///
    /// A failed `git` command, also when the base ref does not exist or `head` names
    /// no commit.
    pub(crate) fn place(&self, end: &str, head: &str) -> Result<Place, String> {
        let Some(end) = self.sha(end)? else {
            return Ok(Place::Off);
        };
        let base = self.base()?;
        let head =
            self.git(&["rev-parse", "--verify", &format!("{head}^{{commit}}")])?;
        let range = format!("{end}..{head}");
        let chain = self.git(&["rev-list", "--first-parent", "--parents", &range])?;
        let chain: Vec<_> = chain.lines().collect();
        let parent = chain.last().and_then(|line| line.split(' ').nth(1));
        if end != head && parent != Some(end.as_str()) {
            return Ok(Place::Off);
        }
        let (mut place, mut unclean) = (Place::Head, false);
        for line in chain {
            let commits: Vec<_> = line.split(' ').collect();
            if commits.len() == 2 {
                place = Place::Before;
                continue;
            }
            let Some((first, second)) = self.base_merge(line, &base)? else {
                return Ok(Place::Off);
            };
            unclean |= !self.clean(commits[0], first, second)?;
        }
        Ok(if unclean { Place::Unclean } else { place })
    }

    /// The first code change of `from..end`, as a phrase: "changes code at
    /// `<file>:<line>`", or "changes code at `<file>`" when no line differs. Each
    /// change to a file that is not a `.md` file is code (its lines, its mode, or its
    /// path), except a `.rs` line that, trimmed, is empty or starts with `//`. A moved
    /// file counts as removed and added.
    ///
    /// A merge of a commit on the base on the first-parent chain of `end` counts only
    /// by its resolution: a file that is not a `.md` file that `git merge-tree` finds a
    /// conflict in between its parents gives "resolves a conflict in `<file>` in
    /// `<merge>`". Else the change is read to `end` from the tree that `git merge-tree`
    /// makes of `from` and the newest base commit that `end` holds, not from `from`:
    /// the base's code does not count, and text of the range that the base moves into a
    /// code file does. A code file that this tree has a conflict in gives "has a
    /// conflict in `<file>` between its start and the base", so a conflict that leaves
    /// no markers fails closed too. A `.md` file that `from` changes since its merge
    /// base with that base commit, as the merge reads it, and that the merge that makes
    /// this tree moves into a code file by its own rename detection gives "has `<old>`,
    /// which the PR changes and the base moves into the code file `<new>`". An `end`
    /// that holds more than one newest base commit gives "holds the base at more than
    /// one newest commit: `<commit>`, `<commit>`", also when the range has a conflict.
    ///
    /// The line number is in `end` for an added line, and in `from` or that tree for
    /// a removed one.
    ///
    /// `None` when the range changes no code, also when it is empty. `from` and `end`
    /// are SHAs or prefixes of at least 7 digits, and `from` may end in one `^` for its
    /// first parent; text that names no single commit gives the phrase "has `<text>`,
    /// which names no commit".
    ///
    /// # Errors
    ///
    /// A failed `git` command, also when the base ref does not exist, or a changed
    /// path that is not UTF-8.
    pub(crate) fn code_change(
        &self,
        from: &str,
        end: &str,
    ) -> Result<Option<String>, String> {
        match self.range(from, end, &CODE)? {
            Range::Empty => Ok(None),
            Range::Counted(phrase) => Ok(Some(phrase)),
            Range::Diff { tree, end } => self.first_change(&tree, &end),
        }
    }

    /// The first change of a decision in the range `<from>..<end>`, as a phrase, or
    /// `None`, also when the range is empty. A file under `docs/decisions/` that the
    /// range adds, changes, or removes gives "changes the decision `<path>`". Here and
    /// below, `docs/decisions/` is in any ASCII case of those letters.
    ///
    /// The change is read to `end` from the tree that `git merge-tree` makes of `from`
    /// and the newest base commit that `end` holds, as `code_change` reads it: a change
    /// of the base does not count. A file under `docs/decisions/` that a merge of the
    /// base in the range resolves a conflict in gives "resolves a conflict in `<path>`
    /// in `<merge>`", and one that this tree has a conflict in gives "has a conflict in
    /// `<path>` between its start and the base", each also when the conflict leaves no
    /// markers or the range ends with the text of the base. A path outside
    /// `docs/decisions/` that `from` changes since its merge base with that base
    /// commit, and that this tree moves into `docs/decisions/`, gives "has `<old>`,
    /// which the PR changes and the base moves into the decision `<new>`", as
    /// `code_change` reads a move into a code file. `from` and `end` are as in
    /// `code_change`, with its two phrases: "has `<text>`, which names no commit", and
    /// "holds the base at more than one newest commit: `<commit>`, `<commit>`".
    ///
    /// # Errors
    ///
    /// A failed `git` command, also when the base ref does not exist.
    pub(crate) fn decision_change(
        &self,
        from: &str,
        end: &str,
    ) -> Result<Option<String>, String> {
        let (tree, end) = match self.range(from, end, &DECISION)? {
            Range::Empty => return Ok(None),
            Range::Counted(phrase) => return Ok(Some(phrase)),
            Range::Diff { tree, end } => (tree, end),
        };
        let changed = self
            .diff(&tree, &end)?
            .into_iter()
            .map(|entry| String::from_utf8_lossy(&entry.path).into_owned())
            .find(|path| (DECISION.counts)(path));
        Ok(changed.map(|path| format!("changes the decision `{path}`")))
    }

    /// The range `<from>..<end>`, as a rule that counts the paths of `kind` reads it.
    ///
    /// # Errors
    ///
    /// A failed `git` command, also when the base ref does not exist.
    fn range(&self, from: &str, end: &str, kind: &Kind) -> Result<Range, String> {
        let unnamed =
            |text: &str| Range::Counted(format!("has `{text}`, which names no commit"));
        let Some(from) = self.named(from)? else {
            return Ok(unnamed(from));
        };
        let Some(end) = self.sha(end)? else {
            return Ok(unnamed(end));
        };
        if from == end {
            return Ok(Range::Empty);
        }
        let base = self.base()?;
        let bases = self.git(&["merge-base", "--all", &end, &base])?;
        let mut bases: Vec<_> = bases.lines().collect();
        bases.sort_unstable();
        let [newest] = bases[..] else {
            return Ok(Range::Counted(format!(
                "holds the base at more than one newest commit: `{}`",
                bases.join("`, `")
            )));
        };
        if let Some((path, merge)) = self.resolved(&from, &end, &base, kind.counts)? {
            return Ok(Range::Counted(format!(
                "resolves a conflict in `{path}` in `{merge}`"
            )));
        }
        let merged = self.merged(&from, newest)?;
        if let Some(path) = merged.conflict(kind.counts) {
            return Ok(Range::Counted(format!(
                "has a conflict in `{path}` between its start and the base"
            )));
        }
        Ok(match self.moved(&from, newest, &merged.tree, kind)? {
            Some(moved) => Range::Counted(moved),
            None => Range::Diff {
                tree: merged.tree,
                end,
            },
        })
    }

    /// The first conflicted path for which `counts` holds in a merge of the base on
    /// the first-parent chain of `<from>..<end>`, and that merge. `base` is the SHA of
    /// the base.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn resolved(
        &self,
        from: &str,
        end: &str,
        base: &str,
        counts: impl Fn(&str) -> bool,
    ) -> Result<Option<(String, String)>, String> {
        let commits = format!("{from}..{end}");
        let chain = self.git(&[
            "rev-list",
            "--reverse",
            "--first-parent",
            "--parents",
            &commits,
        ])?;
        for line in chain.lines() {
            let Some((first, second)) = self.base_merge(line, base)? else {
                continue;
            };
            if let Some(path) = self.merged(first, second)?.conflict(&counts) {
                let merge = line.split(' ').next().unwrap_or_default();
                return Ok(Some((path.to_string(), merge.to_string())));
            }
        }
        Ok(None)
    }

    /// The first code change from the tree-ish `old` to `new`, as `code_change`
    /// gives it.
    ///
    /// # Errors
    ///
    /// A failed `git` command, or a changed path that is not UTF-8.
    fn first_change(&self, old: &str, new: &str) -> Result<Option<String>, String> {
        for entry in self.diff(old, new)? {
            let path = std::str::from_utf8(&entry.path).map_err(|e| {
                format!(
                    "git diff: the path `{}` is not UTF-8: {e}",
                    String::from_utf8_lossy(&entry.path)
                )
            })?;
            // A submodule's blob is a commit and a symlink's blob is its target, so
            // neither has lines to read.
            let link = [&entry.old_mode, &entry.new_mode]
                .iter()
                .any(|mode| [GITLINK, SYMLINK].contains(&mode.as_str()));
            if text_path(path) && !link {
                continue;
            }
            let rust = rust_path(path);
            if !link
                && let Some(line) = self.first_code(&entry.old, &entry.new, rust)?
            {
                return Ok(Some(format!("changes code at `{path}:{line}`")));
            }
            if !rust || link || entry.mode_changed() {
                return Ok(Some(format!("changes code at `{path}`")));
            }
        }
        Ok(None)
    }

    /// The line number of the first line of code that a change from the blob `old` to
    /// the blob `new` adds or removes, in `new` for an added line and in `old` for a
    /// removed one. A blob of zeros is a file that does not exist. `rust` tells
    /// whether the file is a `.rs` file.
    fn first_code(
        &self,
        old: &str,
        new: &str,
        rust: bool,
    ) -> Result<Option<u32>, String> {
        if absent(old) || absent(new) {
            let blob = if absent(old) { new } else { old };
            let text = self.output(&["cat-file", "blob", blob])?;
            let text = String::from_utf8_lossy(&text);
            return Ok((1..)
                .zip(text.lines())
                .find(|(_, l)| code(rust, l))
                .map(|(n, _)| n));
        }
        let diff = self.git(&[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--text",
            "--unified=0",
            "--inter-hunk-context=0",
            old,
            new,
        ])?;
        let (mut old, mut new) = (0, 0);
        let mut hunk = false;
        for line in diff.lines() {
            if let Some(header) = line.strip_prefix("@@ ") {
                hunk = true;
                (old, new) = starts(header)
                    .ok_or_else(|| format!("git diff: a bad hunk header `{line}`"))?;
            } else if hunk && let Some(text) = line.strip_prefix('-') {
                if code(rust, text) {
                    return Ok(Some(old));
                }
                old += 1;
            } else if hunk && let Some(text) = line.strip_prefix('+') {
                if code(rust, text) {
                    return Ok(Some(new));
                }
                new += 1;
            }
        }
        Ok(None)
    }

    /// The full SHA of the commit that `text` names when it is a SHA or a prefix of at
    /// least 7 digits, or `None`. One final `^` names the first parent of that commit,
    /// as the form `<first-fix>^..<head>` of a later round needs. A ref is never read,
    /// so a tag named like a prefix cannot take the commit's place.
    fn named(&self, text: &str) -> Result<Option<String>, String> {
        if let Some(child) = text.strip_suffix('^') {
            let Some(child) = self.sha(child)? else {
                return Ok(None);
            };
            let line = self.git(&["rev-list", "--parents", "-n", "1", &child])?;
            return Ok(line.split(' ').nth(1).map(str::to_string));
        }
        self.sha(text)
    }

    /// The full SHA of the commit that `text` names when it is a SHA or a prefix of at
    /// least 7 digits, or `None`.
    fn sha(&self, text: &str) -> Result<Option<String>, String> {
        if !(7..=40).contains(&text.len())
            || !text.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Ok(None);
        }
        let objects = self.git(&["rev-parse", &format!("--disambiguate={text}")])?;
        let mut commits = Vec::new();
        for object in objects.lines() {
            if self.git(&["cat-file", "-t", object])? == "commit" {
                commits.push(object);
            }
        }
        Ok(match commits[..] {
            [commit] => Some(commit.to_string()),
            _ => None,
        })
    }

    /// The SHA of the base, read only with `show-ref --verify`.
    fn base(&self) -> Result<String, String> {
        self.git(&["show-ref", "--verify", "--hash", &self.base])
    }

    /// The two parents of the commit in `line` (`<commit> <parent>...`, from
    /// `rev-list --parents`) when it is a merge of a commit on the base: it has two
    /// parents, and the second is an ancestor of `base`, the SHA of the base.
    fn base_merge<'l>(
        &self,
        line: &'l str,
        base: &str,
    ) -> Result<Option<(&'l str, &'l str)>, String> {
        let [_, first, second] = line.split(' ').collect::<Vec<_>>()[..] else {
            return Ok(None);
        };
        Ok(self.ancestor(second, base)?.then_some((first, second)))
    }

    /// Reports whether `commit` is an ancestor of the commit `of`, or is `of`.
    fn ancestor(&self, commit: &str, of: &str) -> Result<bool, String> {
        let status = command(self.root)
            .args(["merge-base", "--is-ancestor", commit, of])
            .status()
            .map_err(|e| format!("git merge-base: {e}"))?;
        match status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(format!(
                "git merge-base --is-ancestor {commit} {of}: {status}"
            )),
        }
    }

    /// Reports whether `merge` has the tree that a merge of `first` and `second`
    /// makes with no conflict, and the merge moves no text of `first` into a code
    /// file (`moved`).
    fn clean(&self, merge: &str, first: &str, second: &str) -> Result<bool, String> {
        let merged = self.merged(first, second)?;
        let tree = self.git(&["rev-parse", &format!("{merge}^{{tree}}")])?;
        Ok(merged.clean
            && merged.tree == tree
            && self.moved(first, second, &merged.tree, &CODE)?.is_none())
    }

    /// The phrase "has `<old>`, which the PR changes and the base moves into
    /// <noun>`<new>`" for the first path `<old>` that `kind` does not count, that
    /// `first` changes (`changed`), and that the merge of `first` and `second`, with
    /// the tree `tree`, moves into a path `<new>` that `kind` counts, where `<noun>` is
    /// `kind.noun`. The moves are the merge's own: `second` is merged again with a copy
    /// of `first` that gives each such path a probe text of its own, and the probe text
    /// is read in the files at such a `<new>` that the two merges make differently.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn moved(
        &self,
        first: &str,
        second: &str,
        tree: &str,
        kind: &Kind,
    ) -> Result<Option<String>, String> {
        // The merge is `first`, and moves nothing.
        if self.ancestor(second, first)? {
            return Ok(None);
        }
        let Some(probe) = self.probe(first, second, |path| !(kind.counts)(path))?
        else {
            return Ok(None);
        };
        let probed = self.merged(&probe.commit, second)?.tree;
        for entry in self.diff(tree, &probed)? {
            let new = String::from_utf8_lossy(&entry.path);
            if !(kind.counts)(&new) {
                continue;
            }
            let text = self.output(&["cat-file", "blob", &entry.new])?;
            let old = String::from_utf8_lossy(&text).lines().find_map(|line| {
                let n: usize = line.strip_prefix(&probe.mark)?.parse().ok()?;
                probe
                    .paths
                    .get(n)
                    .map(|old| String::from_utf8_lossy(old).into_owned())
            });
            if let Some(old) = old {
                return Ok(Some(format!(
                    "has `{old}`, which the PR changes and the base moves into \
                     {}`{new}`",
                    kind.noun
                )));
            }
        }
        Ok(None)
    }

    /// A copy of `first` in which each path of `changed(first, second, counts)` holds
    /// the probe text `<mark><n>`, where `paths[n]` is that path. `None` when no such
    /// path exists.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn probe(
        &self,
        first: &str,
        second: &str,
        counts: impl Fn(&str) -> bool,
    ) -> Result<Option<Probe>, String> {
        let changed = self.changed(first, second, counts)?;
        if changed.is_empty() {
            return Ok(None);
        }
        let mark = format!("{PROBE} {first} {second} ");
        let files: Vec<_> = changed
            .iter()
            .enumerate()
            .map(|(n, (mode, path))| File {
                mode,
                path,
                text: format!("{mark}{n}\n"),
            })
            .collect();
        Ok(Some(Probe {
            commit: self.copy(first, &files)?,
            mark,
            paths: changed.into_iter().map(|(_, path)| path).collect(),
        }))
    }

    /// The mode and path of each file for which `counts` holds that `first` changes
    /// since its merge base with `second`, as a merge of the two reads that base. A
    /// file that `first` moves since that base is in too, which changes no result: no
    /// file of the base has its path, so the base cannot move it.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn changed(
        &self,
        first: &str,
        second: &str,
        counts: impl Fn(&str) -> bool,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        // A merge of `first` with a copy of `second` that has no files builds the same
        // base. It gives a conflict with `first` as stage 2 for each file that the base
        // and `first` both hold with a difference, and for each file that `first`
        // moves.
        let gone = self.commit(second, EMPTY_TREE)?;
        let (_, output) = self.merge_tree(&[first, &gone])?;
        // `<tree>NUL`, then `<mode> <blob> <stage>TAB<path>NUL` for each stage of each
        // conflicted path, then `NUL` and the messages.
        let mut changed = Vec::new();
        for field in output
            .split(|&b| b == 0)
            .skip(1)
            .take_while(|field| !field.is_empty())
        {
            let mut parts = field.splitn(2, |&b| b == b'\t');
            let stage = String::from_utf8_lossy(parts.next().unwrap_or_default());
            let path = parts.next().unwrap_or_default();
            if let [mode, _, "2"] = stage.split(' ').collect::<Vec<_>>()[..]
                && counts(&String::from_utf8_lossy(path))
            {
                changed.push((mode.to_string(), path.to_vec()));
            }
        }
        Ok(changed)
    }

    /// A copy of `first` (`commit`) with its tree with `files` in place of its own.
    /// It builds the tree in an index file of its own, which it removes.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn copy(&self, first: &str, files: &[File<'_>]) -> Result<String, String> {
        // Each entry is `<mode> <blob>TAB<path>NUL`.
        let mut entries = Vec::new();
        for file in files {
            let blob =
                self.run(&["hash-object", "-w", "--stdin"], file.text.as_bytes(), &[])?;
            entries.extend_from_slice(file.mode.as_bytes());
            entries.push(b' ');
            entries.extend_from_slice(String::from_utf8_lossy(&blob).trim().as_bytes());
            entries.push(b'\t');
            entries.extend_from_slice(file.path);
            entries.push(0);
        }
        let name = format!("xtask-review-probe-{}", std::process::id());
        let index = self
            .root
            .join(self.git(&["rev-parse", "--git-path", &name])?);
        let env = [("GIT_INDEX_FILE", index.as_os_str())];
        let tree = self
            .run(&["read-tree", first], b"", &env)
            .and_then(|_| {
                self.run(&["update-index", "-z", "--index-info"], &entries, &env)
            })
            .and_then(|_| self.run(&["write-tree"], b"", &env));
        let removed = std::fs::remove_file(&index)
            .map_err(|e| format!("{}: {e}", index.display()));
        let tree = tree?;
        removed?;
        self.commit(first, String::from_utf8_lossy(&tree).trim())
    }

    /// A commit of `tree` with the parents and the commit time of `commit`, by a
    /// fixed author. A merge with it finds the same merge bases in the same order as a
    /// merge with `commit`, since the walk reads only parents and commit times, and
    /// so builds the same base from more than one.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn commit(&self, commit: &str, tree: &str) -> Result<String, String> {
        let line =
            self.git(&["show", "-s", "--date=raw", "--format=%cd%x00%P", commit])?;
        let (when, parents) = line.split_once('\0').unwrap_or_default();
        let mut args = vec!["commit-tree", tree, "-m", "probe"];
        for parent in parents.split_whitespace() {
            args.extend(["-p", parent]);
        }
        let who = OsStr::new("xtask");
        let when = format!("@{when}");
        let when = OsStr::new(&when);
        let commit = self.run(
            &args,
            b"",
            &[
                ("GIT_AUTHOR_NAME", who),
                ("GIT_AUTHOR_EMAIL", who),
                ("GIT_AUTHOR_DATE", when),
                ("GIT_COMMITTER_NAME", who),
                ("GIT_COMMITTER_EMAIL", who),
                ("GIT_COMMITTER_DATE", when),
            ],
        )?;
        Ok(String::from_utf8_lossy(&commit).trim().to_string())
    }

    /// The entries of `git diff --raw` from the tree-ish `old` to `new`, with no
    /// renames.
    ///
    /// # Errors
    ///
    /// A failed `git diff`, or an entry not in the raw format.
    fn diff(&self, old: &str, new: &str) -> Result<Vec<Entry>, String> {
        let raw = self.output(&[
            "diff",
            "--raw",
            "-z",
            "--no-abbrev",
            "--no-renames",
            old,
            new,
        ])?;
        // Each entry is `:<old mode> <new mode> <old blob> <new blob> <status>` and
        // the path, each ended by a NUL.
        let mut entries = Vec::new();
        let mut fields = raw.split(|&b| b == 0);
        while let (Some(entry), Some(path)) = (fields.next(), fields.next()) {
            let entry = String::from_utf8_lossy(entry);
            let [old_mode, new_mode, old, new, _] =
                entry.split(' ').collect::<Vec<_>>()[..]
            else {
                return Err(format!("git diff: a bad raw entry `{entry}`"));
            };
            entries.push(Entry {
                old_mode: old_mode.trim_start_matches(':').to_string(),
                new_mode: new_mode.to_string(),
                old: old.to_string(),
                new: new.to_string(),
                path: path.to_vec(),
            });
        }
        Ok(entries)
    }

    /// What `git merge-tree` makes of `first` and `second`.
    ///
    /// # Errors
    ///
    /// A failed `git merge-tree`.
    fn merged(&self, first: &str, second: &str) -> Result<Merged, String> {
        let (clean, output) = self.merge_tree(&["--name-only", first, second])?;
        // `<tree>NUL`, then `<path>NUL` for each conflicted path, then `NUL` and the
        // messages, which are not read: they can hold any bytes.
        let mut fields = output.split(|&b| b == 0);
        let tree = String::from_utf8_lossy(fields.next().unwrap_or_default());
        let conflicts = fields
            .take_while(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned());
        Ok(Merged {
            clean,
            tree: tree.into_owned(),
            conflicts: conflicts.collect(),
        })
    }

    /// Whether `git merge-tree --write-tree -z` with `args` finds no conflict, and its
    /// output. Commits with no common history merge from an empty base, as
    /// `git merge --allow-unrelated-histories` does.
    ///
    /// # Errors
    ///
    /// A failed `git merge-tree`.
    fn merge_tree(&self, args: &[&str]) -> Result<(bool, Vec<u8>), String> {
        let output = command(self.root)
            .arg(format!("--attr-source={EMPTY_TREE}"))
            .args([
                "merge-tree",
                "--write-tree",
                "-z",
                "--allow-unrelated-histories",
            ])
            .args(args)
            .output()
            .map_err(|e| format!("git merge-tree: {e}"))?;
        match output.status.code() {
            Some(0) => Ok((true, output.stdout)),
            Some(1) => Ok((false, output.stdout)),
            _ => Err(failure("merge-tree", &output.stderr)),
        }
    }

    /// The trimmed output of a `git` command that must succeed.
    fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = self.output(args)?;
        Ok(String::from_utf8_lossy(&output).trim().to_string())
    }

    /// The output of a `git` command that must succeed.
    fn output(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        self.run(args, b"", &[])
    }

    /// The output of a `git` command that must succeed, given `stdin` and the
    /// variables `env`.
    fn run(
        &self,
        args: &[&str],
        stdin: &[u8],
        env: &[(&str, &OsStr)],
    ) -> Result<Vec<u8>, String> {
        let spawned = |e| format!("git {}: {e}", args.join(" "));
        let mut child = command(self.root)
            .args(args)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(spawned)?;
        child
            .stdin
            .take()
            .expect("invariant: stdin is piped")
            .write_all(stdin)
            .map_err(spawned)?;
        let output = child.wait_with_output().map_err(spawned)?;
        if !output.status.success() {
            return Err(failure(&args.join(" "), &output.stderr));
        }
        Ok(output.stdout)
    }
}

/// The first word of each line of probe text that `History::probe` writes.
const PROBE: &str = "xtask-review-probe";

/// Where the end of a round is on the first-parent chain of a head
/// ([`History::place`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// The head, or a commit that only clean merges of the base follow.
    Head,
    /// A commit that clean merges of the base and commits that are not merges follow,
    /// with at least one of the second kind.
    Before,
    /// A commit that merges of the base and commits that are not merges follow, with at
    /// least one merge that is not clean.
    Unclean,
    /// Any other commit.
    Off,
}

/// The mode of a submodule.
const GITLINK: &str = "160000";

/// The mode of a symlink.
const SYMLINK: &str = "120000";

/// One entry of `git diff --raw`.
struct Entry {
    /// The mode in the old tree, all zeros when the path is added.
    old_mode: String,
    /// The mode in the new tree, all zeros when the path is removed.
    new_mode: String,
    /// The blob in the old tree, all zeros when the path is added.
    old: String,
    /// The blob in the new tree, all zeros when the path is removed.
    new: String,
    path: Vec<u8>,
}

impl Entry {
    /// Whether the path is in both trees, with a different mode.
    fn mode_changed(&self) -> bool {
        self.old_mode != self.new_mode
            && !absent(&self.old_mode)
            && !absent(&self.new_mode)
    }
}

/// Whether `field`, a mode or a blob of `git diff --raw`, is all zeros: the path is
/// not in that tree.
fn absent(field: &str) -> bool {
    field.bytes().all(|b| b == b'0')
}

/// The paths that a rule of the check counts.
struct Kind {
    /// Whether the rule counts this path, as `git` gives it.
    counts: fn(&str) -> bool,
    /// What a phrase calls a counted path that a move makes, with the space after it.
    noun: &'static str,
}

/// The code files: each path that is not a `.md` file.
const CODE: Kind = Kind {
    counts: |path| !text_path(path),
    noun: "the code file ",
};

/// The files under `docs/decisions/`, in any ASCII case of those letters: a file system
/// that folds case shows such a path as a decision.
const DECISION: Kind = Kind {
    counts: |path| {
        let decisions = "docs/decisions/";
        let start = path.get(..decisions.len());
        start.is_some_and(|start| start.eq_ignore_ascii_case(decisions))
    },
    noun: "the decision ",
};

/// A file to write in a tree.
struct File<'a> {
    mode: &'a str,
    path: &'a [u8],
    text: String,
}

/// A copy of a commit, with a probe text in each file that the commit changes and that
/// the [`Kind`] of the caller does not count.
struct Probe {
    commit: String,
    /// The start of each probe text: [`PROBE`] and the two merged commits. No file of
    /// either commit holds it, since it names the commit of that file.
    mark: String,
    /// The probed paths, by the number in their probe text.
    paths: Vec<Vec<u8>>,
}

/// What `git merge-tree` makes of two commits.
struct Merged {
    /// Whether no path conflicts.
    clean: bool,
    /// The merged tree, with conflict markers in each file whose content conflicts.
    tree: String,
    /// Each path with a conflict, in the order `git` gives, with each byte that is not
    /// UTF-8 replaced. A file that `git` moves aside has its new path, `<path>~<side>`,
    /// which is not a `.md` file.
    conflicts: Vec<String>,
}

impl Merged {
    /// The first conflicted path for which `counts` holds.
    fn conflict(&self, counts: impl Fn(&str) -> bool) -> Option<&str> {
        self.conflicts
            .iter()
            .map(String::as_str)
            .find(|path| counts(path))
    }
}

/// A range `<from>..<end>` of a round, as a rule reads it.
enum Range {
    /// `from` and `end` name the same commit.
    Empty,
    /// A range that counts for the rule whatever its diff holds, as a phrase: one that
    /// the check cannot read, one with a conflict in a path that the rule counts, or
    /// one in which the base moves a path of the PR into such a path.
    Counted(String),
    /// A range whose diff decides: from the tree of `from` and the newest base commit
    /// that `end` holds, to the commit `end`.
    Diff { tree: String, end: String },
}

/// Whether `path`, a path as `git` gives it, is a `.md` file, the one kind of file
/// whose change is never code.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "a `.MD` file is not text, so it fails closed"
)]
fn text_path(path: &str) -> bool {
    path.ends_with(".md")
}

/// Whether `path` is a `.rs` file: also the file `.rs`, which has no extension to
/// `Path`.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "a `.RS` file is code by any line, so it fails closed"
)]
fn rust_path(path: &str) -> bool {
    path.ends_with(".rs")
}

/// A `git` command in `root` that reads no inherited `GIT_*` variable and no global or
/// system config or attributes file, so the machine's settings cannot change a verdict
/// and no variable such as `GIT_DIR` can point it elsewhere. It still reads the
/// repository's own `.git/config` and `.git/info/attributes`, but not its
/// `merge.renames`, `merge.directoryRenames`, or `merge.renameLimit`, which have the
/// values of `git` with no setting.
fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .args(["-c", "core.attributesFile=/dev/null"])
        .args(["-c", "merge.renames=true"])
        .args(["-c", "merge.directoryRenames=conflict"])
        .args(["-c", "merge.renameLimit=7000"])
        // So a walk for merge bases reads only parents and commit times
        // (`History::commit`).
        .args(["-c", "core.commitGraph=false"]);
    command
}

/// Whether `text`, a changed line of a file that is not `.md`, is code: any line of
/// a file that is not `.rs`, and a `.rs` line that, trimmed, is not empty or a
/// comment.
fn code(rust: bool, text: &str) -> bool {
    let text = text.trim();
    !rust || !(text.is_empty() || text.starts_with("//"))
}

/// The first old and new line numbers of a hunk header `-<a>[,<n>] +<b>[,<m>] @@`.
fn starts(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split(' ');
    let mut start = |sign| {
        let part: &str = parts.next()?.strip_prefix(sign)?;
        part.split(',').next()?.parse().ok()
    };
    Some((start('-')?, start('+')?))
}

fn failure(command: &str, stderr: &[u8]) -> String {
    format!("git {command}: {}", String::from_utf8_lossy(stderr).trim())
}

#[cfg(test)]
pub(super) mod tests;
