- **REVIEW CHECK (2026-10-07)** The required status `review` (`cargo xtask review`,
  `.github/workflows/review.yaml`) passes a PR only when its review is done. It reads
  only round comments by the factory bot, in the format of `/review`, "Round comment".
  Each round comment parses, also an earlier one, and has its `Deferred:`,
  `Public surface:`, and `Hot path:` lines. Each round names the reviewers REVIEW TIERS
  requires, and `performance` when the first word of its `Hot path:` value is not
  `none`. Stated by the issue that the director's audits filed,
  https://github.com/synnaxlabs/foundation/issues/1467 (2026-10-07T15:20:38Z).
  The end lines are the last paragraph of the comment, in that order, as `/review`,
  "Round comment", writes them, each at the start of a line as GitHub shows it. So a
  line in a code block is not one, and a line that continues the paragraph with an
  indent is. Each end line may wrap onto the lines after it, and a paragraph after
  them fails. The first word of a value, with its backticks and one final comma,
  period, or semicolon removed, is the word that is checked. Decided by the director at
  2026-10-08T02:57:36Z
  (https://github.com/synnaxlabs/foundation/issues/1467#issuecomment-6051244793). The
  check reads the comment as GitHub does, by `comrak` (chosen by the person, as
  `laptop.monitor` relayed it at 2026-10-09T01:15:32Z,
  https://github.com/synnaxlabs/foundation/pull/1999#issuecomment-6072253988), with
  each tab in the spaces, tabs, and `>` at the start of a line replaced by spaces to the
  next multiple of 4 columns, and with the spaces and tabs at the end of a line kept: as
  CommonMark with the GitHub extensions tables, footnotes, task lists, strikethrough,
  and autolinks. Each field and end line is read in the text that GitHub shows on its
  line: with its escapes and entities read, without the marks of emphasis and code
  spans, with each image as one character that is not text, with no invisible character
  (Unicode default ignorable), and with each run of white space as one space. The check
  does not read a character as another character that looks the same, such as a Cyrillic
  letter in place of a Latin one. The tag filter acts only on raw HTML, which fails
  (corrected by the director at 2026-10-08T21:55:15Z,
  https://github.com/synnaxlabs/foundation/pull/1999#issuecomment-6069788449). A line
  ends at `\n`, `\r\n`, or a lone `\r` (decided by the director at 2026-10-08T04:42:33Z,
  https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6052414062). The round
  heading is the first top-level heading of level 2 whose text, read as a field is, is
  `Review round` or starts with `Review round `, and `<n>` is the rest of that text
  (decided by the director at 2026-10-09T04:03:10Z,
  https://github.com/synnaxlabs/foundation/pull/1999#issuecomment-6074014717). In a
  round posted after the cutoff with no such heading, it is the first such heading in
  the comment read as GitHub reads HTML blocks, by an older CommonMark. At the start of
  each line, after its indent and the marks of quotes, list items, and footnote labels,
  the name of each `search` tag is read as a name in no list of block tags, the name of
  each `source` tag is read as `option`, and `<!` and a lowercase letter start no HTML
  block. That spec lists `source` and not `search` as block tags, so GitHub starts a
  block at a `search` tag only when the line is one complete tag alone and does not
  continue a paragraph, and it needs an uppercase letter after `<!`. The fields are the
  first top-level block after it, and the end lines are the last one, each when it is a
  paragraph. A code block, an HTML block, a list, a quote, or a table is not a
  paragraph, so a list after the `Hot path:` line fails. A footnote with no reference is
  not shown, so its lines do not count, and of the footnotes of one label only the first
  is shown. The footnotes are the last blocks, as GitHub shows them. A round comment
  posted after the cutoff fails when it holds raw HTML outside a code span or a code
  block: an HTML block or inline HTML as `comrak` reads it, or a line of text whose
  source, after the indent and the marks of quotes, list items, and footnote labels,
  starts with `<` and a letter, `!`, `/`, or `?` and that is not an autolink, also when
  the line starts inside a code span, a link, or a link definition. GitHub reads some of
  these lines in a different way, and an open `<!--` or `<details>` hides the text after
  it. A footnote label here is `[^`, one or more characters other than `]`, space, or
  tab, then `]:`. It also fails when a span of text, as `comrak` reads it, holds `[^` on
  a line before the last line of its paragraph: GitHub can read a `]` on a later line as
  the end of a footnote reference and hide the text between them. It also fails when a
  footnote, as `comrak` reads it, is inside another footnote: GitHub shows the inner one
  as a footnote of its own, and can move text of the outer one into it, but `comrak`
  reads no reference to the inner one. It also fails when `comrak` places the text of a
  paragraph before the last line of the paragraph, as it does after a link or an image
  with a line break after its text and after a link reference definition in the
  paragraph, since each line that it then gives is wrong. The message names the first
  line with one of these causes, or the first line of the paragraph. A round comment
  that fails by these rules gets an edit that puts the line in a code span, or that
  writes each link and image on one line and puts a blank line after each link reference
  definition, or that moves each footnote out of another footnote, and the cutoff stays.
  A paragraph that `comrak` places in the wrong lines, and a footnote inside another
  footnote, fail in any round, also in one posted before the cutoff, since the check
  cannot read them. In an old round, a `Hot path:` line, or a `Reviewers:` line of a
  round that does not parse, counts where GitHub shows it at the start of a line of text
  of a paragraph, at any depth and any indent, except in a paragraph that `comrak`
  places in the wrong lines. A line of a code block or an HTML block does not count.
  Changed by https://github.com/synnaxlabs/foundation/issues/1783,
  https://github.com/synnaxlabs/foundation/issues/2037, and
  https://github.com/synnaxlabs/foundation/issues/2050, with the rulings of the director
  at 2026-10-08T21:47:24Z
  (https://github.com/synnaxlabs/foundation/pull/1999#issuecomment-6069679983),
  2026-10-09T00:29:41Z
  (https://github.com/synnaxlabs/foundation/issues/2050#issuecomment-6071765169), and
  2026-10-09T03:40:52Z
  (https://github.com/synnaxlabs/foundation/pull/1999#issuecomment-6073795680), approved
  by the director at 2026-10-09T05:20:36Z
  (https://github.com/synnaxlabs/foundation/pull/1999#issuecomment-6074829060). The
  failure on a footnote inside another footnote: ruled by the director at
  2026-10-12T23:07:13Z
  (https://github.com/synnaxlabs/foundation/pull/2495#issuecomment-6134334707).
  Supersedes the footnote label rule of
  https://github.com/synnaxlabs/foundation/issues/2037#issuecomment-6071274522
  (2026-10-08T23:46:02Z), since `comrak` reads footnote labels as GitHub does.
  Supersedes the code block and indent rules of
  https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6051923239,
  https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6052414062,
  https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6052814147,
  https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6053059328, and
  https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6053208426. The words
  on the spaces and tabs at the end of a line: approved by the director at
  2026-10-11T10:51:12Z
  (https://github.com/synnaxlabs/foundation/pull/2383#issuecomment-6108250288). A round
  comment posted before the cutoff `CUTOFF` in
  `xtask/src/review.rs` (2026-10-08T03:00:00Z) is checked as before, except that a
  paragraph that `comrak` places in the wrong lines, and a footnote inside another
  footnote, fail. Otherwise an earlier free-form round passes, and it needs no end
  lines. A `Hot path:` line that counts, as stated above, and names a function still
  needs `performance`. Decided by the director at 2026-10-08T02:44:00Z
  (https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6051099968).
  Supersedes the reviewers of a later round in ruling 2 of
  https://github.com/synnaxlabs/foundation/issues/1169#issuecomment-6040439732
  (2026-10-07T06:32:32Z). For this rule, an old round that parses names
  `performance` in its `Reviewers:` field, read as before. One that does not parse
  names it in any `Reviewers:` line of its text that counts, as stated above. A
  `Hot path:` line counts as stated above. Decided by the director at
  2026-10-08T04:42:33Z
  (https://github.com/synnaxlabs/foundation/pull/1752#issuecomment-6052414062).
  The last round finds none and ends at the head, or at a commit on the first-parent
  chain of the head after which each merge is a clean merge of the base
  (`git merge-tree`) and the range to the head changes no code, read as the range of a
  round with `Breaker: skipped` is read. When a commit that is not a merge follows it,
  its round comment has a line `Text fixes:` that is not `Text fixes: none`
  (TEXT FIXES). Decided by the director at 2026-10-10T23:01:29Z
  (https://github.com/synnaxlabs/foundation/issues/2315), 2026-10-10T23:21:22Z
  (https://github.com/synnaxlabs/foundation/issues/2315#issuecomment-6103264281), and
  2026-10-10T23:29:42Z
  (https://github.com/synnaxlabs/foundation/issues/2315#issuecomment-6103326270), on the
  person's approval at 2026-10-10T15:50:57Z
  (https://github.com/synnaxlabs/foundation/issues/1703#issuecomment-6103074621).
  Supersedes ruling 1 of
  https://github.com/synnaxlabs/foundation/issues/1169#issuecomment-6032179989
  (2026-10-07T06:15:46Z). The line counts when it starts a line of a top-level paragraph
  after the round heading, other than the end lines (the check reads no end lines in a
  round posted before the cutoff), and when the first word of its value, read as that of
  an end line, is not `none`. Decided by the director at 2026-10-11T00:54:34Z
  (https://github.com/synnaxlabs/foundation/pull/2319#issuecomment-6103952358) and
  2026-10-11T01:16:17Z
  (https://github.com/synnaxlabs/foundation/pull/2319#issuecomment-6104109296). A merge
  of the base is not clean when the base moves a path that the PR changed since their
  merge base, and that is not code, to a code path, by the rename detection of the
  merge. A base move of a path that the PR did not change stays clean. Decided by the
  director at 2026-10-07T17:21:14Z
  (https://github.com/synnaxlabs/foundation/issues/1496#issuecomment-6043078385). When
  the last round is a later round with `Breaker: skipped`, it fails if its range changes
  code: a change to a file that is not a `.md` file (its lines, its mode, or its path),
  other than a `.rs` line that, trimmed, is blank or starts with `//` (a doctest line is
  a comment) (decided by the director at 2026-10-10T23:01:29Z in
  https://github.com/synnaxlabs/foundation/issues/2315). Each line of a moved file
  counts as removed and added. A merge of the base in the range counts only by its
  resolution: a conflict that `git merge-tree` finds between its parents in a file that
  is not a `.md` file is a code change (decided by the director at 2026-10-10T23:01:29Z
  in https://github.com/synnaxlabs/foundation/issues/2315). The conflict sentence above
  supersedes that of
  https://github.com/synnaxlabs/foundation/issues/1496#issuecomment-6044222273
  (2026-10-07T18:27:30Z), and the code definition supersedes that of
  https://github.com/synnaxlabs/foundation/issues/1496#issuecomment-6043078385 and of
  the first REVIEW CHECK record
  (https://github.com/synnaxlabs/foundation/commit/34cc4124d), decided by the director
  on #1169 and in messages on #1193. The rest of the range is read from the tree that
  `git merge-tree` makes of its start and the newest base commit that its end holds, not
  from its start: the base's code does not count, and text that the range changes and
  the base moves into a code file does. So does a path that is not code, that the start
  changes since its merge base with that base commit, as the merge reads it, and that
  this merge moves into a code file, by the merge's own rename detection. A conflict in
  this tree in a code file is a code change, also one that leaves no markers, and so is
  an end that holds more than one newest base commit. In the range, a base move counts
  only through this tree. Decided by the director at 2026-10-07T18:27:30Z
  (https://github.com/synnaxlabs/foundation/issues/1496#issuecomment-6044222273).
  Supersedes the range sentence of
  https://github.com/synnaxlabs/foundation/issues/1496#issuecomment-6043078385. Found by
  the director at 2026-10-07T14:50:33Z
  (https://github.com/synnaxlabs/foundation/pull/1193#issuecomment-6040535575), fixed by
  #1451. An earlier round's `Breaker:` skip is taken as written, since a rebase can drop
  its range from the clone. An earlier round in the fixed format that does not parse
  fails. A red-team `oracle` PR also needs ``Director: approved at `<sha>` `` at the
  head. Decided by the director on #1169
  (https://github.com/synnaxlabs/foundation/issues/1169#issuecomment-6032179989) and in
  messages on #1193. On `merge_group`, the check runs for the PR of the group, at the
  head of that PR, and its result is the status of the group. A group whose ref names
  no PR gets the status `error`. Decided by the director at 2026-10-12T16:16:53Z
  (https://github.com/synnaxlabs/foundation/issues/2458). Supersedes the `merge_group`
  sentence of
  https://github.com/synnaxlabs/foundation/issues/1169#issuecomment-6032179989. Ruled by
  the director at 2026-10-12T18:28:50Z
  (https://github.com/synnaxlabs/foundation/issues/2458#issuecomment-6130199949). A
  round comment fails when it does not start with its name line
  `**<session>** · <role>`, then `Quality: <n>/10` with `n` from 1 to 10, then a
  summary line. Stated by the director at 2026-10-08T01:47:11Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6050487121), and
  ruled at 2026-10-11T10:03:22Z
  (https://github.com/synnaxlabs/foundation/pull/2383#issuecomment-6107851789). The
  check reads the lines of the top-level paragraphs with which the comment starts, as
  GitHub shows them. It stops at the round heading and at the first block that is not a
  paragraph, so a list, a quote, a code block, a table, or a heading between the three
  lines fails the round. The first block of the comment is a paragraph that starts with
  the session name in bold, then ` · `, then a role. The second line is the rating, with
  nothing after it. The third line is not empty. A blank line between two of them
  changes nothing. A session name, a role, or a summary line that holds only images and
  the spaces between them is empty. An emoji name such as `:octocat:` is text to the
  check, also one that GitHub shows as an image. Lost: a list of those names in the
  check, since GitHub can change it. Ruled by the director at 2026-10-11T11:24:24Z
  (https://github.com/synnaxlabs/foundation/pull/2383#issuecomment-6108509497) and
  2026-10-11T11:25:49Z
  (https://github.com/synnaxlabs/foundation/pull/2383#issuecomment-6108520648).
  Supersedes the sentence on an image of
  https://github.com/synnaxlabs/foundation/pull/2383#issuecomment-6107851789
  (2026-10-11T10:03:22Z). The check reads neither the session name nor the role against
  a list. A round posted before the cutoff `RATING_CUTOFF` in `xtask/src/review.rs`
  (2026-10-11T16:00:00Z) is checked as before, so that the new rule does not fail the
  rounds that came before it. The cutoff is a fixed time: it stays when the rule merges
  after it. A round that fails by this rule gets an edit of its round comment that adds
  the lines, with the rating and the summary of its `reviewer` report as given. When
  that report is lost, a new run of `reviewer` on the range of the round gives them, and
  the edit says so. A later round that does not name `architecture` fails when its range
  changes a file under `docs/decisions/`: one that the range adds, changes, or removes.
  The check reads `docs/decisions/` in any case of its letters, since a file system that
  folds case shows such a path as a decision. It folds only ASCII: a character such as
  U+017F, which APFS reads as `s`, is not a letter of the path to the check. Lost: a
  fold table in the check, since each file system has its own. Stated by the director at
  2026-10-08T22:50:06Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6070608950), and
  ruled at 2026-10-11T07:44:23Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6106799591),
  07:55:36Z
  (https://github.com/synnaxlabs/foundation/pull/2373#issuecomment-6106884396),
  08:19:09Z
  (https://github.com/synnaxlabs/foundation/pull/2373#issuecomment-6107062843),
  11:19:09Z
  (https://github.com/synnaxlabs/foundation/pull/2373#issuecomment-6108468045),
  11:24:26Z
  (https://github.com/synnaxlabs/foundation/pull/2373#issuecomment-6108509702), and
  12:05:01Z
  (https://github.com/synnaxlabs/foundation/pull/2373#issuecomment-6108831770).
  Supersedes the reviewers of a later round in ruling 2 of
  https://github.com/synnaxlabs/foundation/issues/1169#issuecomment-6040439732
  (2026-10-07T06:32:32Z). For this rule, a round comment after the first is a later
  round, whatever its number, and so is a first round comment whose number is above 1.
  Each later round is read, not only the last. The range from the last round to the head
  is not read for a decision. The check reads it only for code (TEXT FIXES), and the
  `review` skill gives a text fix in a decision no round. The range is read as that of a
  round with `Breaker: skipped` is read, so a change of the base does not count. A
  decision in which a merge of the base in the range resolves a conflict counts, and so
  does one in which the tree of the start and the base has a conflict, each also when
  the conflict leaves no markers or the range ends with the text of the base. So does a
  path outside `docs/decisions/` that the start changes since its merge base with the
  base, and that the tree of the start and the base moves into `docs/decisions/`, by the
  rename detection of the merge. A range with a start or an end that names no commit of
  the clone fails too, and so does an end that holds more than one newest base commit:
  the `review` skill asks for a merge of the base after round 1, never a rebase. When
  REVIEW TIERS does not already require `architecture`, the problem states the remedy
  and ends with the cause, such as "Its range changes the decision
  `docs/decisions/x.md`." A round posted before the cutoff `DECISION_CUTOFF` in
  `xtask/src/review.rs` (2026-10-11T16:00:00Z) is checked as before, so that the new
  rule does not fail the rounds that came before it. The cutoff is a fixed time: it
  stays when the rule merges after it, and the rule for `.rs` files gets a cutoff of its
  own. A round that fails by this rule gets a run of `architecture` on its range, or on
  the whole PR when the clone cannot read the range, and then an edit of its round
  comment that names `architecture` and adds its findings. One run on the whole PR
  serves each such round of the PR, and each edit states the range of the run. The last
  round fails when its `Public surface:` line holds the word `owed`, or when the first
  word of its value is not `none` and the line holds no comment link. The first comment
  link after each `departs from` is no approval and does not count on that line. The
  words of a line are the parts that its spaces divide it into, each with the characters
  at its ends removed that are no letter and no number, as `char::is_alphanumeric` of
  Rust reads a character. In a PR that changes a file of a risk crate (`crates/<name>/`,
  with `<name>` in `RISK` in `xtask/src/review.rs`, which holds the risk crates of
  ENGINEERS in `docs/decisions/operations/engineers.md`), the last round also fails when
  its `Deferred:` line names an issue with no comment link after it in the line and
  without the words `not in a risk crate` between it and the next issue of the line. The
  first word `none` does not stop this rule. A file that the PR moves counts at its old
  path and at its new one, in each rule that reads the files of the PR. An issue is each
  `#<n>` of the line, also in a URL, each `GH-<n>`, and each URL
  `https://github.com/<owner>/<repo>/issues/<n>` or
  `https://github.com/<owner>/<repo>/pull/<n>` that is no comment link. A `GH-<n>` is an
  issue only when it starts a text node of the line as GitHub shows it, or follows in
  its node a character that is no ASCII letter, no ASCII digit, and no `_`. A code span,
  a link, an image, and the start or the end of a mark each end a text node. An
  ignorable character, such as U+200B or U+00AD, is such a character. No issue starts in
  a comment link or in another issue. The problem names the round and the first such
  issue. A comment link is a URL in the text of the line as GitHub shows it:
  `https://github.com/<owner>/<repo>/pull/<n>` or
  `https://github.com/<owner>/<repo>/issues/<n>`, then `#issuecomment-<id>`,
  `#pullrequestreview-<id>`, or `#discussion_r<id>`. It starts the line or follows a
  space or `(`. A `[text](url)` link shows no URL and does not count, but its text
  counts when it is such a URL. On the `Deferred:` line, a comment link with no issue
  before it in the line is an issue and no comment link: the line names the deferral by
  the link of its comment. The limits: one link serves each issue before it on the
  `Deferred:` line, the words `not in a risk crate` serve only the issue before them,
  one link serves each item of the `Public surface:` line, and the check does not read
  what a link says. An issue that only the target of a `[text](url)` link names is not
  read: the check reads each line as GitHub shows it. The check also reads as an issue a
  `GH-<n>` in a URL that is no issue link, such as the owner of
  `https://github.com/gh-1/b/issues`, a `#<n>` or a `GH-<n>` that a letter, a digit, or
  a `_` follows, such as `#13abc`, the `#8203` of `&#8203;`, and a `GH-<n>` after `@`,
  which GitHub shows as the mention of a user. GitHub shows no issue for each. It does
  not read a path with no host, such as `synnaxlabs/foundation/issues/13`, for which
  GitHub shows an issue. The check links a bare URL by the autolink rule of comrak,
  which differs from that of GitHub in some forms, such as text after `www.` with no
  domain with a dot, and a domain that starts with a symbol such as `©`. In such a form
  the check can read the line in another way than GitHub shows it. For example, it reads
  the `&#40;` of `#12 https://©.y&#40;<comment link>)` as `(`, where GitHub shows
  `&#40;` as written, so it counts a comment link that GitHub does not show. The check
  reads `owed`, `departs from`, `GH-<n>`, `not in a risk crate`, and the path of a file
  under `crates/` in each ASCII case. Supersedes the words of goals 3 and 4 of
  https://github.com/synnaxlabs/foundation/issues/1211 that ask for a link for each
  item. A `Later steps:` line after the first of the end lines is not read. A last round
  posted before the cutoff `LINK_CUTOFF` in `xtask/src/review.rs` (2026-10-12T12:00:00Z)
  is checked as before, so that the rule does not fail the rounds that came before it.
  The cutoff is a fixed time: it stays when the rule merges after it. Stated by the
  director as rule item 24 at 2026-10-11T03:39:26Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6105098668), and
  ruled at 2026-10-11T10:47:32Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6108222189), and
  approved at 2026-10-11T20:32:18Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6113438520), and
  ruled for the rules of review round 1 at 2026-10-11T21:06:14Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6113751754), and
  approved with those rules at 2026-10-11T21:24:40Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6113934252), and
  ruled for the words of a line at 2026-10-11T23:16:06Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6114997222), and
  ruled for a comment link with no issue before it and for the start of a `GH-<n>` at
  2026-10-11T23:28:34Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6115109286), and
  ruled for the target of a `[text](url)` link and for an issue in a comment link at
  2026-10-12T15:30:35Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6127385827), and
  ruled for the start of a `GH-<n>` and for three limits at 2026-10-12T18:22:54Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6130107518), and
  ruled for a `GH-<n>` after a mark and after `@` at 2026-10-12T20:54:03Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6132434331), and
  ruled for text after `www.` at 2026-10-12T21:59:23Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6133389460) and for
  each form in which comrak and GitHub link a bare URL in different ways at
  2026-10-12T22:18:56Z
  (https://github.com/synnaxlabs/foundation/pull/2437#issuecomment-6133655869). The
  director approves the text of this record. A PR that changes it, and no public item of
  a crate and no dependency, needs no approval of an architect: `xtask` is on no
  architect's list, and the review rule is the director's.
  The check also fails while a merge condition of the PR body names an open issue or PR,
  with one problem for each such issue, in body order: "the PR body has
  `Merges after #2452`, and #2452 is open. Run `review` again when it merges or closes."
  A condition is `merges after` in each ASCII case, after no letter and no digit, then
  white space and `#<n>`, in a paragraph as GitHub shows it. A paragraph of a list item
  or a block quote counts, and so does a code span: an example such as
  `Merges after #2452` in the PR body fails while #2452 is open. A code block, a
  heading, a table, and HTML do not count. A PR body with a footnote inside another
  footnote, as `comrak` reads it, gives one problem with the line of the inner one, as a
  round comment does. A merged PR is closed. The check reads the state of each issue
  through `gh` at the time of the run, so the rule has no cutoff, and a failed read
  fails the run with the error of `gh`. The limits: only the issue right after the words
  counts, so `Merges after #1869 (#1860)` names only #1869. The check reads only the PR
  body, and in it no `GH-<n>`, no URL, and no `holds for #<n>`, although the `review`
  skill ("Done") asks that each condition that the PR body or an answer states is met.
  The failure of a PR body with a footnote inside another footnote: ruled by the
  director at 2026-10-12T23:07:13Z
  (https://github.com/synnaxlabs/foundation/pull/2495#issuecomment-6134334707). Stated
  by the director as goal 8 at 2026-10-08T20:25:39Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6068406481), put
  next at 2026-10-12T20:40:08Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6132233750), and
  ruled for a code span, a list item, a block quote, and a table at 2026-10-12T21:23:35Z
  (https://github.com/synnaxlabs/foundation/pull/2495#issuecomment-6132866182).
