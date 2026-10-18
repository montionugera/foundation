//! What the body of each kind of block takes: the one list of keys that its check
//! reads, and the text that the docs give.

/// What the body of one kind of block takes. Its check refuses each attribute and
/// block that it does not state, and `foundation docs` states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Body {
    /// One sentence on what the body defines, with the `kind` that picks it when its
    /// block has more than one form.
    pub summary: &'static str,
    /// Each attribute that the body takes, in the order that the docs give them.
    pub attributes: &'static [Attribute],
    /// Each block that the body can hold.
    pub blocks: &'static [Block],
    /// Examples that the check takes with no diagnostic. Each attribute and block
    /// above is in at least one. The body of a block inside another body has none:
    /// the examples of that body show it.
    pub examples: &'static [&'static str],
}

/// One kind of block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// The keyword of the block, such as `channel`.
    pub keyword: &'static str,
    /// What the label names, or `None` for a block with no label.
    pub label: Option<&'static str>,
    /// The body of each form of the block, picked by its `kind`.
    pub forms: &'static [Body],
}

/// One attribute of a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attribute {
    /// The key of the attribute.
    pub name: &'static str,
    /// The value it takes, as the docs state it, such as `a string`.
    pub value: &'static str,
    /// `None` when the attribute is required, or the value that its absence gives.
    pub default: Option<&'static str>,
    /// One sentence on what the attribute sets.
    pub summary: &'static str,
}
