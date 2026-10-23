//! A value fetched now and then from elsewhere, shared until it is too old.

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
}
