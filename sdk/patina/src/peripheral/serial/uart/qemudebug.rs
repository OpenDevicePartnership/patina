//! QemuDebug [`SerialIO`](crate::peripheral::serial::SerialIO) implementation.
//!
//! ## License
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::arch::asm;

/// qemu debug console
#[derive(Debug)]
pub struct UartQemuDebug {
    iobase: u16,
    present: bool,
}

impl UartQemuDebug {
    // read value indicating the device is present
    const PROBE: u8 = 0xe9;

    pub const fn new(iobase: u16) -> Self {
        Self { iobase, present: false }
    }

    fn outb(&self, value: u8) {
        // SAFETY: write output byte to isa-debugcon device
        unsafe { asm!("outb %al, %dx", in("al") value, in("dx") self.iobase, options(att_syntax)) }
    }

    fn inb(&self) -> u8 {
        // SAFETY: read device detection byte from isa-debugcon device
        unsafe {
            let ret: u8;
            asm!("inb %dx, %al", in("dx") self.iobase, out("al") ret, options(att_syntax));
            ret
        }
    }
}

impl crate::peripheral::serial::SerialIO for UartQemuDebug {
    fn init(&mut self) {
        let value = self.inb();
        self.present = value == Self::PROBE;
    }

    fn write(&mut self, buffer: &[u8]) {
        if self.present {
            for c in buffer {
                self.outb(*c);
            }
        }
    }

    fn read(&mut self) -> u8 {
        // PANIC: Would loop forever, better to panic.
        panic!();
    }

    fn try_read(&mut self) -> Option<u8> {
        None
    }
}
