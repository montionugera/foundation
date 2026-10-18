//! Faults for the dedicated thread starts of a node.

/// The faults aimed at the thread starts of one node, by name.
#[derive(Default)]
pub(crate) struct Faults(Vec<String>);

impl Faults {
    pub(crate) fn fail(&mut self, name: &str) {
        self.0.push(name.into());
    }

    /// Takes the first fault aimed at a start named `name`, and returns whether there
    /// was one.
    pub(crate) fn take(&mut self, name: &str) -> bool {
        let Some(at) = self.0.iter().position(|aimed| aimed == name) else {
            return false;
        };
        self.0.remove(at);
        true
    }
}
