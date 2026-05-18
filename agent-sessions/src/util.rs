use smol_str::SmolStr;
/// Convert a `&str` to `SmolStr`.
#[inline]
pub fn box_str(value: &str) -> SmolStr {
    SmolStr::from(value)
}

/// Convert a `Cow<str>` to `SmolStr`.
#[inline]
pub fn cow_to_box(value: std::borrow::Cow<'_, str>) -> SmolStr {
    match value {
        std::borrow::Cow::Borrowed(s) => SmolStr::from(s),
        std::borrow::Cow::Owned(s) => s.into(),
    }
}

/// Convert an `Option<Cow<str>>` to `Option<SmolStr>` using `cow_to_box`.
#[inline]
pub fn opt_cow_to_box(value: Option<std::borrow::Cow<'_, str>>) -> Option<SmolStr> {
    value.map(cow_to_box)
}
