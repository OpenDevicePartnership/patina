//! virtio-MMIO (version 2 / virtio 1.x) transport.
//!
//! Provides the device-agnostic register-block handshake, queue programming,
//! and notification primitives shared by any virtio device riding on the
//! virtio-MMIO transport.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    ptr::NonNull,
    sync::atomic::{Ordering, fence},
};

use crate::mmio::{
    UniqueMmioPointer, field,
    fields::{ReadPure, ReadPureWrite, WriteOnly},
};

use super::queue::VirtQueue;

/// Magic value read from offset 0.
const VIRTIO_MMIO_MAGIC: u32 = crate::signature!('v', 'i', 'r', 't');
/// Modern virtio-MMIO transport version.
const VIRTIO_MMIO_VERSION: u32 = 2;

// Bits in the status register.
const STATUS_ACKNOWLEDGE: u32 = crate::bit!(0);
const STATUS_DRIVER: u32 = crate::bit!(1);
const STATUS_DRIVER_OK: u32 = crate::bit!(2);
const STATUS_FEATURES_OK: u32 = crate::bit!(3);
const STATUS_FAILED: u32 = crate::bit!(7);

/// `VIRTIO_F_VERSION_1` lives at bit 32 in the device-features bitmap. It is
/// written into the high (selector = 1) 32-bit word as bit 0. Acknowledging
/// this feature is required for a modern (1.x) virtio driver.
const VIRTIO_F_VERSION_1_HIGH_WORD: u32 = crate::bit!(0);

/// Virtio-MMIO register layout. Reads are side-effect free for every register
/// here; only writes affect device state.
#[repr(C)]
pub(super) struct VirtioMmioRegs {
    /// 0x000 Magic value, must equal [`VIRTIO_MMIO_MAGIC`].
    magic_value: ReadPure<u32>,
    /// 0x004 Transport version.
    version: ReadPure<u32>,
    /// 0x008 Device id.
    device_id: ReadPure<u32>,
    /// 0x00C Vendor id.
    vendor_id: ReadPure<u32>,
    /// 0x010 Device features.
    device_features: ReadPure<u32>,
    /// 0x014 Device features selector.
    device_features_sel: WriteOnly<u32>,
    _reserved0: [u32; 2],
    /// 0x020 Driver features.
    driver_features: WriteOnly<u32>,
    /// 0x024 Driver features selector.
    driver_features_sel: WriteOnly<u32>,
    _reserved1: [u32; 2],
    /// 0x030 Queue selector.
    queue_sel: WriteOnly<u32>,
    /// 0x034 Maximum supported queue size for the selected queue.
    queue_num_max: ReadPure<u32>,
    /// 0x038 Queue size to use for the selected queue.
    queue_num: WriteOnly<u32>,
    _reserved2: [u32; 2],
    /// 0x044 Queue ready flag for the selected queue.
    queue_ready: ReadPureWrite<u32>,
    _reserved3: [u32; 2],
    /// 0x050 Queue notify: write the queue index to kick the device.
    queue_notify: WriteOnly<u32>,
    _reserved4: [u32; 3],
    /// 0x060 Interrupt status.
    interrupt_status: ReadPure<u32>,
    /// 0x064 Interrupt acknowledge.
    interrupt_ack: WriteOnly<u32>,
    _reserved5: [u32; 2],
    /// 0x070 Device status.
    status: ReadPureWrite<u32>,
    _reserved6: [u32; 3],
    /// 0x080 Descriptor table physical address (low 32 bits).
    queue_desc_low: WriteOnly<u32>,
    /// 0x084 Descriptor table physical address (high 32 bits).
    queue_desc_high: WriteOnly<u32>,
    _reserved7: [u32; 2],
    /// 0x090 Driver ring physical address (low 32 bits).
    queue_driver_low: WriteOnly<u32>,
    /// 0x094 Driver ring physical address (high 32 bits).
    queue_driver_high: WriteOnly<u32>,
    _reserved8: [u32; 2],
    /// 0x0A0 Device ring physical address (low 32 bits).
    queue_device_low: WriteOnly<u32>,
    /// 0x0A4 Device ring physical address (high 32 bits).
    queue_device_high: WriteOnly<u32>,
}

/// Errors returned by the virtio-MMIO init / queue-configuration routines.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum InitError {
    /// Magic value, transport version, or device id did not match expectations.
    UnsupportedDevice,
    /// The device rejected the negotiated feature bits.
    FeaturesRejected,
    /// The device's `queue_num_max` was below the requested queue size, or
    /// the queue was already in use by another driver.
    QueueUnavailable,
}

/// A virtio-MMIO transport at a fixed base address.
#[derive(Debug)]
pub(super) struct VirtioMmio {
    regs: UniqueMmioPointer<'static, VirtioMmioRegs>,
}

// SAFETY: `VirtioMmio` owns a unique pointer to a device-memory MMIO window
// All device-touching methods take `&mut self`, so there is no concurrent
// access through the same instance.
unsafe impl Send for VirtioMmio {}

