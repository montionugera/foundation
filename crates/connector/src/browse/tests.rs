use std::collections::BTreeMap;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use document::diagnostic::Diagnostic;
use proptest::prelude::*;

use super::*;

/// Polls `future` once: each device of these tests answers at once.
fn now<T>(future: impl Future<Output = T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    match pin!(future).poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future is not ready"),
    }
}

/// One node of a test tree.
#[derive(Clone, Debug)]
enum Node {
    Folder(String, Vec<Node>),
    Point(String),
    Unreadable(String),
}

impl Node {
    fn name(&self) -> &str {
        match self {
            Self::Folder(name, _) | Self::Point(name) | Self::Unreadable(name) => name,
        }
    }
}

/// A device that holds a tree in memory and gives pages of `size` entries, with the
/// offset of the next one as the cursor.
#[derive(Debug, Default)]
struct Tree {
    /// The entries of each folder, by the text of its path.
    folders: BTreeMap<String, Vec<Child>>,
    values: BTreeMap<String, Value>,
    size: usize,
    /// Each folder that `level` was asked for, in order, and each `values` call.
    browsed: Vec<Path>,
    reads: Vec<Vec<String>>,
}

impl Tree {
    fn new(nodes: &[Node], size: usize) -> Self {
        let mut tree = Self {
            size,
            ..Self::default()
        };
        tree.add(&Path::root(), nodes);
        tree
    }

    fn add(&mut self, at: &Path, nodes: &[Node]) {
        let mut entries = Vec::new();
        for node in nodes {
            let path = at.child(node.name());
            let shape = match node {
                Node::Folder(_, children) => {
                    self.add(&path, children);
                    Shape::Folder
                }
                Node::Point(_) => Shape::Point(Facts::default()),
                Node::Unreadable(_) => Shape::Unreadable("an array".into()),
            };
            let tag = path.to_string();
            entries.push(Child {
                name: node.name().into(),
                tag,
                shape,
            });
        }
        self.folders.insert(at.to_string(), entries);
    }
}

impl Tree {
    fn page(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> Option<Page> {
        let at = folder.map_or_else(Path::root, |folder| folder.path.clone());
        let entries = self.folders.get(&at.to_string())?;
        self.browsed.push(at);
        let from = cursor.map_or(0, |cursor| cursor.as_str().parse().expect("offset"));
        let to = (from + self.size).min(entries.len());
        let next = (to < entries.len()).then(|| Cursor::new(to.to_string()));
        Some(Page {
            entries: entries[from..to].to_vec(),
            next,
        })
    }

    fn read(&mut self, tags: &[&str]) -> Vec<Option<Value>> {
        self.reads
            .push(tags.iter().map(ToString::to_string).collect());
        tags.iter()
            .map(|tag| self.values.get(*tag).copied())
            .collect()
    }
}

impl Device for Tree {
    fn level(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>> {
        std::future::ready(Ok(self.page(folder, cursor)))
    }

    fn values(
        &mut self,
        tags: &[&str],
    ) -> impl Future<Output = Result<Vec<Option<Value>>, Error>> {
        std::future::ready(Ok(self.read(tags)))
    }
}

/// A device that cannot read values.
struct Blind(Tree);

impl Device for Blind {
    fn level(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>> {
        self.0.level(folder, cursor)
    }
}

fn path(text: &str) -> Path {
    text.parse().expect("a path")
}

fn pattern(text: &str) -> Pattern {
    text.parse().expect("a pattern")
}

fn folder(name: &str, children: Vec<Node>) -> Node {
    Node::Folder(name.into(), children)
}

fn point(name: &str) -> Node {
    Node::Point(name.into())
}

fn query(start: &str, scope: Scope) -> Query {
    Query {
        start: path(start),
        scope,
        values: Values::Skipped,
    }
}

/// The paths of `found`, as text.
fn paths(found: &Found) -> Vec<String> {
    found.entries.iter().map(|e| e.path.to_string()).collect()
}

/// The one diagnostic of a config error.
fn diagnostic(result: Result<Found, Error>) -> Diagnostic {
    match result {
        Err(Error::Config(mut diagnostics)) if diagnostics.len() == 1 => {
            diagnostics.remove(0)
        }
        other => panic!("one config error: {other:?}"),
    }
}

/// The diagnostic of a break of the rule `rule` of the `Device` contract.
fn broken(rule: &str) -> Diagnostic {
    Diagnostic::new(
        Code::new("connector.broken-browse"),
        None,
        format!("the connector kind broke a rule of a browse: {rule}"),
        "Report this defect of the connector kind with this message".into(),
    )
}

/// A plant: two lines with temperatures and a pressure, and an array.
fn plant() -> Vec<Node> {
    vec![
        folder(
            "Line1",
            vec![point("Temp1"), point("Temp2"), point("Pressure")],
        ),
        folder(
            "Line2",
            vec![
                folder("Oven", vec![point("Temp"), Node::Unreadable("Map".into())]),
                point("Speed"),
            ],
        ),
        point("Clock"),
    ]
}

mod text {
    use super::*;

