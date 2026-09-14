use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SocketHandle {
    index: u16,
    generation: u16,
}

impl SocketHandle {
    pub const fn index(self) -> usize { self.index as usize }
    pub const fn generation(self) -> u16 { self.generation }
}

#[derive(Debug, Clone)]
pub struct SocketStorage<T> {
    generation: u16,
    value: Option<T>,
}

impl<T> SocketStorage<T> {
    pub const EMPTY: Self = Self { generation: 1, value: None };
    pub const fn is_empty(&self) -> bool { self.value.is_none() }
}

/// Generic socket collection backed entirely by caller-provided slots.
///
/// Handles contain an index and generation, so removing and reusing a slot
/// cannot accidentally make an old handle refer to a different socket.
#[derive(Debug)]
pub struct SocketSet<'a, T> {
    storage: &'a mut [SocketStorage<T>],
}

impl<'a, T> SocketSet<'a, T> {
    pub fn new(storage: &'a mut [SocketStorage<T>]) -> Self { Self { storage } }
    pub fn capacity(&self) -> usize { self.storage.len() }
    pub fn len(&self) -> usize { self.storage.iter().filter(|s| s.value.is_some()).count() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    pub fn add(&mut self, value: T) -> Result<SocketHandle> {
        let (index, slot) = self.storage.iter_mut().enumerate().find(|(_, s)| s.value.is_none()).ok_or(Error::BufferFull)?;
        if index > u16::MAX as usize { return Err(Error::BufferFull); }
        slot.value = Some(value);
        Ok(SocketHandle { index: index as u16, generation: slot.generation })
    }

    fn checked_slot(&self, handle: SocketHandle) -> Result<&SocketStorage<T>> {
        let slot = self.storage.get(handle.index()).ok_or(Error::InvalidField)?;
        if slot.generation != handle.generation || slot.value.is_none() { return Err(Error::InvalidState); }
        Ok(slot)
    }
    fn checked_slot_mut(&mut self, handle: SocketHandle) -> Result<&mut SocketStorage<T>> {
        let slot = self.storage.get_mut(handle.index()).ok_or(Error::InvalidField)?;
        if slot.generation != handle.generation || slot.value.is_none() { return Err(Error::InvalidState); }
        Ok(slot)
    }
    pub fn get(&self, handle: SocketHandle) -> Result<&T> { self.checked_slot(handle)?.value.as_ref().ok_or(Error::InvalidState) }
    pub fn get_mut(&mut self, handle: SocketHandle) -> Result<&mut T> { self.checked_slot_mut(handle)?.value.as_mut().ok_or(Error::InvalidState) }
    pub fn remove(&mut self, handle: SocketHandle) -> Result<T> {
        let slot = self.checked_slot_mut(handle)?;
        let value = slot.value.take().ok_or(Error::InvalidState)?;
        slot.generation = slot.generation.wrapping_add(1).max(1);
        Ok(value)
    }
    pub fn iter(&self) -> impl Iterator<Item = (SocketHandle, &T)> {
        self.storage.iter().enumerate().filter_map(|(index, slot)| {
            let value = slot.value.as_ref()?;
            Some((SocketHandle { index: index as u16, generation: slot.generation }, value))
        })
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (SocketHandle, &mut T)> {
        self.storage.iter_mut().enumerate().filter_map(|(index, slot)| {
            let generation = slot.generation;
            let value = slot.value.as_mut()?;
            Some((SocketHandle { index: index as u16, generation }, value))
        })
    }
    pub fn find(&self, mut predicate: impl FnMut(&T) -> bool) -> Option<SocketHandle> {
        self.iter().find_map(|(h, value)| predicate(value).then_some(h))
    }
}

#[cfg(feature = "alloc")]
mod owned {
    use alloc::vec::Vec;
    use super::{SocketHandle, SocketStorage};
    use crate::error::{Error, Result};

    /// Alloc-backed convenience SocketSet used by hosted/default builds.
    /// Bare-metal code should prefer [`super::SocketSet`].
    #[derive(Debug, Default, Clone)]
    pub struct OwnedSocketSet<T> {
        storage: Vec<SocketStorage<T>>,
    }

    impl<T> OwnedSocketSet<T> {
        pub const fn new() -> Self { Self { storage: Vec::new() } }
        pub fn len(&self) -> usize { self.storage.iter().filter(|s| s.value.is_some()).count() }
        pub fn is_empty(&self) -> bool { self.len() == 0 }
        pub fn add(&mut self, value: T) -> Result<SocketHandle> {
            if let Some((index, slot)) = self.storage.iter_mut().enumerate().find(|(_, s)| s.value.is_none()) {
                if index > u16::MAX as usize { return Err(Error::BufferFull); }
                slot.value = Some(value);
                return Ok(SocketHandle { index: index as u16, generation: slot.generation });
            }
            if self.storage.len() > u16::MAX as usize { return Err(Error::BufferFull); }
            let index = self.storage.len();
            let mut slot = SocketStorage::EMPTY;
            slot.value = Some(value);
            let generation = slot.generation;
            self.storage.push(slot);
            Ok(SocketHandle { index: index as u16, generation })
        }
        pub fn get(&self, handle: SocketHandle) -> Result<&T> {
            let slot = self.storage.get(handle.index()).ok_or(Error::InvalidField)?;
            if slot.generation != handle.generation { return Err(Error::InvalidState); }
            slot.value.as_ref().ok_or(Error::InvalidState)
        }
        pub fn get_mut(&mut self, handle: SocketHandle) -> Result<&mut T> {
            let slot = self.storage.get_mut(handle.index()).ok_or(Error::InvalidField)?;
            if slot.generation != handle.generation { return Err(Error::InvalidState); }
            slot.value.as_mut().ok_or(Error::InvalidState)
        }
        pub fn remove(&mut self, handle: SocketHandle) -> Result<T> {
            let slot = self.storage.get_mut(handle.index()).ok_or(Error::InvalidField)?;
            if slot.generation != handle.generation { return Err(Error::InvalidState); }
            let value = slot.value.take().ok_or(Error::InvalidState)?;
            slot.generation = slot.generation.wrapping_add(1).max(1);
            Ok(value)
        }
        pub fn iter(&self) -> impl Iterator<Item = (SocketHandle, &T)> {
            self.storage.iter().enumerate().filter_map(|(index, slot)| {
                let value = slot.value.as_ref()?;
                Some((SocketHandle { index: index as u16, generation: slot.generation }, value))
            })
        }
        pub fn iter_mut(&mut self) -> impl Iterator<Item = (SocketHandle, &mut T)> {
            self.storage.iter_mut().enumerate().filter_map(|(index, slot)| {
                let generation = slot.generation;
                let value = slot.value.as_mut()?;
                Some((SocketHandle { index: index as u16, generation }, value))
            })
        }
        pub fn find(&self, mut predicate: impl FnMut(&T) -> bool) -> Option<SocketHandle> {
            self.iter().find_map(|(h, value)| predicate(value).then_some(h))
        }
    }
    pub use OwnedSocketSet as PublicOwnedSocketSet;
}

#[cfg(feature = "alloc")]
pub use owned::PublicOwnedSocketSet as OwnedSocketSet;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn borrowed_socket_set_rejects_stale_handle() {
        let mut slots = [SocketStorage::<u32>::EMPTY, SocketStorage::<u32>::EMPTY];
        let mut set = SocketSet::new(&mut slots);
        let h = set.add(10).unwrap();
        assert_eq!(*set.get(h).unwrap(), 10);
        assert_eq!(set.remove(h).unwrap(), 10);
        let h2 = set.add(20).unwrap();
        assert_ne!(h, h2);
        assert_eq!(set.get(h), Err(Error::InvalidState));
        assert_eq!(*set.get(h2).unwrap(), 20);
    }
    #[test]
    fn borrowed_socket_set_is_strictly_bounded() {
        let mut slots = [SocketStorage::<u8>::EMPTY];
        let mut set = SocketSet::new(&mut slots);
        set.add(1).unwrap();
        assert_eq!(set.add(2), Err(Error::BufferFull));
    }
}
