//! [`SerialIO`](crate::peripheral::serial::SerialIO) implementation for a virtio-console device on the virtio-MMIO transport.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

// The internal virtio infrastructure is being left private for now. It's written to be re-usable,
// but until there is a known use-case, don't expose a public API for it.
mod mmio;
mod queue;

use mmio::VirtioMmio;
use queue::VirtQueue;

/// Virtio device id for a console device.
const VIRTIO_ID_CONSOLE: u32 = 3;

/// Receive virt queue index.
const RX_QUEUE: u32 = 0;
/// Transmit virt queue index.
const TX_QUEUE: u32 = 1;

/// A [`SerialIO`](crate::peripheral::serial::SerialIO) implementation for a virtio-console
/// device attached via the virtio-MMIO transport.
pub struct VirtioSerial<const N: usize = 8, const B: usize = 128> {
    initialized: bool,
    transport: VirtioMmio,
    rx: VirtQueue<N, B>,
    tx: VirtQueue<N, B>,
    rx_block_offset: usize,
}

impl<const N: usize, const B: usize> VirtioSerial<N, B> {
    /// Constructs a new instance for a virtio-MMIO device at the given base
    /// address. [`SerialIO::init`](crate::peripheral::serial::SerialIO::init) must be
    /// called before any I/O.
    ///
    /// # Safety
    ///
    /// `base_address` must point to an appropriately mapped virtio-MMIO register
    /// window for a virtio-console. This address must be exclusively used by
    /// this instance.
    pub const unsafe fn new(base_address: usize) -> Self {
        Self {
            initialized: false,
            // SAFETY: forwarded from caller.
            transport: unsafe { VirtioMmio::new(base_address) },
            rx: VirtQueue::new(),
            tx: VirtQueue::new(),
            rx_block_offset: 0,
        }
    }
}

impl<const N: usize, const B: usize> crate::peripheral::serial::SerialIO for VirtioSerial<N, B> {
    fn init(&mut self) {
        if self.initialized {
            return;
        }

        if self.transport.begin_init(VIRTIO_ID_CONSOLE).is_err() {
            return;
        }
        if self.transport.configure_queue(RX_QUEUE, &self.rx).is_err()
            || self.transport.configure_queue(TX_QUEUE, &self.tx).is_err()
        {
            self.transport.set_failed();
            return;
        }
        self.transport.set_driver_ok();
        self.rx.rx_clean();
        self.transport.notify(RX_QUEUE);
        self.initialized = true;
    }

    fn write(&mut self, buffer: &[u8]) {
        if !self.initialized {
            return;
        }

        let mut offset = 0;
        while offset < buffer.len() {
            // Spin until a block is available in the queue.
            let take = loop {
                let remaining = buffer.get(offset..).expect("offset < buffer.len()");
                if let Some(n) = self.tx.tx_try_submit(remaining) {
                    break n;
                }
                core::hint::spin_loop();
            };
            offset += take;
            self.transport.notify(TX_QUEUE);
        }
    }

    fn read(&mut self) -> u8 {
        loop {
            if let Some(b) = self.try_read() {
                return b;
            }
            core::hint::spin_loop();
        }
    }