    #[test]
    fn reads_the_root_from_the_empty_text() {
        assert_eq!(path(""), Path::root());
        assert_eq!(Path::root().to_string(), "");
    }

    #[test]
    fn reads_the_names_of_a_path() {
        let read = path(r"Line\/3/Temp*/a\\b");
        let names = ["Line/3", "Temp*", r"a\b"];
        assert_eq!(read.names().collect::<Vec<_>>(), names);
        assert_eq!(
            read,
            Path::root().child(names[0]).child(names[1]).child(names[2])
        );
        assert_eq!(read.to_string(), r"Line\/3/Temp\*/a\\b");
    }

    #[test]
    fn compares_paths_by_their_names() {
        assert_eq!(path("a/b"), Path::root().child("a").child("b"));
        assert_ne!(path("a/b"), path("b"));
        assert_ne!(path("b"), path("a/b"));
        assert_ne!(path("a/b"), path("a/c"));
        assert_ne!(path("a"), Path::root());
    }

    #[test]
    fn drops_a_deep_path() {
        let mut path = Path::root();
        for _ in 0..1_000_000 {
            path = path.child("n");
        }
        assert_eq!(path.names().len(), 1_000_000);
    }

    #[test]
    fn shows_a_path_as_its_text_in_debug() {
        assert_eq!(format!("{:?}", path("a/b")), r#""a/b""#);
    }

    #[test]
    fn shows_a_pattern_as_its_text() {
        let read = pattern(r"**/T***p/\*/\/x");
        assert_eq!(read.to_string(), r"**/T*p/\*/\/x");
        assert_eq!(format!("{read:?}"), r#""**/T*p/\\*/\\/x""#);
    }

    #[test]
    fn refuses_an_empty_name() {
        for text in ["a//b", "/a", "a/"] {
            assert_eq!(text.parse::<Path>(), Err(Malformed::Empty), "{text}");
            assert_eq!(text.parse::<Pattern>(), Err(Malformed::Empty), "{text}");
        }
        assert_eq!("".parse::<Pattern>(), Err(Malformed::Empty));
        assert_eq!(Malformed::Empty.to_string(), "a name in it is empty");
    }

    #[test]
    fn refuses_a_trailing_escape() {
        assert_eq!(r"a\".parse::<Path>(), Err(Malformed::Escape));
        assert_eq!(r"a/\".parse::<Pattern>(), Err(Malformed::Escape));
        assert_eq!(
            Malformed::Escape.to_string(),
            r"it ends in a `\` that takes no character",
        );
    }

    proptest! {
        #[test]
        fn round_trips_a_path(names in prop::collection::vec("\\PC{1,4}", 0..4)) {
            let built = names
                .iter()
                .fold(Path::root(), |path, name| path.child(name.as_str()));
            prop_assert_eq!(built.to_string().parse::<Path>(), Ok(built));
        }

        #[test]
        fn round_trips_a_pattern(segments in segments()) {
            let read = pattern(&text_of(&segments));
            prop_assert_eq!(read.to_string().parse::<Pattern>(), Ok(read));
        }
    }
}

/// One segment of a reference pattern.
#[derive(Clone, Debug)]
enum Reference {
    Any,
    Glob(Vec<Option<char>>),
}

fn segments() -> impl Strategy<Value = Vec<Reference>> {
    let glob = prop::collection::vec(
        prop_oneof![
            Just(None),
            prop::sample::select(vec!['a', 'b', '/', '*', '\\']).prop_map(Some),
        ],
        1..4,
    )
    // `**` alone is `Any`.
    .prop_filter("not two stars alone", |glob| glob != &[None, None]);
    let segment =
        prop_oneof![1 => Just(Reference::Any), 3 => glob.prop_map(Reference::Glob)];
    prop::collection::vec(segment, 1..4)
}

fn text_of(segments: &[Reference]) -> String {
    let texts: Vec<String> = segments
        .iter()
        .map(|segment| match segment {
            Reference::Any => "**".into(),
            Reference::Glob(glob) => glob
                .iter()
                .map(|c| match c {
                    None => "*".into(),
                    Some(c @ ('/' | '*' | '\\')) => format!("\\{c}"),
                    Some(c) => c.to_string(),
                })
                .collect(),
        })
        .collect();
    texts.join("/")
}

fn glob(glob: &[Option<char>], name: &[char]) -> bool {
    match (glob.split_first(), name.split_first()) {
        (None, _) => name.is_empty(),
        (Some((None, rest)), _) => {
            self::glob(rest, name)
                || name.split_first().is_some_and(|(_, n)| self::glob(glob, n))
        }
        (Some((Some(c), rest)), Some((n, names))) => c == n && self::glob(rest, names),
        (Some((Some(_), _)), None) => false,
    }
}

/// Whether `names` match `segments`.
fn reference(segments: &[Reference], names: &[&str]) -> bool {
    match (segments.split_first(), names.split_first()) {
        (None, _) => names.is_empty(),
        (Some((Reference::Any, rest)), _) => {
            reference(rest, names)
                || names
                    .split_first()
                    .is_some_and(|(_, n)| reference(segments, n))
        }
        (Some((Reference::Glob(g), rest)), Some((name, names))) => {
            let chars: Vec<char> = name.chars().collect();
            glob(g, &chars) && reference(rest, names)
        }
        (Some((Reference::Glob(_), _)), None) => false,
    }
}

/// Whether some names after `names` make a path that matches `segments`.
fn extends(segments: &[Reference], names: &[&str]) -> bool {
    match (segments.split_first(), names.split_first()) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some((Reference::Any, rest)), Some((_, after))) => {
            extends(rest, names) || extends(segments, after)
        }
        (Some((Reference::Glob(g), rest)), Some((name, after))) => {
            let chars: Vec<char> = name.chars().collect();
            glob(g, &chars) && extends(rest, after)
        }
    }
}

/// Whether `pattern` matches `names`, through the steps that the walk takes. Through
/// private calls: a `Pattern` has no public match, and a walk shows one only over a
/// whole device.
fn steps(pattern: &Pattern, names: &[&str]) -> bool {
    let state = names
        .iter()
        .fold(pattern.start(), |state, name| pattern.step(&state, name));
    pattern.matched(&state)
}

mod pattern {
    use super::*;

    #[test]
    fn matches_any_number_of_names_with_two_stars() {
        let any = pattern("Line*/**/Temp*");
        assert!(steps(&any, &["Line1", "Temp1"]));
        assert!(steps(&any, &["Line2", "Oven", "Rack", "Temp"]));
        assert!(!steps(&any, &["Line2", "Oven"]));
        assert!(!steps(&any, &["Plant", "Temp"]));
    }

    #[test]
    fn matches_an_escaped_star_as_itself() {
        let star = pattern(r"a\*");
        assert!(steps(&star, &["a*"]));
        assert!(!steps(&star, &["ab"]));
    }

    proptest! {
        #[test]
        fn matches_as_the_reference(
            segments in segments(),
            names in prop::collection::vec("[ab/*\\\\]{1,3}", 0..5),
        ) {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let read = pattern(&text_of(&segments));
            prop_assert_eq!(steps(&read, &names), reference(&segments, &names));
            let state = names
                .iter()
                .fold(read.start(), |state, name| read.step(&state, name));
            prop_assert_eq!(read.open(&state), extends(&segments, &names));
        }
    }
}

/// What a walk of `segments` over `nodes` with `limit` and `visits` gives, from a
/// recursive walk of the whole tree.
fn expect(
    nodes: &[Node],
    segments: &[Reference],
    limit: usize,
    visits: usize,
) -> (Vec<String>, End) {
    struct Walk<'a> {
        segments: &'a [Reference],
        limit: usize,
        visits: usize,
        seen: usize,
        found: Vec<String>,
    }
    fn go(walk: &mut Walk<'_>, names: &[&str], nodes: &[Node]) -> ControlFlow<End> {
        if nodes.is_empty() {
            walk.seen += 1;
            if walk.seen > walk.visits {
                return ControlFlow::Break(End::Budget);
            }
        }
        for node in nodes {
            walk.seen += 1;
            if walk.seen > walk.visits {
                return ControlFlow::Break(End::Budget);
            }
            let mut child = names.to_vec();
            child.push(node.name());
            match node {
                Node::Folder(_, children) => {
                    if extends(walk.segments, &child) {
                        go(walk, &child, children)?;
                    }
                }
                Node::Point(_) | Node::Unreadable(_) => {
                    if reference(walk.segments, &child) {
                        if walk.found.len() == walk.limit {
                            return ControlFlow::Break(End::Limit);
                        }
                        let path = child.iter().fold(Path::root(), |p, n| p.child(*n));
                        walk.found.push(path.to_string());
                    }
                }
            }
        }
        ControlFlow::Continue(())
    }
    let mut walk = Walk {
        segments,
        limit,
        visits,
        seen: 0,
        found: Vec::new(),
    };
    let end = match go(&mut walk, &[], nodes) {
        ControlFlow::Break(end) => end,
        ControlFlow::Continue(()) => End::Done,
    };
    (walk.found, end)
}

fn nodes() -> impl Strategy<Value = Vec<Node>> {
    let name = prop::sample::select(vec!["a", "b", "ab", "ba", "a*"]);
    let leaf = prop_oneof![
        3 => name.clone().prop_map(point),
        1 => name.clone().prop_map(|n| Node::Unreadable(n.into())),
    ];
    let node = leaf.prop_recursive(4, 40, 4, move |inner| {
        prop_oneof![
            1 => inner.clone(),
            2 => (name.clone(), prop::collection::vec(inner, 0..4))
                .prop_map(|(n, children)| folder(n, unique(children))),
        ]
    });
    prop::collection::vec(node, 0..5).prop_map(unique)
}

/// `nodes` with the first of each name only.
fn unique(nodes: Vec<Node>) -> Vec<Node> {
    let mut names = std::collections::BTreeSet::new();
    nodes
        .into_iter()
        .filter(|node| names.insert(node.name().to_owned()))
        .collect()
}

mod walk {
    use super::*;

