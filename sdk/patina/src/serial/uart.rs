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

        use core::ptr::{NonNull as PtrNonNull, with_exposed_provenance_mut};
        use spin::Mutex;
        use uart_16550::backend::{MmioBackend, PioBackend};
        use uart_16550::{Config, Uart16550 as Uart16550Device};

        /// Returns the Current Privilege Level (CPL) from the CS selector.
        fn current_privilege_level() -> u16 {
            let cs: u16;
            // SAFETY: Reading the CS register has no side effects.
            unsafe { core::arch::asm!("mov {0:x}, cs", out(reg) cs, options(nostack, nomem)); }
            cs & 0b11
        }

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
            Io(Mutex<Uart16550Device<PioBackend>>),
            /// The Memory Mapped I/O interface for the Uart16550 serial port.
            Mmio(Mutex<Uart16550Device<MmioBackend>>),
        }

        impl Uart16550 {
            /// Constructs a UART backed by x86 port I/O.
            ///
            /// # Safety
            ///
            /// The base port must be valid and safe to access for the lifetime
            /// of the returned wrapper.
            pub unsafe fn new_port(base: u16) -> Self {
                // SAFETY: The caller guarantees that `base` is valid for UART port I/O.
                let uart = unsafe { Uart16550Device::new_port(base) }
                    .expect("UART 16550 I/O base address must allow access to all registers");
                Self::Io(Mutex::new(uart))
            }

            /// Constructs a UART backed by memory-mapped I/O.
            ///
            /// # Safety
            ///
            /// The base address must be valid and safe to access for the
            /// lifetime of the returned wrapper.
            pub unsafe fn new_mmio(base: usize, reg_stride: u8) -> Self {
                let base = PtrNonNull::new(with_exposed_provenance_mut::<u8>(base))
                    .expect("UART 16550 MMIO base address must be non-null");
                // SAFETY: The caller guarantees that `base` is valid for UART MMIO access.
                let uart = unsafe { Uart16550Device::new_mmio(base, reg_stride) }
                    .expect("UART 16550 MMIO base address and register stride must be valid");
                Self::Mmio(Mutex::new(uart))
            }
        }

        impl super::SerialIO for Uart16550 {
            fn init(&self) {
                let init = || match self {
                    Uart16550::Io(uart) => {
                        let mut uart = uart
                            .try_lock()
                            .expect("UART 16550 I/O device lock must not be re-entered");
                        uart.init(Config::default())
                            .expect("UART 16550 I/O device initialization must succeed");
                    }
                    Uart16550::Mmio(uart) => {
                        let mut uart = uart
                            .try_lock()
                            .expect("UART 16550 MMIO device lock must not be re-entered");
                        uart.init(Config::default())
                            .expect("UART 16550 MMIO device initialization must succeed");
                    }
                };

                if current_privilege_level() == 0 {
                    // CPL is 0, so cli/sti are permitted.
                    without_interrupts(init);
                } else {
                    init();
                }
            }

            fn write(&self, buffer: &[u8]) {
                match self {
                    Uart16550::Io(uart) => {
                        let send = || {
                            let Some(mut uart) = uart.try_lock() else {
                                debug_assert!(
                                    false,
                                    "UART 16550 I/O device lock must not be re-entered",
                                );
                                return;
                            };

                            uart.send_bytes_exact(buffer);
                        };
                        if current_privilege_level() == 0 {
                            // CPL is 0, so cli/sti are permitted.
                            without_interrupts(send);
                        } else {
                            send();
                        }
                    }
                    Uart16550::Mmio(uart) => {
                        let send = || {
                            let Some(mut uart) = uart.try_lock() else {
                                debug_assert!(
                                    false,
                                    "UART 16550 MMIO device lock must not be re-entered",
                                );
                                return;
                            };

                            uart.send_bytes_exact(buffer);
                        };
                        if current_privilege_level() == 0 {
                            // CPL is 0, so cli/sti are permitted.
                            without_interrupts(send);
                        } else {
                            send();
                        }
                    }
                }
            }

            fn read(&self) -> u8 {
                match self {
                    Uart16550::Io(uart) => {
                        let Some(mut uart) = uart.try_lock() else {
                            debug_assert!(
                                false,
                                "UART 16550 I/O device lock must not be re-entered",
                            );
                            return 0;
                        };

                        let mut byte = [0];
                        uart.receive_bytes_exact(&mut byte);
                        byte[0]
                    }
                    Uart16550::Mmio(uart) => {
                        let Some(mut uart) = uart.try_lock() else {
                            debug_assert!(
                                false,
                                "UART 16550 MMIO device lock must not be re-entered",
                            );
                            return 0;
                        };

                        let mut byte = [0];
                        uart.receive_bytes_exact(&mut byte);
                        byte[0]
                    }
                }
            }

            fn try_read(&self) -> Option<u8> {
                match self {
                    Uart16550::Io(uart) => {
                        uart.try_lock().and_then(|mut uart| uart.try_receive_byte().ok())
                    }
                    Uart16550::Mmio(uart) => {
                        uart.try_lock().and_then(|mut uart| uart.try_receive_byte().ok())
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
                    UniqueMmioPointer::new(
                        NonNull::new(self.base_address as *mut Pl011Registers)
                            .expect("PL011 base address must be non-null"),
                    )
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
