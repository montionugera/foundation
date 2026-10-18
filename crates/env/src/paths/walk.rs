//! The walk of a path, name by name, as the host takes it, on open directories.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::iter;
use std::path::{Component, Path, PathBuf};

use types::hash;

use super::dir::{self, Entry};
use super::{Driver, Error, Key, LINKS, names_directory};

/// The most times that the walk of one path starts again after a program moves a
/// directory of the path.
pub(super) const STARTS: usize = 40;

/// A name that the walk reached. Its resolved path is [`path`].
#[derive(Clone, Debug)]
pub(super) struct Step {
    /// The name in the directory of `up`, or the root path for a root.
    name: OsString,
    /// The key of the file at the step with no link followed, or `None` when nothing is
    /// there or the host refused it.
    pub(super) own: Option<Key>,
    /// The step of the directory that holds this one, `None` for a root.
    pub(super) up: Option<usize>,
    /// A `..` takes the walk above this step, or an end `/` or `/.` failed at it.
    pub(super) back: bool,
    pub(super) directory: bool,
}

/// The steps of the walk of one path, in order.
#[derive(Debug)]
pub(super) struct Walk {
    pub(super) steps: Vec<Step>,
    /// `Ok` when the walk reached its last name, and then the last step is where the
    /// path ends: `Ok(Err)` when the host refused that name. `Err` when it stopped
    /// before, at its last step.
    pub(super) end: Result<Result<(), Error>, Error>,
    /// The host refused a name in a step that is a file, which holds no names.
    pub(super) in_file: bool,
}

impl Walk {
    /// The walk of `path`. A link to nothing at the last name ends at its target.
    pub(super) fn new(driver: &dyn Driver, path: &Path) -> Self {
        Self::from(driver, path, false)
    }

    /// The walk of `path`, which is absolute, with no bound on its length: the caller
    /// bounded the path that it came from.
    pub(super) fn on_root(driver: &dyn Driver, path: &Path) -> Self {
        Self::from(driver, path, true)
    }

    fn from(driver: &dyn Driver, path: &Path, rooted: bool) -> Self {
        let mut walker = Walker {
            driver,
            given: path,
            rooted,
            steps: Vec::new(),
            names: VecDeque::new(),
            at: 0,
            dir: None,
            links: LINKS,
            starts: 0,
            in_file: false,
        };
        let end = walker.run();
        Self {
            steps: walker.steps,
            end,
            in_file: walker.in_file,
        }
    }

    /// Where the path ends, or the error of the walk or of the host at that step.
    pub(super) fn into_end(self) -> Result<End, Error> {
        self.end??;
        let index = self
            .steps
            .len()
            .checked_sub(1)
            .expect("invariant: a walk that ends has an end step");
        Ok(End {
            path: path(&self.steps, index),
            own: self.steps[index].own,
        })
    }
}

/// The step where a walk ends.
pub(super) struct End {
    pub(super) path: PathBuf,
    /// The key of the file at `path` with no link followed, or `None` when nothing is
    /// there.
    pub(super) own: Option<Key>,
}

/// The resolved path of step `index` of `steps`.
pub(super) fn path(steps: &[Step], index: usize) -> PathBuf {
    let names: Vec<&OsStr> = iter::successors(Some(index), |&at| steps[at].up)
        .map(|at| steps[at].name.as_os_str())
        .collect();
    names.into_iter().rev().collect()
}

/// The key of the directory that holds step `index` of `steps`, or `None` for a root
/// or a step in a file.
pub(super) fn holder(steps: &[Step], index: usize) -> Option<Key> {
    let up = &steps[steps[index].up?];
    up.directory.then_some(up.own).flatten()
}

/// What the directories above each step of a walk hold, found in one pass for each
/// key asked, so that a check of each step costs the keys, not the depth.
pub(super) struct Above<'a> {
    steps: &'a [Step],
    /// For each key asked: at each step, a directory above that step has the key.
    held: RefCell<Vec<(Key, Vec<bool>)>>,
}

impl<'a> Above<'a> {
    pub(super) fn new(steps: &'a [Step]) -> Self {
        Self {
            steps,
            held: RefCell::new(Vec::new()),
        }
    }