    proptest! {
        /// Through `walk_within`: bounds as small as the trees that a property test
        /// makes.
        #[test]
        fn gives_each_match_in_device_order_within_the_bounds(
            nodes in nodes(),
            segments in segments(),
            limit in 0..5usize,
            visits in 0..40usize,
            size in 1..4usize,
        ) {
            let mut tree = Tree::new(&nodes, size);
            let query = query("", Scope::Match(pattern(&text_of(&segments))));
            let found = now(walk_within(&mut tree, &query, limit, visits))
                .expect("found");
            let (expected, end) = expect(&nodes, &segments, limit, visits);
            prop_assert_eq!(paths(&found), expected);
            prop_assert_eq!(found.end, end);
            prop_assert!(found.values.is_empty());
            for browsed in tree.browsed.iter().skip(1) {
                let names: Vec<&str> =
                    browsed.names().collect();
                prop_assert!(extends(&segments, &names), "browsed {browsed:?}");
            }
        }
    }

    #[test]
    fn matches_from_its_start() {
        let mut tree = Tree::new(&plant(), 2);
        let query = query("Line2", Scope::Match(pattern("**/*")));
        let found = now(walk(&mut tree, &query)).expect("found");
        assert_eq!(
            paths(&found),
            ["Line2/Oven/Temp", "Line2/Oven/Map", "Line2/Speed"]
        );
        assert_eq!(found.end, End::Done);
        let tags: Vec<&str> = found.entries.iter().map(|e| e.tag.as_str()).collect();
        assert_eq!(tags, ["Line2/Oven/Temp", "Line2/Oven/Map", "Line2/Speed"]);
        assert_eq!(found.entries[1].shape, Shape::Unreadable("an array".into()),);
    }

