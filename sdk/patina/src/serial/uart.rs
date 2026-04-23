//! [SerialIO](crate::serial::SerialIO) UART implementations.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// A null (stub) device that does nothing.
#[derive(Debug)]
pub struct UartNull {}

impl super::SerialIO for UartNull {
    fn init(&self) {}

    fn write(&self, _buffer: &[u8]) {}

    fn read(&self) -> u8 {
        // PANIC: Would loop forever, better to panic.
        panic!();
    }

    fn try_read(&self) -> Option<u8> {
        None
    }
}

cfg_if::cfg_if! {
    if #[cfg(all(target_arch = "x86_64", any(target_os = "uefi", feature = "doc")))] {

        use uart_16550::MmioSerialPort;
        use uart_16550::SerialPort as IoSerialPort;

        /// Runs `f` with CPU interrupts disabled, restoring the previous interrupt state afterward.
        fn without_interrupts<F, R>(f: F) -> R
        where
            F: FnOnce() -> R,
        {
            let flags: u64;
            // SAFETY: Reading RFLAGS and disabling interrupts around the closure is safe because we restore
            // interrupts afterwards, but only if they were previously enabled.
            unsafe {
                core::arch::asm!("pushfq; pop {0}; cli", out(reg) flags, options(nomem));
            }
            let result = f();
            // Restore interrupts only if they were previously enabled (IF bit).
            if flags & (1 << 9) != 0 {
                // SAFETY: Re-enabling interrupts that were enabled before.
                unsafe { core::arch::asm!("sti", options(nostack, nomem)); }
            }
            result
        }

        /// An interface for writing to a Uart16550 device.
        #[derive(Debug)]
        pub enum Uart16550 {
            /// The I/O interface for the Uart16550 serial port.
            Io {
                /// The base address of the UART control registers.
                base: u16
            },
            /// The Memory Mapped I/O interface for the Uart16550 serial port.
            Mmio {
                /// The base address of the UART control registers.
                base: usize,
                /// The number of bytes between consecutive registers.
                reg_stride: usize
            },
        }

        impl super::SerialIO for Uart16550 {
            fn init(&self) {
                match self {
                    Uart16550::Io { base } => {
                        // SAFETY: The base address is provided during Uart16550 construction and is assumed to be valid for I/O port access.
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        serial_port.init();
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        // SAFETY: The base address and stride are provided during Uart16550 construction and are assumed to be valid for MMIO access.
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        serial_port.init();
                    }
                }
            }

            fn write(&self, buffer: &[u8]) {
                match self {
                    Uart16550::Io { base } => {
                        // SAFETY: The base address is provided during Uart16550 construction and is assumed to be valid for I/O port access.
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        without_interrupts(|| {
                            for b in buffer {
                                serial_port.send(*b);
                            }
                        });
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        // SAFETY: The base address and stride are provided during Uart16550 construction and are assumed to be valid for MMIO access.
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        without_interrupts(|| {
                            for b in buffer {
                                serial_port.send(*b);
                            }
                        });
                    }
                }
            }

            fn read(&self) -> u8 {
                match self {
                    Uart16550::Io { base } => {
                        // SAFETY: The base address is provided during Uart16550 construction and is assumed to be valid for I/O port access.
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        serial_port.receive()
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        // SAFETY: The base address and stride are provided during Uart16550 construction and are assumed to be valid for MMIO access.
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        serial_port.receive()
                    }
                }
            }

            fn try_read(&self) -> Option<u8> {
                match self {
                    Uart16550::Io { base } => {
                        // SAFETY: The base address is provided during Uart16550 construction and is assumed to be valid for I/O port access.
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        serial_port.try_receive().ok()
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        // SAFETY: The base address and stride are provided during Uart16550 construction and are assumed to be valid for MMIO access.
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        serial_port.try_receive().ok()
                    }
                }
            }

        }
    }
}

cfg_if::cfg_if! {
    if #[cfg(any(feature = "doc", all(target_os = "uefi", target_arch = "aarch64")))] {
        use core::ptr::NonNull;
        use crate::mmio::{field, fields::{ReadPure, ReadWrite}, UniqueMmioPointer};

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

        /// An interface for writing to a UartPl011 device.
        #[derive(Debug)]
        pub struct UartPl011 {
            /// The base address of the UART control registers.
            base_address: usize,
        }

        impl UartPl011 {
            /// Constructs a new instance of the UART driver for a PL011 device at the
            /// given base address.
            ///
            /// # Safety
            ///
            /// The given base address must point to the MMIO control registers of a
            /// PL011 device, which must be mapped into the address space of the process
            /// as device memory and not have any other aliases.
            pub const fn new(base_address: usize) -> Self {
                Self { base_address }
            }

            /// Returns a [`UniqueMmioPointer`] to the PL011 register block.
            ///
            /// # Safety
            ///
            /// The caller must ensure that no other `UniqueMmioPointer` to the same
            /// MMIO region exists for the duration of the returned pointer's use.
            unsafe fn registers(&self) -> UniqueMmioPointer<'_, Pl011Registers> {
                // SAFETY: The base address is required by the safety contract of new() to point
                // to a PL011 register block that is mapped as device memory.
                unsafe {
                    UniqueMmioPointer::new(NonNull::new(self.base_address as *mut Pl011Registers).unwrap())
                }
            }

            /// Writes a single byte to the UART.
            pub fn write_byte(&self, byte: u8) {
                // SAFETY: Exclusive MMIO access is given by calling `UartPl011::new`.
                let mut regs = unsafe { self.registers() };

                // Wait until there is room in the TX buffer.
                while field!(regs, fr).read() & FR_TXFF != 0 {}

                // Write to the TX buffer.
                field!(regs, dr).write(byte);

                // Wait until the UART is no longer busy.
                while field!(regs, fr).read() & FR_BUSY != 0 {}
            }

            /// Reads a single byte from the UART.
            pub fn read_byte(&self) -> Option<u8> {
                // SAFETY: Exclusive MMIO access is given by calling `UartPl011::new`.
                let mut regs = unsafe { self.registers() };

                // Check if the RX buffer is empty.
                if field!(regs, fr).read() & FR_RXFE != 0 {
                    return None;
                }

                // Read from the RX buffer.
                Some(field!(regs, dr).read())
            }
        }

        impl super::SerialIO for UartPl011 {
            fn init(&self) {}

            fn write(&self, buffer: &[u8]) {
                for byte in buffer {
                    self.write_byte(*byte);
                }
            }

            fn read(&self) -> u8 {
                loop {
                    if let Some(byte) = self.read_byte() {
                        return byte;
                    }
                }
            }

            fn try_read(&self) -> Option<u8> {
                self.read_byte()
            }
        }
    }
}