    fn try_read(&mut self) -> Option<u8> {
        if !self.initialized {
            return None;
        }

        // Get the next byte of the next RX block.
        let data = self.rx.rx_peek()?;
        let byte = *data.get(self.rx_block_offset).expect("rx_block_offset < data.len()");
        self.rx_block_offset += 1;

        // Release this block if all its data is consumed.
        if self.rx_block_offset >= data.len() {
            self.rx_block_offset = 0;
            self.rx.rx_release();
            self.transport.notify(RX_QUEUE);
        }
        Some(byte)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use crate::peripheral::serial::SerialIO;
    use mmio::VirtioMmioRegs;
    use std::vec::Vec;

    fn test_instance<const N: usize, const B: usize>(regs: &VirtioMmioRegs) -> VirtioSerial<N, B> {
        // SAFETY: `regs` outlives the returned `VirtioSerial`.
        let mut serial = unsafe { VirtioSerial::<N, B>::new(core::ptr::from_ref(regs) as usize) };

        // Skip device-handshake initialization.
        serial.rx.rx_clean();
        serial.initialized = true;

        serial
    }

    #[test]
    fn test_virtio_init_checks() {
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<4, 16> = test_instance(&regs);

        // The test instance is already initialized, so this tests the re-init path.
        serial.init();

        // Now forcibly uninit, and make sure that all calls fail gracefully.
        serial.initialized = false;
        serial.write(b"a");
        assert_eq!(serial.try_read(), None);
    }

    #[test]
    fn test_virtio_serial_write_single_descriptor() {
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<4, 16> = test_instance(&regs);
        serial.write(b"hello");
        let chunks = serial.tx.test_drain_tx();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], b"hello");
    }

    #[test]
    fn test_virtio_serial_write_spans_multiple_descriptors() {
        // B = 4 forces chunking.
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<4, 4> = test_instance(&regs);
        serial.write(b"abcdefghij");
        let chunks = serial.tx.test_drain_tx();
        let combined: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(combined, b"abcdefghij");
    }

    #[test]
    fn test_virtio_serial_write_reuses_slots_after_drain() {
        // Two 8-byte writes through an 8-byte (N * B) queue.
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<2, 4> = test_instance(&regs);
        serial.write(b"abcdefgh");
        let first = serial.tx.test_drain_tx();
        let first_combined: Vec<u8> = first.into_iter().flatten().collect();
        assert_eq!(first_combined, b"abcdefgh");

        serial.write(b"ABCDEFGH");
        let second = serial.tx.test_drain_tx();
        let second_combined: Vec<u8> = second.into_iter().flatten().collect();
        assert_eq!(second_combined, b"ABCDEFGH");
    }

    #[test]
    fn test_virtio_serial_try_read_empty() {
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<4, 16> = test_instance(&regs);
        assert_eq!(serial.try_read(), None);
    }

    #[test]
    fn test_virtio_serial_read_single_block() {
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<4, 16> = test_instance(&regs);
        let supplied = serial.rx.test_supply_rx(b"world");
        assert_eq!(supplied, Some(5));
        let mut got = Vec::new();
        for _ in 0..5 {
            got.push(serial.read());
        }
        assert_eq!(got, b"world");
        assert_eq!(serial.try_read(), None);
    }

    #[test]
    fn test_virtio_serial_read_spans_multiple_blocks() {
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<4, 4> = test_instance(&regs);
        assert_eq!(serial.rx.test_supply_rx(b"foo"), Some(3));
        assert_eq!(serial.rx.test_supply_rx(b"bar"), Some(3));
        let mut got = Vec::new();
        for _ in 0..6 {
            got.push(serial.read());
        }
        assert_eq!(got, b"foobar");
    }

    #[test]
    fn test_virtio_serial_tx_reap_reclaims_completed_slots() {
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<2, 4> = test_instance(&regs);

        // Fill both TX descriptors, then drain to mark them used.
        serial.write(b"00001111");
        let drained = serial.tx.test_drain_tx();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], b"0000");
        assert_eq!(drained[1], b"1111");

        // This write would spin forever if `tx_reap` didn't reclaim slots.
        serial.write(b"2222");
        let after = serial.tx.test_drain_tx();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0], b"2222");
    }

    #[test]
    fn test_virtio_serial_read_truncates_to_buffer_size() {
        // 5-byte payload truncated to B = 4.
        let regs = VirtioMmioRegs::new_fake(VIRTIO_ID_CONSOLE);
        let mut serial: VirtioSerial<4, 4> = test_instance(&regs);
        assert_eq!(serial.rx.test_supply_rx(b"abcde"), Some(4));
        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(serial.read());
        }
        assert_eq!(got, b"abcd");
    }
}
