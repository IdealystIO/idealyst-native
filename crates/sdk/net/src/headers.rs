/// Case-insensitive header map.
///
/// Stored as a `Vec<(name, value)>` rather than a `HashMap` for two
/// reasons: (1) the typical request has 3-5 headers and a vec is
/// faster than hashing at that size, and (2) duplicate-allowed
/// headers like `Set-Cookie` keep their original order.
///
/// Header names are stored lowercase for case-insensitive lookup but
/// retain whatever spelling the caller used on iteration (we lowercase
/// on insert; the spec says comparison is case-insensitive, so the
/// canonical wire form is fine to canonicalize here).
#[derive(Debug, Clone, Default)]
pub struct Headers {
    entries: Vec<(String, String)>,
}

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a header. Duplicates the entry if the name already exists —
    /// matches HTTP semantics (e.g. multiple `Set-Cookie`). Use [`set`]
    /// for replace-or-insert semantics.
    pub fn append(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.entries.push((name.into().to_ascii_lowercase(), value.into()));
    }

    /// Insert or replace. Removes any existing entries with the same name
    /// first, then appends. Use for single-valued headers like
    /// `Content-Type` where the caller wants exactly one value.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into().to_ascii_lowercase();
        self.entries.retain(|(n, _)| n != &name);
        self.entries.push((name, value.into()));
    }

    /// Insert only if the name is not already present. Used by the
    /// request builder to apply a body's default Content-Type without
    /// stomping a user-supplied one.
    pub fn set_if_absent(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into().to_ascii_lowercase();
        if !self.entries.iter().any(|(n, _)| n == &name) {
            self.entries.push((name, value.into()));
        }
    }

    /// Return the first value matching `name` (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&str> {
        let needle = name.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|(n, _)| n == &needle)
            .map(|(_, v)| v.as_str())
    }

    /// Iterate every entry as `(lowercase-name, value)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
