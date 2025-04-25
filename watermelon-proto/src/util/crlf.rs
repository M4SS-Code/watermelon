#[derive(Debug)]
pub(crate) struct CrlfFinder(memchr::memmem::Finder<'static>);

impl CrlfFinder {
    pub(crate) fn new() -> Self {
        Self(memchr::memmem::Finder::new(b"\r\n"))
    }

    pub(crate) fn find(&self, haystack: &[u8]) -> Option<usize> {
        self.0.find(haystack)
    }
}
