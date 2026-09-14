use crate::error::{Error, Result};

/// Fixed-capacity FIFO over caller-owned storage.
///
/// No allocation is performed by the buffer. The caller controls both the
/// element type and exact capacity by supplying the backing slice.
#[derive(Debug)]
pub struct RingBuffer<'a, T: Copy> {
    storage: &'a mut [T],
    head: usize,
    len: usize,
}

impl<'a, T: Copy> RingBuffer<'a, T> {
    pub fn new(storage: &'a mut [T]) -> Self {
        Self { storage, head: 0, len: 0 }
    }

    pub const fn len(&self) -> usize { self.len }
    pub const fn is_empty(&self) -> bool { self.len == 0 }
    pub fn capacity(&self) -> usize { self.storage.len() }
    pub fn free(&self) -> usize { self.capacity().saturating_sub(self.len) }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    pub fn push_back(&mut self, value: T) -> Result<()> {
        if self.len == self.storage.len() || self.storage.is_empty() {
            return Err(Error::BufferFull);
        }
        let idx = (self.head + self.len) % self.storage.len();
        self.storage[idx] = value;
        self.len += 1;
        Ok(())
    }

    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 { return None; }
        let value = self.storage[self.head];
        self.head = (self.head + 1) % self.storage.len();
        self.len -= 1;
        if self.len == 0 { self.head = 0; }
        Some(value)
    }

    pub fn front(&self) -> Option<&T> {
        if self.len == 0 { None } else { Some(&self.storage[self.head]) }
    }
}

/// Metadata slot used by [`PacketBuffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketMetadata<M: Copy> {
    meta: Option<M>,
    offset: usize,
    len: usize,
}

impl<M: Copy> PacketMetadata<M> {
    pub const EMPTY: Self = Self { meta: None, offset: 0, len: 0 };
    pub const fn is_used(&self) -> bool { self.meta.is_some() }
}

/// Variable-length packet/message FIFO using caller-provided metadata and byte
/// storage. Packets never straddle the end of the byte arena, which means a
/// queued packet is always available as one contiguous slice.
#[derive(Debug)]
pub struct PacketBuffer<'a, M: Copy> {
    metadata: &'a mut [PacketMetadata<M>],
    payload: &'a mut [u8],
    head: usize,
    len: usize,
    used_bytes: usize,
}

impl<'a, M: Copy> PacketBuffer<'a, M> {
    pub fn new(metadata: &'a mut [PacketMetadata<M>], payload: &'a mut [u8]) -> Self {
        for slot in metadata.iter_mut() { *slot = PacketMetadata::EMPTY; }
        Self { metadata, payload, head: 0, len: 0, used_bytes: 0 }
    }

    pub const fn len(&self) -> usize { self.len }
    pub const fn is_empty(&self) -> bool { self.len == 0 }
    pub fn packet_capacity(&self) -> usize { self.metadata.len() }
    pub fn payload_capacity(&self) -> usize { self.payload.len() }
    pub const fn used_payload_bytes(&self) -> usize { self.used_bytes }
    pub fn free_payload_bytes(&self) -> usize { self.payload.len().saturating_sub(self.used_bytes) }

    fn meta_index(&self, logical: usize) -> usize {
        (self.head + logical) % self.metadata.len()
    }

    fn allocation_offset(&self, length: usize) -> Option<usize> {
        if length > self.payload.len() { return None; }
        if self.len == 0 { return Some(0); }

        let first = self.metadata[self.head];
        let last = self.metadata[self.meta_index(self.len - 1)];
        let first_off = first.offset;
        let last_end = last.offset + last.len;

        if last_end >= first_off {
            if self.payload.len().saturating_sub(last_end) >= length {
                Some(last_end)
            } else if first_off >= length {
                Some(0)
            } else {
                None
            }
        } else if first_off.saturating_sub(last_end) >= length {
            Some(last_end)
        } else {
            None
        }
    }