    /// A directory above step `index` has `key`.
    pub(super) fn holds(&self, index: usize, key: Key) -> bool {
        let mut held = self.held.borrow_mut();
        if let Some((_, column)) = held.iter().find(|(at, _)| *at == key) {
            return column[index];
        }
        let mut column: Vec<bool> = Vec::with_capacity(self.steps.len());
        // The step above a step comes before it.
        for step in self.steps {
            column.push(
                step.up
                    .is_some_and(|up| column[up] || self.steps[up].own == Some(key)),
            );
        }
        let holds = column[index];
        held.push((key, column));
        holds
    }
}

/// The key of each directory on the way to `path`, which is resolved, from its root,
/// and that of the file at `path`, or `None` when a link or nothing is there. No link
/// is followed, so the directories stop at a name that is not one.
pub(super) fn keys(driver: &dyn Driver, path: &Path) -> (hash::Set<Key>, Option<Key>) {
    let mut above = hash::Set::default();
    let Ok(mut dir) = driver.root(path) else {
        return (above, None);
    };
    let mut names = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .peekable();
    while let Some(name) = names.next() {
        above.insert(dir.key());
        match dir.open(name) {
            Ok(Entry::Directory(next)) => dir = next,
            Ok(Entry::File(key)) if names.peek().is_none() => {
                return (above, Some(key));
            }
            _ => return (above, None),
        }
    }
    (above, Some(dir.key()))
}

/// A name of a path that the walk has yet to take.
#[derive(Debug)]
enum Name {
    Normal(OsString),
    Up,
    /// An end in `/` or `/.`: the step before it must be a directory.
    Directory,
}

#[derive(Debug)]
struct Walker<'a> {
    driver: &'a dyn Driver,
    /// The path of the walk, as given.
    given: &'a Path,
    /// `given` is absolute, with no bound on its length.
    rooted: bool,
    steps: Vec<Step>,
    names: VecDeque<Name>,
    /// The step of the directory that the next name is in.
    at: usize,
    /// Step `at`, open, or `None` when it is not a directory.
    dir: Option<Box<dyn dir::Driver>>,
    /// The links that this start of the walk can still follow.
    links: usize,
    /// The times that the walk started again.
    starts: usize,
    in_file: bool,
}