    #[test]
    fn browses_no_folder_past_the_depth_of_a_pattern() {
        let mut tree = Tree::new(&plant(), 10);
        let query = query("", Scope::Match(pattern("Line*/Temp*")));
        let found = now(walk(&mut tree, &query)).expect("found");
        assert_eq!(paths(&found), ["Line1/Temp1", "Line1/Temp2"]);
        assert_eq!(tree.browsed, [path(""), path("Line1"), path("Line2")]);
    }

    #[test]
    fn browses_no_folder_that_a_pattern_names_as_a_point() {
        let mut tree = Tree::new(&plant(), 10);
        let found = now(walk(
            &mut tree,
            &query("", Scope::Match(pattern("Line2/Oven"))),
        ));
        assert_eq!(found.expect("found").entries, []);
        assert_eq!(tree.browsed, [path(""), path("Line2")]);
    }

    #[test]
    fn ends_at_the_budget_before_its_start() {
        let mut nodes: Vec<Node> =
            (0..VISITS).map(|i| point(&format!("p{i}"))).collect();
        nodes.push(folder("start", vec![point("a")]));
        let mut tree = Tree::new(&nodes, LIMIT);
        let found = now(walk(&mut tree, &query("start", Scope::Match(pattern("*")))));
        let found = found.expect("found");
        assert_eq!(found.entries, []);
        assert_eq!(found.end, End::Budget);
    }

