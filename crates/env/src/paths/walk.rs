//! The walk of a path, name by name, as the host takes it.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::iter;
use std::path::{Component, Path, PathBuf};

use super::{Driver, Entry, Error, Key, LINKS, names_directory};

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
        let mut walker = Walker {
            driver,
            steps: Vec::new(),
            names: VecDeque::new(),
            at: 0,
            path: PathBuf::new(),
            links: LINKS,
            in_file: false,
        };
        let end = walker.run(path);
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
    steps: Vec<Step>,
    names: VecDeque<Name>,
    /// The step of the directory that the next name is in.
    at: usize,
    /// The resolved path of step `at`.
    path: PathBuf,
    /// The links that the walk can still follow.
    links: usize,
    in_file: bool,
}

impl Walker<'_> {
    fn run(&mut self, path: &Path) -> Result<Result<(), Error>, Error> {
        if path.as_os_str().is_empty() {
            return Err(Error::NotFound);
        }
        self.root(path)?;
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
                Name::Up => self.up(),
                Name::Directory => self
                    .directory(".")
                    .inspect_err(|_| self.steps[self.at].back = true),
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

    /// Starts again at the root of the absolute path of `path`, with its names first.
    fn root(&mut self, path: &Path) -> Result<(), Error> {
        let absolute = self.driver.absolute(path)?;
        let root: PathBuf = absolute
            .components()
            .take_while(|part| {
                matches!(part, Component::Prefix(_) | Component::RootDir)
            })
            .collect();
        let entry = self.driver.entry(&root)?;
        self.steps.push(Step {
            name: root.clone().into_os_string(),
            own: Some(entry.key()),
            up: None,
            back: false,
            directory: matches!(entry, Entry::Directory(_)),
        });
        self.at = self.steps.len() - 1;
        self.path = root;
        // `std::path::absolute` drops an end `/.`, so the end comes from `path`.
        self.push(&absolute, names_directory(path));
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
        let entry = self.driver.entry(&self.path.join(name));
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
                    self.enter(index);
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
                self.enter(index);
                Err(e)
            }
            _ => {
                self.enter(index);
                Ok(own.is_none())
            }
        }
    }

    fn up(&mut self) -> Result<(), Error> {
        // A step that fails `..` is still one that the walk would leave.
        self.steps[self.at].back = true;
        self.directory("..")?;
        if let Some(up) = self.steps[self.at].up {
            self.at = up;
            self.path.pop();
        }
        Ok(())
    }

    /// Makes step `index`, a name in the directory of step `at`, the step of the walk.
    fn enter(&mut self, index: usize) {
        self.at = index;
        self.path.push(&self.steps[index].name);
    }

    /// The error of the host for `name` in a step that is not a directory.
    fn directory(&mut self, name: &str) -> Result<(), Error> {
        if self.steps[self.at].directory {
            return Ok(());
        }
        let entry = self.driver.entry(&self.path.join(name));
        // A step that is a file holds no names, so the host refuses each one.
        self.in_file = self.is_file(self.at);
        entry.map(|_| ())
    }

    /// Step `index` is a file. The walk is never in a link, so a step with a key that
    /// is not a directory is one.
    fn is_file(&self, index: usize) -> bool {
        let step = &self.steps[index];
        step.own.is_some() && !step.directory
    }
}
