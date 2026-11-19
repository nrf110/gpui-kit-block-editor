/// Structural role of a collaborative block. Editable blocks own Markdown;
/// list and quote containers own their child ordering instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionKind {
    Text,
    List { ordered: bool, start: u64 },
    ListItem { checked: Option<bool> },
    Quote,
}

impl SectionKind {
    pub fn is_editable(&self) -> bool {
        matches!(self, Self::Text | Self::ListItem { .. })
    }
}

/// Markdown imported as a tree before stable CRDT identities are assigned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSpec {
    pub kind: SectionKind,
    pub markdown: String,
    pub children: Vec<BlockSpec>,
}
