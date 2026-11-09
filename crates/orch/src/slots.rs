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
    items: [T; CAP],
    len: usize,
}

#[cfg(kani)]
pub struct Iter<'a, T> {
    items: &'a [T; CAP],
    len: usize,
    at: usize,
}

#[cfg(kani)]
impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        if self.at < self.len {
            self.len -= 1;
            Some(cell(self.items, self.len))
        } else {
            None
        }
    }
}

#[cfg(kani)]
impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.at < self.len {
            let x = cell(self.items, self.at);
            self.at += 1;
            Some(x)
        } else {
            None
        }
    }
}

#[cfg(kani)]
macro_rules! each {
    ($k:ident => $body:expr) => {{
        {
            let $k: usize = 0;
            $body;
        }
        {
            let $k: usize = 1;
            $body;
        }
        {
            let $k: usize = 2;
            $body;
        }
        {
            let $k: usize = 3;
            $body;
        }
    }};
}

#[cfg(kani)]
struct IterMut<'a, T> {
    refs: [Option<&'a mut T>; CAP],
    len: usize,
    at: usize,
}

#[cfg(kani)]
impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<&'a mut T> {
        if self.at < self.len {
            let x = self.refs[self.at].take();
            self.at += 1;
            x
        } else {
            None
        }
    }
}

#[cfg(kani)]
fn blank<T>() -> T {
    unsafe { std::mem::zeroed() }
}

#[cfg(kani)]
fn cell<T>(items: &[T; CAP], i: usize) -> &T {
    let [a, b, c, d] = items;
    match i {
        0 => a,
        1 => b,
        2 => c,
        _ => d,
    }
}

#[cfg(kani)]
fn cell_mut<T>(items: &mut [T; CAP], i: usize) -> &mut T {
    let [a, b, c, d] = items;
    match i {
        0 => a,
        1 => b,
        2 => c,
        _ => d,
    }
}

#[cfg(kani)]
impl<T> Slots<T> {
    pub fn new() -> Self {
        Self {
            items: blank(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        if i < self.len {
            Some(cell(&self.items, i))
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i < self.len {
            Some(cell_mut(&mut self.items, i))
        } else {
            None
        }
    }

    fn shift_up(&mut self, j: usize, i: usize) {
        if j > i && j <= self.len {
            let v = std::mem::replace(&mut self.items[j - 1], blank());
            self.items[j] = v;
        }
    }

    fn shift_down(&mut self, j: usize, i: usize) {
        if j >= i && j + 1 < self.len {
            let v = std::mem::replace(&mut self.items[j + 1], blank());
            self.items[j] = v;
        }
    }

    pub fn insert(&mut self, i: usize, x: T) {
        assert!(self.len < CAP && i <= self.len);
        self.shift_up(3, i);
        self.shift_up(2, i);
        self.shift_up(1, i);
        let mut x = x;
        each!(k => if k == i {
            self.items[k] = std::mem::replace(&mut x, blank());
        });
        self.len += 1;
    }

    pub fn remove(&mut self, i: usize) -> T {
        assert!(i < self.len);
        let mut x = blank();
        each!(k => if k == i {
            x = std::mem::replace(&mut self.items[k], blank());
        });
        self.shift_down(0, i);
        self.shift_down(1, i);
        self.shift_down(2, i);
        self.len -= 1;
        x
    }

    pub fn push(&mut self, x: T) {
        let n = self.len;
        self.insert(n, x);
    }

    pub fn clear(&mut self) {
        self.items = blank();
        self.len = 0;
    }

    pub fn retain(&mut self, mut f: impl FnMut(&T) -> bool) {
        let mut out: [T; CAP] = blank();
        let mut w = 0;
        each!(r => if r < self.len {
            let mut x = std::mem::replace(&mut self.items[r], blank());
            if f(&x) {
                each!(k => if k == w {
                    out[k] = std::mem::replace(&mut x, blank());
                });
                w += 1;
            }
        });
        self.items = out;
        self.len = w;
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            items: &self.items,
            len: self.len,
            at: 0,
        }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        let len = self.len;
        let [a, b, c, d] = &mut self.items;
        IterMut {
            refs: [Some(a), Some(b), Some(c), Some(d)],
            len,
            at: 0,
        }
    }
}

#[cfg(kani)]
pub struct IntoIter<T> {
    items: [T; CAP],
    len: usize,
    at: usize,
}

#[cfg(kani)]
impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.at < self.len {
            let x = std::mem::replace(cell_mut(&mut self.items, self.at), blank());
            self.at += 1;
            Some(x)
        } else {
            None
        }
    }
}

#[cfg(kani)]
impl<T> IntoIterator for Slots<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            items: self.items,
            len: self.len,
            at: 0,
        }
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
