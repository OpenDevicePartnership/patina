//! Split virtqueue data structures.
//!
//! A split virtqueue is the shared-memory channel between the driver and the
//! device. It consists of three regions plus a pool of data buffers:
//!
//! - [`DescTable`]: array of `N` [`VirtqDesc`] entries. Each describes one
//!   buffer (physical address, length, flags). The driver fills these in.
//! - [`AvailRing`]: driver-to-device ring. The driver appends descriptor
//!   indices it wants the device to process and bumps `idx`.
//! - [`UsedRing`]: device-to-driver ring. The device appends
//!   [`VirtqUsedElem`] entries (descriptor index + bytes written) for
//!   completed requests and bumps `idx`.
//! - `buffers`: one fixed-size data buffer per descriptor slot.
//!
//! TX flow: pick a free desc slot, copy bytes into its buffer, publish via
//! `avail`, kick the device, reap completions from `used`.
//!
//! RX flow: pre-publish all descs as device-writable via `avail`; the device
//! fills buffers and posts entries on `used`; consume them, then re-publish
//! the desc to receive more.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// All array/slice indexing in this module uses `% N` (descriptor and ring slot
// modulo the queue size) or fixed-size copies bounded by `min(B, ...)`, so the
// indices are always in range by construction.
#![allow(clippy::indexing_slicing)]

use core::sync::atomic::{Ordering, fence};

// Descriptor flag bits.
const VRING_DESC_F_WRITE: u16 = crate::bit!(1);

/// A single descriptor in the descriptor table.
#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

impl VirtqDesc {
    const ZERO: Self = Self { addr: 0, len: 0, flags: 0, next: 0 };
}

/// Descriptor table. Must be 16-byte aligned per the virtio specification.
#[repr(C, align(16))]
struct DescTable<const N: usize>([VirtqDesc; N]);

impl<const N: usize> DescTable<N> {
    const fn new() -> Self {
        Self([VirtqDesc::ZERO; N])
    }
}

/// Driver (available) ring. Must be 2-byte aligned.
#[repr(C, align(2))]
struct AvailRing<const N: usize> {
    flags: u16,
    idx: u16,
    ring: [u16; N],
    used_event: u16,
}

impl<const N: usize> AvailRing<N> {
    const fn new() -> Self {
        Self { flags: 0, idx: 0, ring: [0; N], used_event: 0 }
    }
}

/// A single entry in the used ring.
#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

impl VirtqUsedElem {
    const ZERO: Self = Self { id: 0, len: 0 };
}

/// Device (used) ring. Must be 4-byte aligned.
#[repr(C, align(4))]
struct UsedRing<const N: usize> {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; N],
    avail_event: u16,
}

impl<const N: usize> UsedRing<N> {
    const fn new() -> Self {
        Self { flags: 0, idx: 0, ring: [VirtqUsedElem::ZERO; N], avail_event: 0 }
    }
}

/// A complete split virtqueue: descriptor table, available ring, used ring,
/// and a pool of fixed-size data buffers (one per descriptor slot).
///
/// `N` is the number of descriptors (must be a power of two per the virtio
/// spec) and `B` is the per-descriptor buffer size in bytes.
#[repr(C, align(16))]
pub(super) struct VirtQueue<const N: usize, const B: usize> {
    desc: DescTable<N>,
    avail: AvailRing<N>,
    used: UsedRing<N>,

    // Implementation state. Below here, the fields are not spec defined.
    //
    buffers: [[u8; B]; N],
    last_used_idx: u16,
    next_desc: u16,
    rx_held: Option<(u16, u32)>,
}

impl<const N: usize, const B: usize> VirtQueue<N, B> {
    /// Compile-time validation of the queue size.
    const VALIDATE_QUEUE: () = {
        assert!(N > 0, "VirtQueue: N must be greater than zero");
        assert!(B > 0, "VirtQueue: B must be greater than zero");
        assert!(N.is_power_of_two(), "VirtQueue: N must be a power of two");
        assert!(N <= 0x8000, "VirtQueue: N must be <= 0x8000");
    };

    /// Constructs an empty virt queue.
    pub(super) const fn new() -> Self {
        // Force evaluation of the compile-time queue-size check.
        let () = Self::VALIDATE_QUEUE;
        Self {
            desc: DescTable::new(),
            avail: AvailRing::new(),
            used: UsedRing::new(),
            buffers: [[0; B]; N],
            last_used_idx: 0,
            next_desc: 0,
            rx_held: None,
        }
    }

    /// Posts every descriptor as an RX buffer. Call once after the device
    /// has been brought to `DRIVER_OK` and before the first receive. The
    /// caller must notify the device afterwards.
    pub(super) fn rx_clean(&mut self) {
        for i in 0..N {
            self.rx_post(i as u16);
        }
    }

    /// Returns the next RX block ready to be read, or `None` if the device
    /// has not posted any data. Call [`Self::rx_release`] to return the
    /// descriptor to the device once done.
    pub(super) fn rx_peek(&mut self) -> Option<&[u8]> {
        if self.rx_held.is_none() {
            loop {
                let (desc, len) = self.rx_pop()?;
                if len == 0 {
                    // Spurious empty completion: re-arm and try again.
                    self.rx_post(desc);
                    continue;
                }
                let len = core::cmp::min(len as usize, B) as u32;
                self.rx_held = Some((desc, len));
                break;
            }
        }
        let (desc, len) = self.rx_held?;
        let idx = (desc as usize) % N;
        Some(&self.buffers[idx][..len as usize])
    }

