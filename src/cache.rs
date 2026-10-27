//! Values fetched now and then from elsewhere, shared until they are too old.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct TimedCache<T> {
    max_age: Duration,
    slot: Mutex<Option<(Instant, Arc<T>)>>,
}

impl<T> TimedCache<T> {
    pub const fn new(max_age: Duration) -> Self {
        Self { max_age, slot: Mutex::const_new(None) }
    }

    /// The cached value, or a fresh one from `fetch`. Concurrent callers wait
    /// for one fetch rather than each making their own.
    pub async fn get_or_fetch<E>(&self, fetch: impl AsyncFnOnce() -> Result<T, E>) -> Result<Arc<T>, E> {
        let mut slot = self.slot.lock().await;
        if let Some((fetched, value)) = slot.as_ref()
            && fetched.elapsed() < self.max_age
        {
            return Ok(value.clone());
        }
        let value = Arc::new(fetch().await?);
        *slot = Some((Instant::now(), value.clone()));
        Ok(value)
    }
}

/// As [`TimedCache`], one value per key, such as per wiki. Keys come from
/// requests, so there are at most `capacity`; when full, the cache starts over.
/// A slow fetch for one key does not hold up the others.
pub struct TimedMap<K, V> {
    max_age: Duration,
    capacity: usize,
    entries: Mutex<Option<Entries<K, V>>>,
}

/// When each value was fetched.
type Entries<K, V> = HashMap<K, (Instant, Arc<V>)>;

impl<K: Eq + Hash, V> TimedMap<K, V> {
    pub const fn new(max_age: Duration, capacity: usize) -> Self {
        Self { max_age, capacity, entries: Mutex::const_new(None) }
    }

    /// The cached value for `key`, or a fresh one from `fetch`.
    pub async fn get_or_fetch<E>(&self, key: K, fetch: impl AsyncFnOnce() -> Result<V, E>) -> Result<Arc<V>, E> {
        if let Some((fetched, value)) = self.entries.lock().await.as_ref().and_then(|e| e.get(&key))
            && fetched.elapsed() < self.max_age
        {
            return Ok(value.clone());
        }
        let value = Arc::new(fetch().await?);
        let mut entries = self.entries.lock().await;
        let entries = entries.get_or_insert_with(HashMap::new);
        if entries.len() >= self.capacity {
            entries.clear();
        }
        entries.insert(key, (Instant::now(), value.clone()));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetches_once_until_stale() {
        let cache = TimedCache::new(Duration::from_secs(60));
        let first = cache.get_or_fetch(async || Ok::<_, ()>(1)).await.unwrap();
        let second = cache.get_or_fetch(async || Ok::<_, ()>(2)).await.unwrap();
        assert_eq!((*first, *second), (1, 1));

        let stale = TimedCache::new(Duration::ZERO);
        stale.get_or_fetch(async || Ok::<_, ()>(1)).await.unwrap();
        assert_eq!(*stale.get_or_fetch(async || Ok::<_, ()>(2)).await.unwrap(), 2);
        assert!(stale.get_or_fetch(async || Err::<i32, _>("down")).await.is_err());
    }

    #[tokio::test]
    async fn per_key_and_bounded() {
        let map = TimedMap::new(Duration::from_secs(60), 2);
        assert_eq!(*map.get_or_fetch("a", async || Ok::<_, ()>(1)).await.unwrap(), 1);
        assert_eq!(*map.get_or_fetch("b", async || Ok::<_, ()>(2)).await.unwrap(), 2);
        assert_eq!(*map.get_or_fetch("a", async || Ok::<_, ()>(9)).await.unwrap(), 1);
        // Full: starts over.
        assert_eq!(*map.get_or_fetch("c", async || Ok::<_, ()>(3)).await.unwrap(), 3);
        assert_eq!(*map.get_or_fetch("a", async || Ok::<_, ()>(4)).await.unwrap(), 4);
        assert!(map.get_or_fetch("d", async || Err::<i32, _>("down")).await.is_err());
    }
}