    pub fn enqueue(&mut self, meta: M, data: &[u8]) -> Result<()> {
        if self.metadata.is_empty() || self.len == self.metadata.len() {
            return Err(Error::BufferFull);
        }
        let offset = self.allocation_offset(data.len()).ok_or(Error::BufferFull)?;
        let idx = self.meta_index(self.len);
        self.payload[offset..offset + data.len()].copy_from_slice(data);
        self.metadata[idx] = PacketMetadata { meta: Some(meta), offset, len: data.len() };
        self.len += 1;
        self.used_bytes += data.len();
        Ok(())
    }

    pub fn peek(&self) -> Option<(M, &[u8])> {
        if self.len == 0 { return None; }
        let slot = self.metadata[self.head];
        let meta = slot.meta?;
        Some((meta, &self.payload[slot.offset..slot.offset + slot.len]))
    }

    /// Copy and remove the oldest packet. Returns `(metadata, payload_len)`.
    pub fn dequeue_into(&mut self, out: &mut [u8]) -> Result<Option<(M, usize)>> {
        if self.len == 0 { return Ok(None); }
        let slot = self.metadata[self.head];
        let meta = slot.meta.ok_or(Error::InvalidState)?;
        if out.len() < slot.len { return Err(Error::BufferFull); }
        out[..slot.len].copy_from_slice(&self.payload[slot.offset..slot.offset + slot.len]);
        self.metadata[self.head] = PacketMetadata::EMPTY;
        self.head = (self.head + 1) % self.metadata.len();
        self.len -= 1;
        self.used_bytes = self.used_bytes.saturating_sub(slot.len);
        if self.len == 0 { self.head = 0; }
        Ok(Some((meta, slot.len)))
    }

    pub fn drop_front(&mut self) -> Option<M> {
        if self.len == 0 { return None; }
        let slot = self.metadata[self.head];
        let meta = slot.meta?;
        self.metadata[self.head] = PacketMetadata::EMPTY;
        self.head = (self.head + 1) % self.metadata.len();
        self.len -= 1;
        self.used_bytes = self.used_bytes.saturating_sub(slot.len);
        if self.len == 0 { self.head = 0; }
        Some(meta)
    }

    pub fn clear(&mut self) {
        for slot in self.metadata.iter_mut() { *slot = PacketMetadata::EMPTY; }
        self.head = 0;
        self.len = 0;
        self.used_bytes = 0;
    }
}

pub type MessageBuffer<'a, M> = PacketBuffer<'a, M>;

/// One fixed-size message slot in a [`MessagePool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageSlot<M: Copy> {
    meta: Option<M>,
    len: usize,
}

impl<M: Copy> MessageSlot<M> {
    pub const EMPTY: Self = Self { meta: None, len: 0 };
    pub const fn is_used(&self) -> bool { self.meta.is_some() }
}

/// Random-access bounded message pool. The caller supplies both slot metadata
/// and the byte arena. The arena is divided evenly between slots so accepting
/// a slot always guarantees storage for one full configured message.
#[derive(Debug)]
pub struct MessagePool<'a, M: Copy> {
    slots: &'a mut [MessageSlot<M>],
    bytes: &'a mut [u8],
    slot_size: usize,
}

impl<'a, M: Copy> MessagePool<'a, M> {
    pub fn new(slots: &'a mut [MessageSlot<M>], bytes: &'a mut [u8]) -> Result<Self> {
        if slots.is_empty() || bytes.len() < slots.len() { return Err(Error::InvalidLength); }
        let slot_size = bytes.len() / slots.len();
        if slot_size == 0 { return Err(Error::InvalidLength); }
        for slot in slots.iter_mut() { *slot = MessageSlot::EMPTY; }
        Ok(Self { slots, bytes, slot_size })
    }

    pub const fn slot_size(&self) -> usize { self.slot_size }
    pub fn capacity(&self) -> usize { self.slots.len() }
    pub fn used(&self) -> usize { self.slots.iter().filter(|s| s.meta.is_some()).count() }
    pub fn free(&self) -> usize { self.capacity().saturating_sub(self.used()) }

    pub fn insert(&mut self, meta: M, data: &[u8]) -> Result<usize> {
        if data.len() > self.slot_size { return Err(Error::MessageTooLarge); }
        let idx = self.slots.iter().position(|s| s.meta.is_none()).ok_or(Error::BufferFull)?;
        let start = idx * self.slot_size;
        self.bytes[start..start + data.len()].copy_from_slice(data);
        self.slots[idx] = MessageSlot { meta: Some(meta), len: data.len() };
        Ok(idx)
    }

