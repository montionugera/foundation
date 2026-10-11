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
    /// chain of `head`. Each merge between them must be a clean merge of a commit on
    /// the base: its tree is the tree that `git merge-tree` makes of its parents, and
    /// its base side moves no text of its first parent into a code file, as in
    /// `code_change`. Else, and for text that names no single commit, it is
    /// [`Place::Off`].
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
        let mut place = Place::Head;
        for line in chain {
            let commits: Vec<_> = line.split(' ').collect();
            if commits.len() == 2 {
                place = Place::Before;
                continue;
            }
            let Some((first, second)) = self.base_merge(line, &base)? else {
                return Ok(Place::Off);
            };
            if !self.clean(commits[0], first, second)? {
                return Ok(Place::Off);
            }
        }
        Ok(place)
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
    /// one newest commit: `<commit>`, `<commit>`".
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
        let read = match self.range(from, end)? {
            Range::Empty => return Ok(None),
            Range::Unread(phrase) => return Ok(Some(phrase)),
            Range::Read(read) => read,
        };
        if let Some(conflict) = self.conflict(&read, "", |path| !text_path(path))? {
            return Ok(Some(conflict));
        }
        let tree = &read.merged.tree;
        if let Some(moved) = self.moved_into_code(&read.from, &read.newest, tree)? {
            return Ok(Some(moved));
        }
        self.first_change(tree, &read.end)
    }

    /// The first change of a decision in the range `<from>..<end>`, as a phrase, or
    /// `None`, also when the range is empty. A file under `docs/decisions/` that the
    /// range adds, changes, or removes gives "changes the decision `<path>`".
    ///
    /// The change is read to `end` from the tree that `git merge-tree` makes of `from`
    /// and the newest base commit that `end` holds, as `code_change` reads it: a
    /// change of the base does not count. A file under `docs/decisions/` that a merge
    /// of the base in the range resolves a conflict in gives "resolves a conflict in
    /// the decision `<path>` in `<merge>`", and one that this tree has a conflict in
    /// gives "has a conflict in the decision `<path>` between its start and the base",
    /// each also when the conflict leaves no markers or the range ends with the text of
    /// the base. `from` and `end` are as in `code_change`, with its two phrases: "has
    /// `<text>`, which names no commit", and "holds the base at more than one newest
    /// commit: `<commit>`, `<commit>`".
    ///
    /// # Errors
    ///
    /// A failed `git` command, also when the base ref does not exist.
    pub(crate) fn surface_change(
        &self,
        from: &str,
        end: &str,
    ) -> Result<Option<String>, String> {
        let read = match self.range(from, end)? {
            Range::Empty => return Ok(None),
            Range::Unread(phrase) => return Ok(Some(phrase)),
            Range::Read(read) => read,
        };
        let decision = |path: &str| path.starts_with(DECISIONS);
        if let Some(conflict) = self.conflict(&read, "the decision ", decision)? {
            return Ok(Some(conflict));
        }
        let decision = self
            .diff(&read.merged.tree, &read.end)?
            .into_iter()
            .find(|entry| entry.path.starts_with(DECISIONS.as_bytes()));
        Ok(decision.map(|entry| {
            let path = String::from_utf8_lossy(&entry.path);
            format!("changes the decision `{path}`")
        }))
    }

    /// The range `<from>..<end>`, as `code_change` and `surface_change` read it.
    ///
    /// # Errors
    ///
    /// A failed `git` command, also when the base ref does not exist.
    fn range(&self, from: &str, end: &str) -> Result<Range, String> {
        let unnamed =
            |text: &str| Range::Unread(format!("has `{text}`, which names no commit"));
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
            return Ok(Range::Unread(format!(
                "holds the base at more than one newest commit: `{}`",
                bases.join("`, `")
            )));
        };
        let newest = newest.to_string();
        let merged = self.merged(&from, &newest)?;
        Ok(Range::Read(Read {
            from,
            end,
            base,
            newest,
            merged,
        }))
    }

    /// The first conflict of `read` in a path for which `counts` holds, as a phrase
    /// that names the path after `kind`, or `None`: "resolves a conflict in
    /// <kind>`<path>` in `<merge>`" for a merge of the base in the range, then "has a
    /// conflict in <kind>`<path>` between its start and the base".
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn conflict(
        &self,
        read: &Read,
        kind: &str,
        counts: impl Fn(&str) -> bool,
    ) -> Result<Option<String>, String> {
        let resolved = self.resolved(&read.from, &read.end, &read.base, &counts)?;
        if let Some((path, merge)) = resolved {
            return Ok(Some(format!(
                "resolves a conflict in {kind}`{path}` in `{merge}`"
            )));
        }
        Ok(read.merged.conflict(counts).map(|path| {
            format!("has a conflict in {kind}`{path}` between its start and the base")
        }))
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
    /// file (`moved_into_code`).
    fn clean(&self, merge: &str, first: &str, second: &str) -> Result<bool, String> {
        let merged = self.merged(first, second)?;
        let tree = self.git(&["rev-parse", &format!("{merge}^{{tree}}")])?;
        Ok(merged.clean
            && merged.tree == tree
            && self.moved_into_code(first, second, &merged.tree)?.is_none())
    }

    /// The phrase "has `<old>`, which the PR changes and the base moves into the code
    /// file `<new>`" for the first path `<old>` of `changed(first, second)` that their
    /// merge, with the tree `tree`, moves into the code file `<new>`. The moves are the
    /// merge's own: `second` is merged again with a copy of `first` that gives each
    /// such path a probe text of its own, and the probe text is read in the code files
    /// that the two merges make differently.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn moved_into_code(
        &self,
        first: &str,
        second: &str,
        tree: &str,
    ) -> Result<Option<String>, String> {
        // The merge is `first`, and moves nothing.
        if self.ancestor(second, first)? {
            return Ok(None);
        }
        let Some(probe) = self.probe(first, second)? else {
            return Ok(None);
        };
        let probed = self.merged(&probe.commit, second)?.tree;
        for entry in self.diff(tree, &probed)? {
            let new = String::from_utf8_lossy(&entry.path);
            if text_path(&new) {
                continue;
            }
            let text = self.output(&["cat-file", "blob", &entry.new])?;
            let old = String::from_utf8_lossy(&text).lines().find_map(|line| {
                let n: usize =
                    line.strip_prefix(PROBE)?.strip_prefix(' ')?.parse().ok()?;
                probe
                    .paths
                    .get(n)
                    .map(|old| String::from_utf8_lossy(old).into_owned())
            });
            if let Some(old) = old {
                return Ok(Some(format!(
                    "has `{old}`, which the PR changes and the base moves into the \
                     code file `{new}`"
                )));
            }
        }
        Ok(None)
    }

    /// A copy of `first` in which each path of `changed(first, second)` holds the
    /// probe text `<PROBE> <n>`, where `paths[n]` is that path. `None` when no such
    /// path exists.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn probe(&self, first: &str, second: &str) -> Result<Option<Probe>, String> {
        let changed = self.changed(first, second)?;
        if changed.is_empty() {
            return Ok(None);
        }
        let files: Vec<_> = changed
            .iter()
            .enumerate()
            .map(|(n, (mode, path))| File {
                mode,
                path,
                text: format!("{PROBE} {n}\n"),
            })
            .collect();
        Ok(Some(Probe {
            commit: self.copy(first, &files)?,
            paths: changed.into_iter().map(|(_, path)| path).collect(),
        }))
    }

    /// The mode and path of each `.md` file that `first` changes since its merge base
    /// with `second`, as a merge of the two reads that base. A file that `first` moves
    /// since that base is in too, which changes no result: no file of the base has its
    /// path, so the base cannot move it into a code file.
    ///
    /// # Errors
    ///
    /// A failed `git` command.
    fn changed(
        &self,
        first: &str,
        second: &str,
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
                && text_path(&String::from_utf8_lossy(path))
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

/// The first word of each line of probe text that `History::moved_into_code` writes.
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
    /// Any other commit.
    Off,
}

