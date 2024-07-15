use std::{
    borrow::Borrow,
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

pub(crate) struct ExpiringMap<K, V>
where
    K: Hash + Eq,
{
    data: HashMap<K, Value<V>>,
    duration: Duration,
}

impl<K: Hash + Eq, V> ExpiringMap<K, V> {
    pub fn new(duration: Duration) -> Self {
        Self {
            data: HashMap::new(),
            duration,
        }
    }

    fn cleanup(&mut self) {
        let now = Instant::now();
        self.data.retain(|_, v| v.expiration > now);
        self.data.shrink_to_fit();
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.cleanup();
        self.data
            .insert(key, Value::new(value, Instant::now() + self.duration))
            .map(|v| v.value)
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Ord + Hash,
    {
        self.data
            .get(key)
            .filter(|v| !v.is_expired())
            .map(Value::value)
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: ?Sized + Ord + Hash,
    {
        self.get(key).is_some()
    }
}

struct Value<T> {
    value: T,
    expiration: Instant,
}

impl<T> Value<T> {
    fn new(value: T, expiration: Instant) -> Self {
        Self { value, expiration }
    }

    fn value(&self) -> &T {
        &self.value
    }

    fn is_expired(&self) -> bool {
        self.expiration < Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::ExpiringMap;
    use std::{thread::sleep, time::Duration};

    #[test]
    fn test_insert() {
        let mut cache = ExpiringMap::new(Duration::from_millis(50));
        cache.insert("key", "value");
        assert_eq!(cache.get("key"), Some(&"value"));
        sleep(Duration::from_millis(60));
        assert!(cache.get("key").is_none());
        cache.cleanup();
        cache.insert("a", "b");
        assert_eq!(cache.data.len(), 1);
    }
}
