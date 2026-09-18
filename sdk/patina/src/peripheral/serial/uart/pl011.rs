//! [`UartPl011`] — a PL011 UART [`SerialIO`](crate::peripheral::serial::SerialIO) implementation.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::mmio::{
    UniqueMmioPointer, field,
    fields::{ReadPure, ReadWrite},
};
use core::ptr::NonNull;

/// PL011 flag register bit: UART busy.
const FR_BUSY: u8 = 1 << 3;
/// PL011 flag register bit: receive FIFO empty.
const FR_RXFE: u8 = 1 << 4;
/// PL011 flag register bit: transmit FIFO full.
const FR_TXFF: u8 = 1 << 5;

/// PL011 MMIO register block.
///
/// Models the Data Register (DR) at offset 0x00 and the Flag Register (FR) at offset 0x18.
/// Intermediate registers are represented as reserved padding.
#[repr(C)]
struct Pl011Registers {
    /// Data Register: reading pops from receive FIFO (side-effect), writing pushes to
    /// transmit FIFO.
    dr: ReadWrite<u8>,
    /// Reserved registers between DR (0x00) and FR (0x18).
    _reserved: [u8; 0x17],
    /// Flag Register: reading has no side-effects (pure status bits).
    fr: ReadPure<u8>,
}

/// An interface for writing to a `UartPl011` device.
#[derive(Debug)]
pub struct UartPl011 {
    /// Owned pointer to the PL011 MMIO register block.
    regs: UniqueMmioPointer<'static, Pl011Registers>,
}

impl UartPl011 {
    /// Constructs a new instance of the UART driver for a PL011 device at the
    /// given base address.
    ///
    /// # Safety
    ///
    /// The given base address must point to the MMIO control registers of a
    /// PL011 device, which must be mapped into the address space of the process
    /// as device memory and not have any other aliases for the lifetime of the
    /// returned instance.
    pub const unsafe fn new(base_address: usize) -> Self {
        // SAFETY: The caller guarantees `base_address` points to an exclusively-owned PL011
        // register block mapped as device memory, satisfying `UniqueMmioPointer::new`.
        let regs = unsafe {
            UniqueMmioPointer::new(
                NonNull::new(base_address as *mut Pl011Registers)
                    .expect("UART PL011 should have a non-null base address"),
            )
        };
        Self { regs }
    }

    /// Writes a single byte to the UART.
    pub fn write_byte(&mut self, byte: u8) {
        let mut regs = self.regs.reborrow();

        // Wait until there is room in the TX buffer.
        while field!(regs, fr).read() & FR_TXFF != 0 {}

        // Write to the TX buffer.
        field!(regs, dr).write(byte);

        // Wait until the UART is no longer busy.
        while field!(regs, fr).read() & FR_BUSY != 0 {}
    }

    /// Reads a single byte from the UART.
    pub fn read_byte(&mut self) -> Option<u8> {
        let mut regs = self.regs.reborrow();

        // Check if the RX buffer is empty.
        if field!(regs, fr).read() & FR_RXFE != 0 {
            return None;
        }

        // Read from the RX buffer.
        Some(field!(regs, dr).read())
    }
}

impl crate::peripheral::serial::SerialIO for UartPl011 {
    fn init(&mut self) {}

    fn write(&mut self, buffer: &[u8]) {
        for byte in buffer {
            self.write_byte(*byte);
        }
    }

    fn read(&mut self) -> u8 {
        loop {
            if let Some(byte) = self.read_byte() {
                return byte;
            }
        }
    }

    fn try_read(&mut self) -> Option<u8> {
        self.read_byte()
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::peripheral::serial::SerialIO;

    /// Builds a zeroed fake PL011 register block.
    fn fake_regs() -> Pl011Registers {
        Pl011Registers { dr: ReadWrite(0), _reserved: [0; 0x17], fr: ReadPure(0) }
    }

    /// Constructs a [`UartPl011`] backed by the given fake register block.
    ///
    /// The returned UART aliases `regs`; `regs` must outlive it.
    fn fake_uart(regs: &mut Pl011Registers) -> UartPl011 {
        // SAFETY: `regs` points to a valid, exclusively-owned register block that outlives
        // the returned UART.
        unsafe { UartPl011::new(core::ptr::from_mut(regs) as usize) }
    }

    #[test]
    fn test_uart_pl011_init() {
        let mut regs = fake_regs();
        let mut uart = fake_uart(&mut regs);
        uart.init();

        // Init isn't properly implemented yet, so nothing to check at the moment.
    }

    #[test]
    fn test_uart_pl011_write_byte() {
        let mut regs = fake_regs();
        let mut uart = fake_uart(&mut regs);
        uart.write_byte(b'A');
        assert_eq!(regs.dr.0, b'A');
        uart.write_byte(b'B');
        assert_eq!(regs.dr.0, b'B');
    }

    #[test]
    fn test_uart_pl011_write() {
        let mut regs = fake_regs();
        let mut uart = fake_uart(&mut regs);
        uart.write(b"abc");

        // PL011 writes bytes one at a time, so the last byte written should be 'c'.
        assert_eq!(regs.dr.0, b'c');
    }

    #[test]
    fn test_uart_pl011_read_empty() {
        let mut regs = fake_regs();
        // Set the empty flag.
        regs.fr = ReadPure(FR_RXFE);
        let mut uart = fake_uart(&mut regs);
        assert_eq!(uart.read_byte(), None);
        assert_eq!(uart.try_read(), None);
    }

    #[test]
    fn test_uart_pl011_read_byte() {
        let mut regs = fake_regs();
        regs.dr = ReadWrite(b'Z');
        let mut uart = fake_uart(&mut regs);
        assert_eq!(uart.read_byte(), Some(b'Z'));
    }

    #[test]
    fn test_uart_pl011_try_read() {
        let mut regs = fake_regs();
        regs.dr = ReadWrite(b'Z');
        let mut uart = fake_uart(&mut regs);
        assert_eq!(uart.try_read(), Some(b'Z'));
    }

    #[test]
    fn test_uart_pl011_read() {
        let mut regs = fake_regs();
        regs.dr = ReadWrite(b'Z');
        let mut uart = fake_uart(&mut regs);
        assert_eq!(uart.read(), b'Z');
    }
}
