#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseSelection {
    meta: bool,
    messages: bool,
    operations: bool,
    usage: bool,
    snapshots: bool,
    raw_unknown: bool,
}

impl ParseSelection {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            meta: false,
            messages: false,
            operations: false,
            usage: false,
            snapshots: false,
            raw_unknown: false,
        }
    }

    #[must_use]
    pub const fn full() -> Self {
        Self {
            meta: true,
            messages: true,
            operations: true,
            usage: true,
            snapshots: true,
            raw_unknown: true,
        }
    }

    #[must_use]
    pub const fn meta_only() -> Self {
        Self {
            meta: true,
            messages: false,
            operations: false,
            usage: false,
            snapshots: false,
            raw_unknown: false,
        }
    }

    #[must_use]
    pub const fn with_meta(mut self) -> Self {
        self.meta = true;
        self
    }

    #[must_use]
    pub const fn with_messages(mut self) -> Self {
        self.messages = true;
        self
    }

    #[must_use]
    pub const fn with_operations(mut self) -> Self {
        self.operations = true;
        self
    }

    #[must_use]
    pub const fn with_usage(mut self) -> Self {
        self.usage = true;
        self
    }

    #[must_use]
    pub const fn with_snapshots(mut self) -> Self {
        self.snapshots = true;
        self
    }

    #[must_use]
    pub const fn with_raw_unknown(mut self) -> Self {
        self.raw_unknown = true;
        self
    }

    #[must_use]
    pub const fn includes_meta(self) -> bool {
        self.meta
    }

    #[must_use]
    pub const fn includes_messages(self) -> bool {
        self.messages
    }

    #[must_use]
    pub const fn includes_operations(self) -> bool {
        self.operations
    }

    #[must_use]
    pub const fn includes_usage(self) -> bool {
        self.usage
    }

    #[allow(dead_code)]
    #[must_use]
    pub(crate) const fn includes_state(self) -> bool {
        self.is_full()
    }

    #[allow(dead_code)]
    #[must_use]
    pub(crate) const fn includes_state_records(self) -> bool {
        self.messages || self.operations || self.usage || self.snapshots || self.raw_unknown
    }

    #[must_use]
    pub const fn includes_snapshots(self) -> bool {
        self.snapshots
    }

    #[must_use]
    pub const fn includes_raw_unknown(self) -> bool {
        self.raw_unknown
    }

    #[must_use]
    pub const fn is_full(self) -> bool {
        self.meta
            && self.messages
            && self.operations
            && self.usage
            && self.snapshots
            && self.raw_unknown
    }

    #[must_use]
    pub const fn is_meta_only(self) -> bool {
        self.meta
            && !self.messages
            && !self.operations
            && !self.usage
            && !self.snapshots
            && !self.raw_unknown
    }
}

impl Default for ParseSelection {
    fn default() -> Self {
        Self::full()
    }
}