    #[test]
    fn asks_for_no_page_past_the_budget() {
        let mut nodes: Vec<Node> =
            (0..VISITS - 1).map(|i| point(&format!("p{i}"))).collect();
        nodes.push(folder("a", vec![point("x")]));
        let mut tree = Tree::new(&nodes, LIMIT);
        let found = now(walk(&mut tree, &query("", Scope::Match(pattern("a/*")))));
        assert_eq!(found.expect("found").end, End::Budget);
        assert!(!tree.browsed.contains(&path("a")), "{:?}", tree.browsed);
    }

    #[test]
    fn walks_past_a_folder_with_no_entries() {
        let mut tree = Tree::new(&[folder("e", vec![]), point("p")], 10);
        let found = now(walk(&mut tree, &query("", Scope::Match(pattern("**")))));
        let found = found.expect("found");
        assert_eq!(paths(&found), ["p"]);
        assert_eq!(found.end, End::Done);
    }

    /// A tree of one folder that holds `n` points.
    fn wide(n: usize) -> Tree {
        let points: Vec<Node> = (0..n).map(|i| point(&format!("p{i}"))).collect();
        Tree::new(&points, LIMIT)
    }

    #[test]
    fn gives_at_most_the_limit_of_points() {
        for (n, end) in [
            (LIMIT - 1, End::Done),
            (LIMIT, End::Done),
            (LIMIT + 1, End::Limit),
        ] {
            let query = query("", Scope::Match(pattern("*")));
            let found = now(walk(&mut wide(n), &query)).expect("found");
            let expected: Vec<String> =
                (0..n.min(LIMIT)).map(|i| format!("p{i}")).collect();
            assert_eq!(paths(&found), expected, "{n}");
            assert_eq!(found.end, end, "{n}");
        }
    }

    #[test]
    fn takes_at_most_the_budget_of_entries() {
        for (n, end) in [
            (VISITS - 1, End::Done),
            (VISITS, End::Done),
            (VISITS + 1, End::Budget),
        ] {
            let query = query("", Scope::Match(pattern("none")));
            let found = now(walk(&mut wide(n), &query)).expect("found");
            assert_eq!(found.entries, [], "{n}");
            assert_eq!(found.end, end, "{n}");
        }
    }

