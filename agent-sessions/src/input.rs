#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputMetadata<'a> {
    pub name: Option<&'a str>,
    pub media_type: Option<&'a str>,
}

impl<'a> InputMetadata<'a> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            name: None,
            media_type: None,
        }
    }

    #[must_use]
    pub fn name(mut self, name: &'a str) -> Self {
        self.name = Some(name);
        self
    }

    #[must_use]
    pub fn media_type(mut self, media_type: &'a str) -> Self {
        self.media_type = Some(media_type);
        self
    }
}
