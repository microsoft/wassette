//! Composable async iteration.

/// A trait for dealing with async iterators.
pub trait AsyncIterator {
    /// The type of the elements being iterated over.
    type Item;

    /// Advances the iterator and returns the next value.
    async fn next(&mut self) -> Option<Self::Item>;
}
