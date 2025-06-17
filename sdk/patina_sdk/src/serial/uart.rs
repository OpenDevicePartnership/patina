//! [SerialIO](crate::serial::SerialIO) UART implementations.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
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
    if #[cfg(any(feature = "doc", all(target_os = "uefi", target_arch = "x86_64")))] {
        extern crate alloc;

        use uart_16550::MmioSerialPort;
        use uart_16550::SerialPort as IoSerialPort;
        use x86_64::instructions::interrupts;

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
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        serial_port.init();
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        serial_port.init();
                    }
                }
            }

            fn write(&self, buffer: &[u8]) {
                match self {
                    Uart16550::Io { base } => {
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        interrupts::without_interrupts(|| {
                            for b in buffer {
                                serial_port.send(*b);
                            }
                        });
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        interrupts::without_interrupts(|| {
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
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        serial_port.receive()
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        serial_port.receive()
                    }
                }
            }

            fn try_read(&self) -> Option<u8> {
                match self {
                    Uart16550::Io { base } => {
                        let mut serial_port = unsafe { IoSerialPort::new(*base) };
                        if let Ok(value) = serial_port.try_receive() {
                            Some(value)
                        } else {
                            None
                        }
                    }
                    Uart16550::Mmio { base, reg_stride } => {
                        let mut serial_port = unsafe { MmioSerialPort::new_with_stride(*base, *reg_stride) };
                        if let Ok(value) = serial_port.try_receive() {
                            Some(value)
                        } else {
                            None
                        }
                    }
                }
            }

        }
    }
}

cfg_if::cfg_if! {
    if #[cfg(any(feature = "doc", all(target_os = "uefi", target_arch = "aarch64")))] {
        mod uart_pl011 {
            pub const FLAG_REGISTER_OFFSET: usize = 0x18;
            pub const FR_BUSY: u8 = 1 << 3;
            pub const FR_RXFE: u8 = 1 << 4;
            pub const FR_TXFF: u8 = 1 << 5;
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
            /// The given base address must point to the 8 MMIO control registers of a
            /// PL011 device, which must be mapped into the address space of the process
            /// as device memory and not have any other aliases.
            pub const fn new(base_address: usize) -> Self {
                Self { base_address }
            }

            /// Writes a single byte to the UART.
            pub fn write_byte(&self, byte: u8) {
                // Wait until there is room in the TX buffer.
                while self.read_flag_register() & uart_pl011::FR_TXFF != 0 {}

                // SAFETY: We know that the base address points to the control
                // registers of a PL011 device which is appropriately mapped.
                unsafe {
                    // Write to the TX buffer.
                    self.get_base().write_volatile(byte);
                }

                // Wait until the UART is no longer busy.
                while self.read_flag_register() & uart_pl011::FR_BUSY != 0 {}
            }

            /// Reads a single byte from the UART.
            pub fn read_byte(&self) -> Option<u8> {
                // Wait until the RX buffer is not empty.
                if self.read_flag_register() & uart_pl011::FR_RXFE != 0 {
                    return None;
                }

                // SAFETY: We know that the base address points to the control
                // registers of a PL011 device which is appropriately mapped.
                unsafe {
                    // Read from the RX buffer.
                    Some(self.get_base().read_volatile())
                }
            }

            fn read_flag_register(&self) -> u8 {
                // SAFETY: We know that the base address points to the control
                // registers of a PL011 device which is appropriately mapped.
                unsafe { self.get_base().add(uart_pl011::FLAG_REGISTER_OFFSET).read_volatile() }
            }

            fn get_base(&self) -> *mut u8 {
                self.base_address as *mut u8
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
