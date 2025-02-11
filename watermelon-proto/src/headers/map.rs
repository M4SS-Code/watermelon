use alloc::{
    collections::{btree_map::Entry, BTreeMap},
    vec,
    vec::Vec,
};
use core::{iter, mem, slice};

use super::{HeaderName, HeaderValue};

static EMPTY_HEADERS: OneOrMany = OneOrMany::Many(Vec::new());

/// A set of NATS headers
///
/// [`HeaderMap`] is a multimap of [`HeaderName`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderMap {
    headers: BTreeMap<HeaderName, OneOrMany>,
    len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OneOrMany {
    One(HeaderValue),
    Many(Vec<HeaderValue>),
}

impl HeaderMap {
    /// Create an empty `HeaderMap`
    ///
    /// The map will be created without any capacity. This function will not allocate.
    ///
    /// Consider using the [`FromIterator`], [`Extend`] implementations if the final
    /// length is known upfront.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            headers: BTreeMap::new(),
            len: 0,
        }
    }

    pub fn get(&self, name: &HeaderName) -> Option<&HeaderValue> {
        self.get_all(name).next()
    }

    pub fn get_all<'a>(
        &'a self,
        name: &HeaderName,
    ) -> impl DoubleEndedIterator<Item = &'a HeaderValue> {
        self.headers.get(name).unwrap_or(&EMPTY_HEADERS).iter()
    }

    pub fn insert(&mut self, name: HeaderName, value: HeaderValue) {
        if let Some(prev) = self.headers.insert(name, OneOrMany::One(value)) {
            self.len -= prev.len();
        }
        self.len += 1;
    }

    pub fn append(&mut self, name: HeaderName, value: HeaderValue) {
        match self.headers.entry(name) {
            Entry::Vacant(vacant) => {
                vacant.insert(OneOrMany::One(value));
            }
            Entry::Occupied(mut occupied) => {
                occupied.get_mut().push(value);
            }
        }
        self.len += 1;
    }

    pub fn remove(&mut self, name: &HeaderName) {
        if let Some(prev) = self.headers.remove(name) {
            self.len -= prev.len();
        }
    }

    /// Returns the number of keys stored in the map
    ///
    /// This number will be less than or equal to [`HeaderMap::len`].
    #[must_use]
    pub fn keys_len(&self) -> usize {
        self.headers.len()
    }

    /// Returns the number of headers stored in the map
    ///
    /// This number represents the total number of **values** stored in the map.
    /// This number can be greater than or equal to the number of **keys** stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the map contains no elements
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    /// Clear the map, removing all key-value pairs. Keeps the allocated memory for reuse
    pub fn clear(&mut self) {
        self.headers.clear();
        self.len = 0;
    }

    #[cfg(test)]
    fn keys(&self) -> impl Iterator<Item = &'_ HeaderName> {
        self.headers.keys()
    }

    pub(crate) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&'_ HeaderName, impl Iterator<Item = &'_ HeaderValue>)>
    {
        self.headers
            .iter()
            .map(|(name, value)| (name, value.iter()))
    }
}

impl FromIterator<(HeaderName, HeaderValue)> for HeaderMap {
    fn from_iter<I: IntoIterator<Item = (HeaderName, HeaderValue)>>(iter: I) -> Self {
        let mut this = Self::new();
        this.extend(iter);
        this
    }
}

impl Extend<(HeaderName, HeaderValue)> for HeaderMap {
    fn extend<T: IntoIterator<Item = (HeaderName, HeaderValue)>>(&mut self, iter: T) {
        iter.into_iter().for_each(|(name, value)| {
            self.append(name, value);
        });
    }
}

impl Default for HeaderMap {
    fn default() -> Self {
        Self::new()
    }
}