    pub fn get(&self, idx: usize) -> Option<(M, &[u8])> {
        let slot = *self.slots.get(idx)?;
        let meta = slot.meta?;
        let start = idx * self.slot_size;
        Some((meta, &self.bytes[start..start + slot.len]))
    }

    pub fn meta(&self, idx: usize) -> Option<M> { self.slots.get(idx)?.meta }

    pub fn set_meta(&mut self, idx: usize, meta: M) -> Result<()> {
        let slot = self.slots.get_mut(idx).ok_or(Error::InvalidField)?;
        if slot.meta.is_none() { return Err(Error::InvalidState); }
        slot.meta = Some(meta);
        Ok(())
    }

    pub fn remove_into(&mut self, idx: usize, out: &mut [u8]) -> Result<(M, usize)> {
        let slot = *self.slots.get(idx).ok_or(Error::InvalidField)?;
        let meta = slot.meta.ok_or(Error::InvalidState)?;
        if out.len() < slot.len { return Err(Error::BufferFull); }
        let start = idx * self.slot_size;
        out[..slot.len].copy_from_slice(&self.bytes[start..start + slot.len]);
        self.slots[idx] = MessageSlot::EMPTY;
        Ok((meta, slot.len))
    }

    pub fn remove(&mut self, idx: usize) -> Result<M> {
        let slot = *self.slots.get(idx).ok_or(Error::InvalidField)?;
        let meta = slot.meta.ok_or(Error::InvalidState)?;
        self.slots[idx] = MessageSlot::EMPTY;
        Ok(meta)
    }

    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() { *slot = MessageSlot::EMPTY; }
    }

    pub fn find(&self, mut predicate: impl FnMut(M) -> bool) -> Option<usize> {
        self.slots.iter().enumerate().find_map(|(i, slot)| {
            slot.meta.filter(|m| predicate(*m)).map(|_| i)
        })
    }

    pub fn for_each(&self, mut f: impl FnMut(usize, M)) {
        for (i, slot) in self.slots.iter().enumerate() {
            if let Some(meta) = slot.meta { f(i, meta); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_wraps_without_allocating() {
        let mut backing = [0u16; 3];
        let mut q = RingBuffer::new(&mut backing);
        q.push_back(1).unwrap();
        q.push_back(2).unwrap();
        assert_eq!(q.pop_front(), Some(1));
        q.push_back(3).unwrap();
        q.push_back(4).unwrap();
        assert_eq!(q.pop_front(), Some(2));
        assert_eq!(q.pop_front(), Some(3));
        assert_eq!(q.pop_front(), Some(4));
    }

    #[test]
    fn packet_buffer_wraps_payload_arena() {
        let mut meta = [PacketMetadata::<u8>::EMPTY; 3];
        let mut bytes = [0u8; 12];
        let mut q = PacketBuffer::new(&mut meta, &mut bytes);
        q.enqueue(1, b"aaaa").unwrap();
        q.enqueue(2, b"bbbb").unwrap();
        let mut out = [0u8; 8];
        assert_eq!(q.dequeue_into(&mut out).unwrap(), Some((1, 4)));
        q.enqueue(3, b"cccc").unwrap();
        assert_eq!(q.dequeue_into(&mut out).unwrap(), Some((2, 4)));
        assert_eq!(q.dequeue_into(&mut out).unwrap(), Some((3, 4)));
    }

    #[test]
    fn message_pool_is_caller_bounded() {
        let mut slots = [MessageSlot::<u8>::EMPTY; 2];
        let mut bytes = [0u8; 16];
        let mut pool = MessagePool::new(&mut slots, &mut bytes).unwrap();
        let a = pool.insert(7, b"hello").unwrap();
        assert_eq!(pool.get(a).unwrap(), (7, &b"hello"[..]));
        pool.insert(8, b"world").unwrap();
        assert_eq!(pool.insert(9, b"x"), Err(Error::BufferFull));
    }
}