/// The folder of the decision records, as the start of a path.
const DECISIONS: &str = "docs/decisions/";

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

/// A file to write in a tree.
struct File<'a> {
    mode: &'a str,
    path: &'a [u8],
    text: String,
}

/// A copy of a commit, with a probe text in each `.md` file that the commit changes.
struct Probe {
    commit: String,
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

/// A range `<from>..<end>` of a round.
enum Range {
    /// `from` and `end` name the same commit.
    Empty,
    /// The phrase of a range that the check cannot read: "has `<text>`, which names
    /// no commit" for the first of `from` and `end` that names none, or "holds the
    /// base at more than one newest commit: `<commit>`, `<commit>`".
    Unread(String),
    /// A range that the check reads.
    Read(Read),
}

/// A range `<from>..<end>` that the check reads.
struct Read {
    /// The SHA of `from`.
    from: String,
    /// The SHA of `end`.
    end: String,
    /// The SHA of the base.
    base: String,
    /// The newest commit of the base that `end` holds.
    newest: String,
    /// The merge of `from` and `newest`.
    merged: Merged,
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

/// A `git` command in `root` that reads no inherited `GIT_*` variable and no global
/// or system config or attributes file, so the machine's settings cannot change a
/// verdict and no variable such as `GIT_DIR` can point it elsewhere. It still reads
/// the repository's own `.git/config` and `.git/info/attributes`.
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