    #[test]
    fn refuses_a_start_with_no_folder() {
        for (start, shown) in [
            ("Line9", r#""Line9""#),
            ("Clock", r#""Clock""#),
            (r"Line\/1", r#""Line\/1""#),
            ("Line9/Oven", r#""Line9""#),
        ] {
            let query = query(start, Scope::Match(pattern("**")));
            let found = now(walk(&mut Tree::new(&plant(), 2), &query));
            assert_eq!(
                diagnostic(found),
                Diagnostic::new(
                    Code::new("connector.no-folder"),
                    None,
                    format!("the device has no folder at {shown}"),
                    "Browse the folder above it to see the names in it".into(),
                ),
            );
        }
    }

    #[test]
    fn refuses_a_folder_that_is_gone() {
        let mut tree = Tree::new(&plant(), 2);
        tree.folders.remove("Line2/Oven");
        let query = query("", Scope::Match(pattern("**")));
        let found = now(walk(&mut tree, &query));
        assert_eq!(
            diagnostic(found).message,
            r#"the device has no folder at "Line2/Oven""#,
        );
    }

    #[test]
    fn refuses_a_start_at_a_point_that_the_device_can_list() {
        let mut tree = Tree::new(&plant(), 10);
        tree.folders.insert("Clock".into(), Vec::new());
        let found = now(walk(&mut tree, &query("Clock", Scope::Level(None))));
        assert_eq!(
            diagnostic(found).message,
            r#"the device has no folder at "Clock""#,
        );
    }

    #[test]
    fn finds_a_start_under_a_paged_parent() {
        let mut tree = Tree::new(&plant(), 1);
        let query = query("Line2/Oven", Scope::Match(pattern("*")));
        let found = now(walk(&mut tree, &query)).expect("found");
        assert_eq!(paths(&found), ["Line2/Oven/Temp", "Line2/Oven/Map"]);
        assert_eq!(found.end, End::Done);
    }

    /// Through `walk_within`: a budget as small as the tree.
    #[test]
    fn counts_the_entries_before_its_start_toward_the_budget() {
        // Line1 and Line2 of the root, and Oven of Line2, find the start.
        for (visits, found, end) in [
            (2, &[][..], End::Budget),
            (3, &[], End::Budget),
            (4, &["Line2/Oven/Temp"], End::Budget),
            (5, &["Line2/Oven/Temp", "Line2/Oven/Map"], End::Done),
        ] {
            let mut tree = Tree::new(&plant(), 10);
            let query = query("Line2/Oven", Scope::Match(pattern("*")));
            let walked = now(walk_within(&mut tree, &query, LIMIT, visits));
            let walked = walked.expect("found");
            assert_eq!(paths(&walked), found, "{visits}");
            assert_eq!(walked.end, end, "{visits}");
        }
        let mut tree = Tree::new(&plant(), 10);
        let query = query("Line2/Oven", Scope::Level(None));
        let walked = now(walk_within(&mut tree, &query, LIMIT, 2)).expect("found");
        assert_eq!(walked.entries, []);
        assert_eq!(walked.end, End::Budget);
    }

    #[test]
    fn refuses_a_page_of_more_than_the_limit() {
        let points: Vec<Node> = (0..=LIMIT).map(|i| point(&format!("p{i}"))).collect();
        let mut tree = Tree::new(&points, LIMIT + 1);
        let found = now(walk(&mut tree, &query("", Scope::Level(None))));
        assert_eq!(
            diagnostic(found),
            broken(r#"a page holds at most 500 entries, and a page of "" held 501"#),
        );
    }

    /// A device whose each folder holds the folder `a`, with the tag `loop`, and the
    /// point `p`.
    struct Loop;

    impl Device for Loop {
        fn level(
            &mut self,
            folder: Option<&Entry>,
            _: Option<&Cursor>,
        ) -> impl Future<Output = Result<Option<Page>, Error>> {
            let at = folder.map_or_else(Path::root, |folder| folder.path.clone());
            let entry = |name: &str, tag: &str, shape| Child {
                name: name.into(),
                tag: tag.into(),
                shape,
            };
            let entries = vec![
                entry("a", "loop", Shape::Folder),
                entry(
                    "p",
                    &at.child("p").to_string(),
                    Shape::Point(Facts::default()),
                ),
            ];
            std::future::ready(Ok(Some(Page {
                entries,
                next: None,
            })))
        }
    }

    #[test]
    fn browses_no_folder_with_the_tag_and_state_of_a_folder_above_it() {
        let query = query("", Scope::Match(pattern("**/p")));
        let found = now(walk(&mut Loop, &query)).expect("found");
        assert_eq!(paths(&found), ["a/p", "p"]);
        assert_eq!(found.end, End::Done);
    }

    /// A device whose root holds the folder `a`, `a` holds the folder `b` and the
    /// point `p`, and `b` holds the folder `a`, with the tag of `a`, and the point `q`.
    struct Ring;

    impl Device for Ring {
        fn level(
            &mut self,
            folder: Option<&Entry>,
            _: Option<&Cursor>,
        ) -> impl Future<Output = Result<Option<Page>, Error>> {
            let entry = |name: &str, shape| Child {
                name: name.into(),
                tag: name.into(),
                shape,
            };
            let entries = match folder.map(|folder| folder.tag.as_str()) {
                None => vec![entry("a", Shape::Folder)],
                Some("a") => vec![
                    entry("b", Shape::Folder),
                    entry("p", Shape::Point(Facts::default())),
                ],
                _ => vec![
                    entry("a", Shape::Folder),
                    entry("q", Shape::Point(Facts::default())),
                ],
            };
            std::future::ready(Ok(Some(Page {
                entries,
                next: None,
            })))
        }
    }

    #[test]
    fn ends_a_loop_of_two_folders() {
        let found = now(walk(&mut Ring, &query("", Scope::Match(pattern("**")))));
        let found = found.expect("found");
        assert_eq!(paths(&found), ["a/b/q", "a/p"]);
        assert_eq!(found.end, End::Done);
    }

    #[test]
    fn finds_a_path_that_a_pattern_names_through_a_loop() {
        let query = query("", Scope::Match(pattern("a/a/p")));
        let found = now(walk(&mut Loop, &query)).expect("found");
        assert_eq!(paths(&found), ["a/a/p"]);
        assert_eq!(found.end, End::Done);
    }

    /// A device whose each folder holds the point `p` and then the folder `f`, each
    /// with its path as its tag.
    struct Chain;

    impl Device for Chain {
        fn level(
            &mut self,
            folder: Option<&Entry>,
            _: Option<&Cursor>,
        ) -> impl Future<Output = Result<Option<Page>, Error>> {
            let at = folder.map_or_else(Path::root, |folder| folder.path.clone());
            let entry = |name: &str, shape| Child {
                name: name.into(),
                tag: at.child(name).to_string(),
                shape,
            };
            let entries = vec![
                entry("p", Shape::Point(Facts::default())),
                entry("f", Shape::Folder),
            ];
            std::future::ready(Ok(Some(Page {
                entries,
                next: None,
            })))
        }
    }

    #[test]
    fn ends_when_it_holds_the_depth_of_folders_open() {
        let found = now(walk(&mut Chain, &query("", Scope::Match(pattern("**/p")))));
        let found = found.expect("found");
        let deepest = found.entries.last().expect("a point").path.names().len();
        assert_eq!(found.entries.len(), DEPTH);
        assert_eq!(deepest, DEPTH);
        assert_eq!(found.end, End::Budget);
    }

    #[test]
    fn walks_a_pattern_from_a_start_deeper_than_the_depth() {
        let start = vec!["f"; DEPTH + 1].join("/");
        let query = query(&start, Scope::Match(pattern("*/p")));
        let found = now(walk(&mut Chain, &query)).expect("found");
        assert_eq!(paths(&found), [format!("{start}/f/p")]);
        assert_eq!(found.end, End::Done);
    }

    #[test]
    fn refuses_an_entry_whose_name_is_empty() {
        let mut tree = Tree::new(&plant(), 10);
        let entries = tree.folders.get_mut("Line1").expect("Line1");
        entries[1].name = String::new();
        let found = now(walk(&mut tree, &query("", Scope::Match(pattern("**")))));
        assert_eq!(
            diagnostic(found),
            broken(r#"an entry has a name, and an entry of "Line1" has an empty name"#),
        );
    }

    /// A device whose each page is empty, with a next one.
    struct Empty;

    impl Device for Empty {
        fn level(
            &mut self,
            _: Option<&Entry>,
            _: Option<&Cursor>,
        ) -> impl Future<Output = Result<Option<Page>, Error>> {
            std::future::ready(Ok(Some(Page {
                entries: Vec::new(),
                next: Some(Cursor::new("more".into())),
            })))
        }
    }

    #[test]
    fn counts_a_page_with_no_entries_as_one_visit() {
        for start in ["", "a"] {
            let query = query(start, Scope::Match(pattern("**")));
            let found = now(walk(&mut Empty, &query)).expect("found");
            assert_eq!(found.entries, [], "{start}");
            assert_eq!(found.end, End::Budget, "{start}");
        }
    }

    #[test]
    fn refuses_an_entry_of_the_root_whose_name_is_empty() {
        for scope in [Scope::Level(None), Scope::Match(pattern("**"))] {
            let mut tree = Tree::new(&plant(), 10);
            let entries = tree.folders.get_mut("").expect("the root");
            entries[2].name = String::new();
            let found = now(walk(&mut tree, &query("", scope.clone())));
            let rule = r#"an entry has a name, and an entry of "" has an empty name"#;
            assert_eq!(diagnostic(found), broken(rule), "{scope:?}");
        }
    }

    /// The path of each point of `nodes` under `at`.
    fn points(at: &Path, nodes: &[Node], out: &mut Vec<Path>) {
        for node in nodes {
            let path = at.child(node.name());
            match node {
                Node::Folder(_, children) => points(&path, children, out),
                Node::Point(_) | Node::Unreadable(_) => out.push(path),
            }
        }
    }

    proptest! {
        #[test]
        fn matches_the_text_of_a_path_to_that_path_alone(nodes in nodes()) {
            let mut all = Vec::new();
            points(&Path::root(), &nodes, &mut all);
            for point in all {
                let mut tree = Tree::new(&nodes, 2);
                let query = query("", Scope::Match(pattern(&point.to_string())));
                let found = now(walk(&mut tree, &query)).expect("found");
                prop_assert_eq!(paths(&found), [point.to_string()]);
            }
        }
    }
}

mod level {
    use super::*;

    #[test]
    fn gives_each_page_and_the_cursor_of_the_next() {
        let mut tree = Tree::new(&plant(), 2);
        let first = now(walk(&mut tree, &query("Line1", Scope::Level(None))));
        let first = first.expect("found");
        assert_eq!(paths(&first), ["Line1/Temp1", "Line1/Temp2"]);
        let End::Next(cursor) = first.end else {
            panic!("a next page: {first:?}");
        };
        let rest = query("Line1", Scope::Level(Some(cursor)));
        let rest = now(walk(&mut tree, &rest)).expect("found");
        assert_eq!(paths(&rest), ["Line1/Pressure"]);
        assert_eq!(rest.end, End::Done);
    }

    #[test]
    fn gives_folders_and_points_of_the_root() {
        let mut tree = Tree::new(&plant(), 10);
        let found =
            now(walk(&mut tree, &query("", Scope::Level(None)))).expect("found");
        assert_eq!(paths(&found), ["Line1", "Line2", "Clock"]);
        assert_eq!(found.entries[0].shape, Shape::Folder);
        assert_eq!(found.entries[2].shape, Shape::Point(Facts::default()));
    }

    #[test]
    fn refuses_a_start_with_no_folder() {
        let found = now(walk(
            &mut Tree::new(&plant(), 2),
            &query("Line1/Temp1", Scope::Level(None)),
        ));
        assert_eq!(
            diagnostic(found).message,
            r#"the device has no folder at "Line1/Temp1""#,
        );
    }
}

mod values {
    use super::*;

    fn read(start: &str, scope: Scope) -> Query {
        Query {
            values: Values::Read,
            ..query(start, scope)
        }
    }

    #[test]
    fn reads_the_points_in_one_call_with_none_for_the_others() {
        let mut tree = Tree::new(&plant(), 2);
        tree.values.insert("Line2/Speed".into(), Value::F64(1.5));
        let found = now(walk(&mut tree, &read("Line2", Scope::Level(None))));
        let found = found.expect("found");
        assert_eq!(paths(&found), ["Line2/Oven", "Line2/Speed"]);
        assert_eq!(found.values, [None, Some(Value::F64(1.5))]);
        assert_eq!(tree.reads, [vec!["Line2/Speed".to_owned()]]);
    }

    #[test]
    fn gives_none_for_a_value_that_the_device_did_not_give() {
        let mut tree = Tree::new(&plant(), 10);
        tree.values.insert("Line1/Temp2".into(), Value::I64(7));
        let query = read("", Scope::Match(pattern("Line1/*")));
        let found = now(walk(&mut tree, &query)).expect("found");
        assert_eq!(found.values, [None, Some(Value::I64(7)), None]);
        assert_eq!(tree.reads.len(), 1);
    }

    #[test]
    fn reads_nothing_with_no_point() {
        let mut tree = Tree::new(&plant(), 10);
        let query = read("Line2/Oven", Scope::Match(pattern("Map")));
        let found = now(walk(&mut tree, &query)).expect("found");
        assert_eq!(found.values, [None]);
        assert_eq!(tree.reads, Vec::<Vec<String>>::new());
    }

    #[test]
    fn refuses_values_from_a_device_that_cannot_read_them() {
        let mut blind = Blind(Tree::new(&plant(), 10));
        let found = now(walk(&mut blind, &read("Line1", Scope::Level(None))));
        assert_eq!(
            diagnostic(found),
            Diagnostic::new(
                Code::new("connector.no-values"),
                None,
                "this connector kind cannot read the values of its points".into(),
                "Browse without values".into(),
            ),
        );
    }

    /// A device that gives one value too few.
    struct Short(Tree);

    impl Device for Short {
        fn level(
            &mut self,
            folder: Option<&Entry>,
            cursor: Option<&Cursor>,
        ) -> impl Future<Output = Result<Option<Page>, Error>> {
            self.0.level(folder, cursor)
        }

        fn values(
            &mut self,
            tags: &[&str],
        ) -> impl Future<Output = Result<Vec<Option<Value>>, Error>> {
            std::future::ready(Ok(vec![None; tags.len() - 1]))
        }
    }

    #[test]
    fn refuses_a_count_of_values_that_is_not_the_count_of_tags() {
        let mut short = Short(Tree::new(&plant(), 10));
        let found = now(walk(&mut short, &read("Line1", Scope::Level(None))));
        assert_eq!(
            diagnostic(found),
            broken("one value comes for each tag, and 2 came for 3 tags"),
        );
    }
}