impl Walker<'_> {
    fn run(&mut self) -> Result<Result<(), Error>, Error> {
        if self.given.as_os_str().is_empty() {
            return Err(Error::NotFound);
        }
        self.begin()?;
        let end = loop {
            let Some(name) = self.names.pop_front() else {
                break Ok(());
            };
            let last = self
                .names
                .iter()
                .all(|name| matches!(name, Name::Directory));
            let taken = match name {
                Name::Normal(name) => match self.normal(&name, last) {
                    Ok(true) => break Ok(()),
                    Ok(false) => Ok(()),
                    Err(e) => Err(e),
                },
                Name::Up => match self.up() {
                    Ok(true) => self.again()?,
                    Ok(false) => Ok(()),
                    Err(e) => Err(e),
                },
                Name::Directory if self.dir.is_some() => Ok(()),
                Name::Directory => {
                    self.steps[self.at].back = true;
                    Err(self.refused())
                }
            };
            match taken {
                Ok(()) => {}
                Err(e) if last => break Err(e),
                Err(e) => return Err(e),
            }
        };
        if self.at + 1 != self.steps.len() {
            let step = self.steps[self.at].clone();
            self.steps.push(step);
        }
        Ok(end)
    }

    /// Starts the walk from the first name of the path, with no step.
    fn begin(&mut self) -> Result<(), Error> {
        self.steps.clear();
        self.names.clear();
        if self.rooted {
            self.start(self.given, names_directory(self.given))
        } else {
            self.root(self.given)
        }
    }

    /// Starts again at the root of the absolute path of `path`, with its names first.
    fn root(&mut self, path: &Path) -> Result<(), Error> {
        let absolute = self.driver.absolute(path)?;
        // `std::path::absolute` drops an end `/.`, so the end comes from `path`.
        self.start(&absolute, names_directory(path))
    }

    /// Starts again at the root of `absolute`, with its names first, and an end that
    /// must be a directory when `directory`.
    fn start(&mut self, absolute: &Path, directory: bool) -> Result<(), Error> {
        let root: PathBuf = absolute
            .components()
            .take_while(|part| {
                matches!(part, Component::Prefix(_) | Component::RootDir)
            })
            .collect();
        let dir = self.driver.root(absolute)?;
        self.steps.push(Step {
            name: root.into_os_string(),
            own: Some(dir.key()),
            up: None,
            back: false,
            directory: true,
        });
        self.enter(self.steps.len() - 1, Some(dir));
        self.push(absolute, directory);
        Ok(())
    }

    /// Puts the names of `path` before the rest, with an end that must be a directory
    /// when `directory`.
    fn push(&mut self, path: &Path, directory: bool) {
        let mut names: Vec<_> = path
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(Name::Normal(name.to_owned())),
                Component::ParentDir => Some(Name::Up),
                _ => None,
            })
            .collect();
        if directory && matches!(names.last(), Some(Name::Normal(_))) {
            names.push(Name::Directory);
        }
        for name in names.into_iter().rev() {
            self.names.push_front(name);
        }
    }

    /// Takes `name` in the directory of the walk. `Ok(true)` when nothing is at the
    /// last name, where the path ends.
    fn normal(&mut self, name: &OsStr, last: bool) -> Result<bool, Error> {
        let entry = match &self.dir {
            Some(dir) => dir.open(name),
            None => Err(self.driver.not_directory()),
        };
        let (own, directory) = match &entry {
            Ok(entry) => (Some(entry.key()), matches!(entry, Entry::Directory(_))),
            Err(_) => (None, false),
        };
        self.steps.push(Step {
            name: name.to_owned(),
            own,
            up: Some(self.at),
            back: false,
            directory,
        });
        let index = self.steps.len() - 1;
        match entry {
            Ok(Entry::Link(_, target)) => {
                let Some(links) = self.links.checked_sub(1) else {
                    self.steps[index].own = None;
                    self.enter(index, None);
                    return Err(self.driver.looped());
                };
                self.links = links;
                if target.has_root() {
                    self.root(&target)?;
                } else {
                    self.push(&target, names_directory(&target));
                }
                Ok(false)
            }
            Err(e) if !last || e != Error::NotFound => {
                self.in_file = self.is_file(self.at);
                self.enter(index, None);
                Err(e)
            }
            Ok(Entry::Directory(dir)) => {
                self.enter(index, Some(dir));
                Ok(false)
            }
            _ => {
                self.enter(index, None);
                Ok(own.is_none())
            }
        }
    }

    /// Takes `..`. `Ok(true)` when the host gives a directory other than the step
    /// above: a program moved a directory of the path during the walk.
    fn up(&mut self) -> Result<bool, Error> {
        // A step that fails `..` is still one that the walk would leave.
        self.steps[self.at].back = true;
        let Some(dir) = &self.dir else {
            return Err(self.refused());
        };
        // `..` of a root is the root.
        let Some(up) = self.steps[self.at].up else {
            return Ok(false);
        };
        let above = dir.up()?;
        if Some(above.key()) == self.steps[up].own {
            self.enter(up, Some(above));
            return Ok(false);
        }
        Ok(true)
    }

    /// Starts the walk again with [`LINKS`] links. `Ok(Err)` with the error of
    /// [`Driver::looped`] past [`STARTS`] starts. `Err` when the start fails, which
    /// leaves no step, so its error is not one of the end.
    fn again(&mut self) -> Result<Result<(), Error>, Error> {
        if self.starts == STARTS {
            return Ok(Err(self.driver.looped()));
        }
        self.starts += 1;
        self.links = LINKS;
        self.begin()?;
        Ok(Ok(()))
    }

    /// Makes step `index` the step of the walk, with `dir` open at it.
    fn enter(&mut self, index: usize, dir: Option<Box<dyn dir::Driver>>) {
        self.at = index;
        self.dir = dir;
    }

    /// The error of the host for a name in step `at`, which is not a directory.
    fn refused(&mut self) -> Error {
        // A step that is a file holds no names, so the host refuses each one.
        self.in_file = self.is_file(self.at);
        self.driver.not_directory()
    }

    /// Step `index` is a file. The walk is never in a link, so a step with a key that
    /// is not a directory is one.
    fn is_file(&self, index: usize) -> bool {
        let step = &self.steps[index];
        step.own.is_some() && !step.directory
    }
}