impl OneOrMany {
    fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(vec) => vec.len(),
        }
    }

    fn push(&mut self, item: HeaderValue) {
        match self {
            Self::One(current_item) => {
                let current_item =
                    mem::replace(current_item, HeaderValue::from_static("replacing"));
                *self = Self::Many(vec![current_item, item]);
            }
            Self::Many(vec) => {
                debug_assert!(!vec.is_empty(), "OneOrMany can't be empty");
                vec.push(item);
            }
        }
    }

    fn iter(&self) -> impl DoubleEndedIterator<Item = &'_ HeaderValue> {
        enum Either<'a> {
            A(iter::Once<&'a HeaderValue>),
            B(slice::Iter<'a, HeaderValue>),
        }

        impl<'a> Iterator for Either<'a> {
            type Item = &'a HeaderValue;

            fn next(&mut self) -> Option<Self::Item> {
                match self {
                    Self::A(a) => a.next(),
                    Self::B(b) => b.next(),
                }
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                match self {
                    Self::A(a) => a.size_hint(),
                    Self::B(b) => b.size_hint(),
                }
            }

            fn last(mut self) -> Option<Self::Item> {
                self.next_back()
            }

            fn fold<B, F>(self, init: B, f: F) -> B
            where
                F: FnMut(B, Self::Item) -> B,
            {
                match self {
                    Self::A(a) => a.fold(init, f),
                    Self::B(b) => b.fold(init, f),
                }
            }
        }

        impl DoubleEndedIterator for Either<'_> {
            fn next_back(&mut self) -> Option<Self::Item> {
                match self {
                    Self::A(a) => a.next_back(),
                    Self::B(b) => b.next_back(),
                }
            }

            fn rfold<B, F>(self, init: B, f: F) -> B
            where
                F: FnMut(B, Self::Item) -> B,
            {
                match self {
                    Self::A(a) => a.rfold(init, f),
                    Self::B(b) => b.rfold(init, f),
                }
            }
        }

        match self {
            Self::One(one) => Either::A(iter::once(one)),
            Self::Many(many) => Either::B(many.iter()),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use crate::headers::{HeaderName, HeaderValue};

    use super::HeaderMap;

    #[test]
    fn manual() {
        let mut headers = HeaderMap::new();
        headers.append(
            HeaderName::from_static("Nats-Message-Id"),
            HeaderValue::from_static("abcd"),
        );
        headers.append(
            HeaderName::from_static("Nats-Sequence"),
            HeaderValue::from_static("1"),
        );
        headers.append(
            HeaderName::from_static("Nats-Message-Id"),
            HeaderValue::from_static("1234"),
        );
        headers.append(
            HeaderName::from_static("Nats-Time-Stamp"),
            HeaderValue::from_static("0"),
        );
        headers.remove(&HeaderName::from_static("Nats-Time-Stamp"));

        verify_header_map(&headers);
    }

    #[test]
    fn collect() {
        let headers = [
            (
                HeaderName::from_static("Nats-Message-Id"),
                HeaderValue::from_static("abcd"),
            ),
            (
                HeaderName::from_static("Nats-Sequence"),
                HeaderValue::from_static("1"),
            ),
            (
                HeaderName::from_static("Nats-Message-Id"),
                HeaderValue::from_static("1234"),
            ),
        ]
        .into_iter()
        .collect::<HeaderMap>();

        verify_header_map(&headers);
    }

    fn verify_header_map(headers: &HeaderMap) {
        assert_eq!(
            [
                HeaderName::from_static("Nats-Message-Id"),
                HeaderName::from_static("Nats-Sequence")
            ]
            .as_slice(),
            headers.keys().cloned().collect::<Vec<_>>().as_slice()
        );

        let raw_headers = headers
            .iter()
            .map(|(name, values)| (name.clone(), values.cloned().collect::<Vec<_>>()))
            .collect::<Vec<_>>();
        assert_eq!(
            [
                (
                    HeaderName::from_static("Nats-Message-Id"),
                    vec![
                        HeaderValue::from_static("abcd"),
                        HeaderValue::from_static("1234")
                    ]
                ),
                (
                    HeaderName::from_static("Nats-Sequence"),
                    vec![HeaderValue::from_static("1")]
                ),
            ]
            .as_slice(),
            raw_headers.as_slice(),
        );
    }
}
