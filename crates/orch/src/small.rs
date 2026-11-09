use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VecMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for VecMap<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: Ord, V> VecMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, k: &K) -> Result<usize, usize> {
        let mut i = 0;
        while i < self.entries.len() {
            match self.entries[i].0.cmp(k) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Equal => return Ok(i),
                std::cmp::Ordering::Greater => return Err(i),
            }
        }
        Err(i)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        let i = self.slot(k).ok()?;
        Some(&self.entries[i].1)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let i = self.slot(k).ok()?;
        Some(&mut self.entries[i].1)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.slot(k).is_ok()
    }

    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        match self.slot(&k) {
            Ok(i) => Some(std::mem::replace(&mut self.entries[i].1, v)),
            Err(i) => {
                self.entries.insert(i, (k, v));
                None
            }
        }
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        let i = self.slot(k).ok()?;
        Some(self.entries.remove(i).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, v)| v)
    }
}

impl<'a, K: Ord, V> IntoIterator for &'a VecMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, (K, V)>, fn(&'a (K, V)) -> (&'a K, &'a V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VecSet<T> {
    items: Vec<T>,
}

impl<T> Default for VecSet<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T: Ord> VecSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, x: &T) -> Result<usize, usize> {
        let mut i = 0;
        while i < self.items.len() {
            match self.items[i].cmp(x) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Equal => return Ok(i),
                std::cmp::Ordering::Greater => return Err(i),
            }
        }
        Err(i)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, x: &T) -> bool {
        self.slot(x).is_ok()
    }

    pub fn insert(&mut self, x: T) -> bool {
        match self.slot(&x) {
            Ok(_) => false,
            Err(i) => {
                self.items.insert(i, x);
                true
            }
        }
    }

    pub fn remove(&mut self, x: &T) -> bool {
        match self.slot(x) {
            Ok(i) => {
                self.items.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    pub fn first(&self) -> Option<&T> {
        self.items.first()
    }
}

impl<T: Ord> Extend<T> for VecSet<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for x in iter {
            self.insert(x);
        }
    }
}

impl<T: Ord> FromIterator<T> for VecSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut s = Self::new();
        s.extend(iter);
        s
    }
}

impl<'a, T: Ord> IntoIterator for &'a VecSet<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<K: Serialize, V: Serialize> Serialize for VecMap<K, V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(s)
    }
}

impl<'de, K: Ord + Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de> for VecMap<K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut m = Self::new();
        for (k, v) in Vec::<(K, V)>::deserialize(d)? {
            m.insert(k, v);
        }
        Ok(m)
    }
}

impl<T: Serialize> Serialize for VecSet<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.items.serialize(s)
    }
}

impl<'de, T: Ord + Deserialize<'de>> Deserialize<'de> for VecSet<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Vec::<T>::deserialize(d)?.into_iter().collect())
    }
}

impl<K: Ord, V> std::ops::Index<&K> for VecMap<K, V> {
    type Output = V;

    fn index(&self, k: &K) -> &V {
        self.get(k).expect("key present in VecMap")
    }
}