impl VirtioMmio {
    /// Constructs a new transport for the virtio-MMIO device at the given
    /// base address.
    ///
    /// # Safety
    ///
    /// `base_address` must be a non-null address of an appropriately mapped
    /// virtio-MMIO register window which is present and exclusively owned for
    /// the lifetime of the returned structure.
    pub(super) const unsafe fn new(base_address: usize) -> Self {
        let ptr = NonNull::new(base_address as *mut VirtioMmioRegs).expect("Caller provided NULL address.");

        // SAFETY: forwarded from the caller of this function. They are responsible for ensuring the address
        // is exclusively owned.
        Self { regs: unsafe { UniqueMmioPointer::new(ptr) } }
    }

    /// Initializes the virtio-MMIO device using the standard init handshake.
    pub(super) fn begin_init(&mut self, expected_device_id: u32) -> Result<(), InitError> {
        let mut regs = self.regs.reborrow();

        if field!(regs, magic_value).read() != VIRTIO_MMIO_MAGIC
            || field!(regs, version).read() != VIRTIO_MMIO_VERSION
            || field!(regs, device_id).read() != expected_device_id
        {
            return Err(InitError::UnsupportedDevice);
        }

        // Reset, then wait for `status` to read back as 0 before continuing.
        field!(regs, status).write(0);
        while field!(regs, status).read() != 0 {
            core::hint::spin_loop();
        }
        field!(regs, status).write(STATUS_ACKNOWLEDGE);
        field!(regs, status).write(STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        // Read device features before writing the subset we accept. We
        // require VIRTIO_F_VERSION_1 (bit 32, bit 0 of the high word).
        field!(regs, device_features_sel).write(1);
        let device_features_high = field!(regs, device_features).read();
        if device_features_high & VIRTIO_F_VERSION_1_HIGH_WORD == 0 {
            field!(regs, status).write(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FAILED);
            return Err(InitError::FeaturesRejected);
        }

        // Acknowledge VIRTIO_F_VERSION_1 and nothing else.
        field!(regs, driver_features_sel).write(0);
        field!(regs, driver_features).write(0);
        field!(regs, driver_features_sel).write(1);
        field!(regs, driver_features).write(VIRTIO_F_VERSION_1_HIGH_WORD);

        field!(regs, status).write(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        if field!(regs, status).read() & STATUS_FEATURES_OK == 0 {
            field!(regs, status).write(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FAILED);
            return Err(InitError::FeaturesRejected);
        }

        Ok(())
    }

    /// Programs the device with the addresses of the three rings for a
    /// virt queue and marks it ready.
    pub(super) fn configure_queue<const N: usize, const B: usize>(
        &mut self,
        index: u32,
        queue: &VirtQueue<N, B>,
    ) -> Result<(), InitError> {
        let mut regs = self.regs.reborrow();
        field!(regs, queue_sel).write(index);
        if field!(regs, queue_ready).read() != 0 {
            // Already initialized by something else.
            return Err(InitError::QueueUnavailable);
        }
        let max = field!(regs, queue_num_max).read();
        if max < N as u32 {
            return Err(InitError::QueueUnavailable);
        }
        field!(regs, queue_num).write(N as u32);

        let desc_pa = queue.desc_addr();
        let avail_pa = queue.avail_addr();
        let used_pa = queue.used_addr();

        field!(regs, queue_desc_low).write(desc_pa as u32);
        field!(regs, queue_desc_high).write((desc_pa >> 32) as u32);
        field!(regs, queue_driver_low).write(avail_pa as u32);
        field!(regs, queue_driver_high).write((avail_pa >> 32) as u32);
        field!(regs, queue_device_low).write(used_pa as u32);
        field!(regs, queue_device_high).write((used_pa >> 32) as u32);

        field!(regs, queue_ready).write(1);
        Ok(())
    }

    /// Sets `STATUS_DRIVER_OK`, completing the init handshake.
    pub(super) fn set_driver_ok(&mut self) {
        let mut regs = self.regs.reborrow();
        field!(regs, status).write(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
    }

    /// Sets `STATUS_FAILED`.
    pub(super) fn set_failed(&mut self) {
        let mut regs = self.regs.reborrow();
        let current = field!(regs, status).read();
        field!(regs, status).write(current | STATUS_FAILED);
    }

    /// Notifies the device that new entries are available on `queue_index`.
    /// Issues a `SeqCst` fence before the write so all prior queue updates are
    /// visible to the device.
    pub(super) fn notify(&mut self, queue_index: u32) {
        fence(Ordering::SeqCst);
        let mut regs = self.regs.reborrow();
        field!(regs, queue_notify).write(queue_index);
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
impl VirtioMmioRegs {
    pub(super) fn new_fake(device_id: u32) -> Self {
        // SAFETY: This is a fake register set, the state will be initialized later.
        let mut regs: Self = unsafe { core::mem::zeroed() };
        let mut ptr = UniqueMmioPointer::from(&mut regs);
        // SAFETY: This is a fake register set, the MMIO semantics can be ignored.
        unsafe {
            field!(ptr, magic_value).write_unsafe(ReadPure(VIRTIO_MMIO_MAGIC));
            field!(ptr, version).write_unsafe(ReadPure(VIRTIO_MMIO_VERSION));
            field!(ptr, device_id).write_unsafe(ReadPure(device_id));
            field!(ptr, device_features).write_unsafe(ReadPure(VIRTIO_F_VERSION_1_HIGH_WORD));
            field!(ptr, queue_num_max).write_unsafe(ReadPure(8));
        }
        regs
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    const TEST_DEVICE_ID: u32 = 3;

    fn make_transport(regs: &mut VirtioMmioRegs) -> VirtioMmio {
        let addr = core::ptr::from_mut(regs) as usize;
        // SAFETY: `addr` is non-null and `regs` outlives the returned transport.
        unsafe { VirtioMmio::new(addr) }
    }

    #[test]
    fn test_begin_init() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        let mut t = make_transport(&mut regs);
        assert_eq!(t.begin_init(TEST_DEVICE_ID), Ok(()));
        assert_eq!(regs.status.0, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        assert_eq!(regs.driver_features_sel.0, 1);
        assert_eq!(regs.driver_features.0, VIRTIO_F_VERSION_1_HIGH_WORD);
    }

    #[test]
    fn test_rejects_bad_magic() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        regs.magic_value = ReadPure(0xDEADBEEF);
        let mut t = make_transport(&mut regs);
        assert_eq!(t.begin_init(TEST_DEVICE_ID), Err(InitError::UnsupportedDevice));
    }

    #[test]
    fn test_rejects_bad_version() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        regs.version = ReadPure(1);
        let mut t = make_transport(&mut regs);
        assert_eq!(t.begin_init(TEST_DEVICE_ID), Err(InitError::UnsupportedDevice));
    }

    #[test]
    fn test_rejects_wrong_device_id() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        let mut t = make_transport(&mut regs);
        assert_eq!(t.begin_init(TEST_DEVICE_ID + 1), Err(InitError::UnsupportedDevice));
    }

    #[test]
    fn test_rejects_when_v1_unsupported() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        regs.device_features = ReadPure(0);
        let mut t = make_transport(&mut regs);
        assert_eq!(t.begin_init(TEST_DEVICE_ID), Err(InitError::FeaturesRejected));
        assert_ne!(regs.status.0 & STATUS_FAILED, 0);
    }

    #[test]
    fn test_configure_queue() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        let queue: VirtQueue<8, 16> = VirtQueue::new();
        let mut t = make_transport(&mut regs);
        assert_eq!(t.configure_queue(1, &queue), Ok(()));
        assert_eq!(regs.queue_sel.0, 1);
        assert_eq!(regs.queue_num.0, 8);
        assert_eq!(regs.queue_ready.0, 1);
        let desc = u64::from(regs.queue_desc_low.0) | (u64::from(regs.queue_desc_high.0) << 32);
        let avail = u64::from(regs.queue_driver_low.0) | (u64::from(regs.queue_driver_high.0) << 32);
        let used = u64::from(regs.queue_device_low.0) | (u64::from(regs.queue_device_high.0) << 32);
        assert_eq!(desc, queue.desc_addr());
        assert_eq!(avail, queue.avail_addr());
        assert_eq!(used, queue.used_addr());
    }

    #[test]
    fn test_configure_queue_rejects_too_small() {
        // Fake reports queue_num_max = 8, but we ask for N = 16.
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        let queue: VirtQueue<16, 16> = VirtQueue::new();
        let mut t = make_transport(&mut regs);
        assert_eq!(t.configure_queue(0, &queue), Err(InitError::QueueUnavailable));
    }

    #[test]
    fn test_configure_queue_rejects_when_already_in_use() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        regs.queue_ready = ReadPureWrite(1);
        let queue: VirtQueue<8, 16> = VirtQueue::new();
        let mut t = make_transport(&mut regs);
        assert_eq!(t.configure_queue(0, &queue), Err(InitError::QueueUnavailable));
    }

    #[test]
    fn test_virtio_set_driver_ok() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        let mut t = make_transport(&mut regs);
        t.set_driver_ok();
        assert_eq!(regs.status.0, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
    }

    #[test]
    fn test_set_failed() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        regs.status = ReadPureWrite(STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        let mut t = make_transport(&mut regs);
        t.set_failed();
        assert_eq!(regs.status.0, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FAILED);
    }

    #[test]
    fn test_notify_writes_queue_index() {
        let mut regs = VirtioMmioRegs::new_fake(TEST_DEVICE_ID);
        let mut t = make_transport(&mut regs);
        t.notify(7);
        assert_eq!(regs.queue_notify.0, 7);
    }
}
