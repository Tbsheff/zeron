use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(not(kani))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Slots<T> {
    items: Vec<T>,
}

#[cfg(not(kani))]
pub type Iter<'a, T> = std::slice::Iter<'a, T>;

#[cfg(not(kani))]
impl<T> Slots<T> {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        self.items.get(i)
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.items.get_mut(i)
    }

    pub fn insert(&mut self, i: usize, x: T) {
        self.items.insert(i, x);
    }

    pub fn remove(&mut self, i: usize) -> T {
        self.items.remove(i)
    }

    pub fn push(&mut self, x: T) {
        self.items.push(x);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn retain(&mut self, f: impl FnMut(&T) -> bool) {
        self.items.retain(f);
    }

    pub fn iter(&self) -> Iter<'_, T> {
        self.items.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.items.iter_mut()
    }
}

#[cfg(not(kani))]
impl<T> IntoIterator for Slots<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

#[cfg(kani)]
pub const CAP: usize = 4;

#[cfg(kani)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Slots<T> {
    items: [Option<T>; CAP],
    len: usize,
}

#[cfg(kani)]
pub type Iter<'a, T> =
    std::iter::FilterMap<std::slice::Iter<'a, Option<T>>, fn(&'a Option<T>) -> Option<&'a T>>;

#[cfg(kani)]
impl<T> Slots<T> {
    pub fn new() -> Self {
        Self {
            items: [const { None }; CAP],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        if i < self.len {
            self.items[i].as_ref()
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i < self.len {
            self.items[i].as_mut()
        } else {
            None
        }
    }

    pub fn insert(&mut self, i: usize, x: T) {
        assert!(self.len < CAP && i <= self.len);
        let mut j = self.len;
        while j > i {
            self.items[j] = self.items[j - 1].take();
            j -= 1;
        }
        self.items[i] = Some(x);
        self.len += 1;
    }

    pub fn remove(&mut self, i: usize) -> T {
        assert!(i < self.len);
        let x = self.items[i].take();
        let mut j = i;
        while j + 1 < self.len {
            self.items[j] = self.items[j + 1].take();
            j += 1;
        }
        self.len -= 1;
        x.expect("slot in range")
    }

    pub fn push(&mut self, x: T) {
        let n = self.len;
        self.insert(n, x);
    }

    pub fn clear(&mut self) {
        for slot in self.items[..self.len].iter_mut() {
            *slot = None;
        }
        self.len = 0;
    }

    pub fn retain(&mut self, mut f: impl FnMut(&T) -> bool) {
        let mut w = 0;
        let mut r = 0;
        while r < self.len {
            if let Some(x) = self.items[r].take() {
                if f(&x) {
                    self.items[w] = Some(x);
                    w += 1;
                }
            }
            r += 1;
        }
        self.len = w;
    }

    pub fn iter(&self) -> Iter<'_, T> {
        self.items[..self.len].iter().filter_map(Option::as_ref)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.items[..self.len].iter_mut().filter_map(Option::as_mut)
    }
}

#[cfg(kani)]
impl<T> IntoIterator for Slots<T> {
    type Item = T;
    type IntoIter = std::iter::Flatten<std::iter::Take<std::array::IntoIter<Option<T>, CAP>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len).flatten()
    }
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Slots<T> {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }

    pub fn last(&self) -> Option<&T> {
        self.len().checked_sub(1).and_then(|i| self.get(i))
    }

    pub fn front(&self) -> Option<&T> {
        self.first()
    }

    pub fn push_back(&mut self, x: T) {
        self.push(x);
    }

    pub fn pop(&mut self) -> Option<T> {
        let i = self.len().checked_sub(1)?;
        Some(self.remove(i))
    }

    pub fn pop_front(&mut self) -> Option<T> {
        if self.is_empty() {
            None
        } else {
            Some(self.remove(0))
        }
    }

    pub fn contains(&self, x: &T) -> bool
    where
        T: PartialEq,
    {
        self.iter().any(|y| y == x)
    }
}

impl<T> FromIterator<T> for Slots<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut s = Self::new();
        for x in iter {
            s.push(x);
        }
        s
    }
}

impl<'a, T> IntoIterator for &'a Slots<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Serialize> Serialize for Slots<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.iter())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Slots<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Vec::<T>::deserialize(d)?.into_iter().collect())
    }
}