    /// Releases the currently held RX block back to the device. The caller
    /// must notify the device afterwards. No-op if no block is held.
    pub(super) fn rx_release(&mut self) {
        if let Some((desc, _)) = self.rx_held.take() {
            self.rx_post(desc);
        }
    }

    /// Posts a single RX descriptor: the device will fill its buffer with up
    /// to `B` received bytes.
    fn rx_post(&mut self, desc_idx: u16) {
        let idx = (desc_idx as usize) % N;
        let addr = self.buffers[idx].as_ptr() as u64;
        self.desc.0[idx] = VirtqDesc { addr, len: B as u32, flags: VRING_DESC_F_WRITE, next: 0 };
        self.publish_avail(desc_idx);
    }

    /// Pops the next entry from the used ring, or returns `None` if no
    /// completions are pending. Returns `(descriptor index, bytes filled)`.
    fn rx_pop(&mut self) -> Option<(u16, u32)> {
        fence(Ordering::SeqCst);
        if self.last_used_idx == self.used.idx {
            return None;
        }
        let elem_pos = (self.last_used_idx as usize) % N;
        let elem = self.used.ring[elem_pos];
        self.last_used_idx = self.last_used_idx.wrapping_add(1);
        Some((elem.id as u16, elem.len))
    }

    /// Tries to publish up to `B` bytes from `data` on the next round-robin
    /// TX descriptor. Returns the number of bytes consumed, or `None` if all
    /// slots are still owned by the device.
    pub(super) fn tx_try_submit(&mut self, data: &[u8]) -> Option<usize> {
        if data.is_empty() {
            return Some(0);
        }

        let desc_idx = (self.next_desc as usize) % N;
        if self.desc.0[desc_idx].len != 0 {
            // Slot looks busy: drain any completions the device has posted
            // and retry.
            self.tx_reap();
            if self.desc.0[desc_idx].len != 0 {
                return None;
            }
        }

        let take = core::cmp::min(B, data.len());
        self.buffers[desc_idx][..take].copy_from_slice(&data[..take]);
        let buf_addr = self.buffers[desc_idx].as_ptr() as u64;
        // Driver-to-device data, no NEXT.
        self.desc.0[desc_idx] = VirtqDesc { addr: buf_addr, len: take as u32, flags: 0, next: 0 };

        self.publish_avail(desc_idx as u16);
        self.next_desc = self.next_desc.wrapping_add(1);
        Some(take)
    }

    /// Reaps any TX descriptors the device has returned via the used ring.
    fn tx_reap(&mut self) {
        fence(Ordering::SeqCst);
        let used_idx = self.used.idx;
        while self.last_used_idx != used_idx {
            let elem_pos = (self.last_used_idx as usize) % N;
            let elem = self.used.ring[elem_pos];
            let d = (elem.id as usize) % N;
            self.desc.0[d].len = 0;
            self.last_used_idx = self.last_used_idx.wrapping_add(1);
        }
    }

    /// Physical address of the descriptor table. For use by the transport
    /// when programming the device.
    pub(super) fn desc_addr(&self) -> u64 {
        self.desc.0.as_ptr() as u64
    }

    /// Physical address of the available (driver) ring.
    pub(super) fn avail_addr(&self) -> u64 {
        core::ptr::from_ref(&self.avail) as u64
    }

    /// Physical address of the used (device) ring.
    pub(super) fn used_addr(&self) -> u64 {
        core::ptr::from_ref(&self.used) as u64
    }

    /// Publishes a descriptor index on the available ring with full
    /// `SeqCst` ordering on either side of the index update.
    fn publish_avail(&mut self, desc_idx: u16) {
        let avail_idx = self.avail.idx;
        let ring_pos = (avail_idx as usize) % N;
        self.avail.ring[ring_pos] = desc_idx;
        fence(Ordering::SeqCst);
        self.avail.idx = avail_idx.wrapping_add(1);
        fence(Ordering::SeqCst);
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
impl<const N: usize, const B: usize> VirtQueue<N, B> {
    pub(super) fn test_drain_tx(&mut self) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
        use alloc::vec::Vec;
        fence(Ordering::SeqCst);
        let mut out = Vec::new();
        while self.used.idx != self.avail.idx {
            let pos = (self.used.idx as usize) % N;
            let desc_idx = self.avail.ring[pos];
            let i = (desc_idx as usize) % N;
            let len = self.desc.0[i].len as usize;
            out.push(self.buffers[i][..len].to_vec());
            self.used.ring[pos] = VirtqUsedElem { id: u32::from(desc_idx), len: len as u32 };
            fence(Ordering::SeqCst);
            self.used.idx = self.used.idx.wrapping_add(1);
        }
        fence(Ordering::SeqCst);
        out
    }

    pub(super) fn test_supply_rx(&mut self, data: &[u8]) -> Option<usize> {
        fence(Ordering::SeqCst);
        if self.used.idx == self.avail.idx {
            return None;
        }
        let pos = (self.used.idx as usize) % N;
        let desc_idx = self.avail.ring[pos];
        let i = (desc_idx as usize) % N;
        let take = core::cmp::min(B, data.len());
        self.buffers[i][..take].copy_from_slice(&data[..take]);
        self.used.ring[pos] = VirtqUsedElem { id: u32::from(desc_idx), len: take as u32 };
        fence(Ordering::SeqCst);
        self.used.idx = self.used.idx.wrapping_add(1);
        fence(Ordering::SeqCst);
        Some(take)
    }
}
