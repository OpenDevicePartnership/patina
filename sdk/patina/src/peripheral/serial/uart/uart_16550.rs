//! [`Uart16550`] — a 16550 UART [`SerialIO`](crate::peripheral::serial::SerialIO) implementation.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ptr::NonNull;

use crate::log_debug_assert;
use uart_16550::backend::{MmioBackend, PioBackend};
use uart_16550::{BaudRate, Config, Uart16550 as Inner};

/// The configuration applied the first time a `Uart16550` port is used.
const CONFIG: Config = Config { baud_rate: BaudRate::Baud38400, ..Config::DEFAULT };

/// A lazily-initialized I/O port-mapped backend.
///
/// Construction only records the base port. The underlying driver is validated but not
/// hardware-probed on first use (see `driver`).
#[derive(Debug)]
pub struct IoPort {
    base: u16,
    inner: Option<Inner<PioBackend>>,
}

impl IoPort {
    const fn new(base: u16) -> Self {
        Self { base, inner: None }
    }

    /// Returns the constructed driver, building it from the base port on the first call.
    /// Returns `None` if the port is rejected (e.g. its register range overflows `u16`). The
    /// attempt is retried on every call since there is not much overhead to retry.
    ///
    /// This only validates the address, it does not probe for hardware presence, so ports that
    /// don't implement a full 16550 register file still work for raw reads/writes even though
    /// `init` below can't confirm that they're present.
    fn driver(&mut self) -> Option<&mut Inner<PioBackend>> {
        if self.inner.is_none() {
            // SAFETY: Forwarded from `Uart16550::new_io`'s caller contract that `self.base` is a
            // valid I/O port range, exclusively owned for the lifetime of this `Uart16550`.
            self.inner = unsafe { Inner::new_port(self.base) }.ok();
        }
        self.inner.as_mut()
    }

    /// Applies [`CONFIG`] to the device. Some ports may not have a probeable identity, so a failed
    /// presence probe here does not block later reads/writes, it just means the baud rate/format may
    /// not match [`CONFIG`].
    fn init(&mut self) {
        if let Some(driver) = self.driver() {
            let _ = driver.init(CONFIG);
        }
    }
}

/// A lazily-initialized Memory Mapped I/O backend.
///
/// Construction only records the base address and register stride. The underlying driver is
/// validated (including rejecting a null base address) but not hardware-probed on first use (see
/// `driver`).
#[derive(Debug)]
pub struct MmioPort {
    base: usize,
    reg_stride: u8,
    inner: Option<Inner<MmioBackend>>,
}

impl MmioPort {
    const fn new(base: usize, reg_stride: u8) -> Self {
        Self { base, reg_stride, inner: None }
    }

    /// Returns the constructed driver, building it from the base address/stride on the first
    /// call. Returns `None` if the base address is null or the address/stride is otherwise
    /// rejected. The attempt is retried on every call since there is not much overhead to retry.
    ///
    /// This only validates the address, it does not probe for hardware presence, so devices
    /// that don't implement a full, probeable 16550 register file still work for raw reads/writes
    /// even though `init` below can't confirm they're present.
    fn driver(&mut self) -> Option<&mut Inner<MmioBackend>> {
        if self.inner.is_none() {
            let base = NonNull::new(core::ptr::with_exposed_provenance_mut::<u8>(self.base))?;
            // SAFETY: Forwarded from `Uart16550::new_mmio`'s caller contract that `self.base` is a
            // valid, exclusively-owned MMIO register range for the lifetime of this `Uart16550`.
            self.inner = unsafe { Inner::new_mmio(base, self.reg_stride) }.ok();
        }
        self.inner.as_mut()
    }

    /// Applies [`CONFIG`] to the device. Some ports may not have a probeable identity, so a failed
    /// presence probe here does not block later reads/writes, it just means the baud rate/format may
    /// not match [`CONFIG`].
    fn init(&mut self) {
        if let Some(driver) = self.driver() {
            let _ = driver.init(CONFIG);
        }
    }
}

/// An interface for writing to a Uart16550 device.
///
/// Each variant owns the underlying serial port. The owning `&mut` access required by the
/// [`SerialIO`](crate::peripheral::serial::SerialIO) methods guarantees exclusive use of the device, so no
/// interior mutability or per-operation reconstruction is required.
///
/// Only the address/stride are validated during construction so it is infallible. The underlying
/// driver is built lazily on first use, and hardware presence is only probed by an explicit call
/// to `SerialIO::init`. A rejected address or a failed presence check degrades to a no-op rather than panicking
/// or blocking boot.
#[derive(Debug)]
pub enum Uart16550 {
    /// The I/O port-mapped interface for the Uart16550 serial port.
    Io(IoPort),
    /// The Memory Mapped I/O interface for the Uart16550 serial port.
    Mmio(MmioPort),
}

impl Uart16550 {
    /// Creates a new I/O port-mapped Uart16550 interface at the given base port.
    ///
    /// # Safety
    ///
    /// The caller must ensure `base` points to a valid Uart16550 I/O port range and that
    /// the caller has the rights to perform the I/O operations on it, for as long as the
    /// returned value is used.
    pub const unsafe fn new_io(base: u16) -> Self {
        Uart16550::Io(IoPort::new(base))
    }

    /// Creates a new memory-mapped Uart16550 interface at the given base address and
    /// register stride.
    ///
    /// This will not fail, even for a null or otherwise invalid address. The address and stride
    /// are validated lazily on first use. Hardware presence is probed by an explicit call to
    /// `SerialIO::init`.
    ///
    /// # Safety
    ///
    /// The caller must ensure `base` points to a valid, exclusively-owned Uart16550 MMIO
    /// register range with the given `reg_stride` between consecutive registers, for as long as
    /// the returned value is used.
    pub const unsafe fn new_mmio(base: usize, reg_stride: u8) -> Self {
        Uart16550::Mmio(MmioPort::new(base, reg_stride))
    }
}

impl crate::peripheral::serial::SerialIO for Uart16550 {
    fn init(&mut self) {
        match self {
            Uart16550::Io(port) => port.init(),
            Uart16550::Mmio(port) => port.init(),
        }
    }

    fn write(&mut self, buffer: &[u8]) {
        if buffer.is_empty() {
            return;
        }
        match self {
            Uart16550::Io(port) => {
                if let Some(driver) = port.driver() {
                    driver.send_bytes_exact(buffer);
                }
            }
            Uart16550::Mmio(port) => {
                if let Some(driver) = port.driver() {
                    driver.send_bytes_exact(buffer);
                }
            }
        }
    }

    fn read(&mut self) -> u8 {
        // Blocks until a byte is available once the device is constructed. A rejected address
        // logs an error and panics in debug builds. In release builds, a `0` sentinel is returned
        // instead of spinning forever.
        let mut byte = 0u8;
        match self {
            Uart16550::Io(port) => {
                if let Some(driver) = port.driver() {
                    driver.receive_bytes_exact(core::slice::from_mut(&mut byte));
                } else {
                    log_debug_assert!("Uart16550::read on a rejected I/O port. Returning a 0 sentinel byte");
                }
            }
            Uart16550::Mmio(port) => {
                if let Some(driver) = port.driver() {
                    driver.receive_bytes_exact(core::slice::from_mut(&mut byte));
                } else {
                    log_debug_assert!("Uart16550::read on a rejected MMIO port. Returning a 0 sentinel byte");
                }
            }
        }
        byte
    }

    fn try_read(&mut self) -> Option<u8> {
        match self {
            Uart16550::Io(port) => port.driver()?.try_receive_byte().ok(),
            Uart16550::Mmio(port) => port.driver()?.try_receive_byte().ok(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::peripheral::serial::SerialIO;

    struct FakeMmio {
        // The size of the full 8-register file (NUM_REGISTERS).
        regs: [u8; 8],
    }

    impl FakeMmio {
        // Register offsets (stride = 1).
        const DATA: usize = 0;
        const INT_EN: usize = 1;
        const FIFO_CTRL: usize = 2;
        const LINE_CTRL: usize = 3;
        const MODEM_CTRL: usize = 4;
        const LINE_STS: usize = 5;
        const MODEM_STS: usize = 6;

        // Line status register bits.
        const INPUT_FULL: u8 = 1;
        const OUTPUT_EMPTY: u8 = 1 << 5;
        const TRANSMITTER_EMPTY: u8 = 1 << 6;

        // Modem status register bits.
        const CLEAR_TO_SEND: u8 = 1 << 4;

        fn new() -> Self {
            let mut regs = [0u8; 8];
            // init() spins on TRANSMITTER_EMPTY. MSR::CTS is set too, though CONFIG doesn't
            // enable check_cts_before_sending, so ready_to_send() doesn't require it.
            regs[Self::LINE_STS] = Self::OUTPUT_EMPTY | Self::TRANSMITTER_EMPTY;
            regs[Self::MODEM_STS] = Self::CLEAR_TO_SEND;
            FakeMmio { regs }
        }

        fn set_rx_byte(&mut self, byte: u8) {
            self.regs[Self::DATA] = byte;
            self.regs[Self::LINE_STS] |= Self::INPUT_FULL;
        }

        fn uart(&mut self) -> Uart16550 {
            let base = self.regs.as_mut_ptr() as usize;
            // SAFETY: `base` points to an eight-byte register file (stride 1) that outlives the
            // returned interface and is exclusively owned for the duration of the test.
            unsafe { Uart16550::new_mmio(base, 1) }
        }
    }

    #[test]
    fn test_uart_16550_new_io_constructs_io_variant() {
        // SAFETY: constructing the interface does not perform any I/O; no port access occurs.
        let uart = unsafe { Uart16550::new_io(0x3F8) };
        assert!(matches!(uart, Uart16550::Io(_)));
    }

    #[test]
    fn test_uart_16550_new_mmio_constructs_mmio_variant() {
        // SAFETY: constructing the interface does not perform any I/O; no memory access occurs.
        let uart = unsafe { Uart16550::new_mmio(0x1000, 1) };
        assert!(matches!(uart, Uart16550::Mmio(_)));
    }

    #[test]
    fn test_uart_16550_mmio_init_programs_registers() {
        let mut fake = FakeMmio::new();
        fake.uart().init();

        // Values written by the 16550 default configuration (38400/8-N-1).
        assert_eq!(fake.regs[FakeMmio::DATA], 0x03);
        // `Config::DEFAULT` does not enable interrupts, so `IER` stays clear.
        assert_eq!(fake.regs[FakeMmio::INT_EN], 0x00);
        assert_eq!(fake.regs[FakeMmio::FIFO_CTRL], 0xC7);
        assert_eq!(fake.regs[FakeMmio::LINE_CTRL], 0x03);
        assert_eq!(fake.regs[FakeMmio::MODEM_CTRL], 0x0B);
    }

    #[test]
    fn test_uart_16550_mmio_write_sends_byte_to_data_register() {
        let mut fake = FakeMmio::new();
        fake.uart().write(b"A");
        assert_eq!(fake.regs[FakeMmio::DATA], b'A');
    }

    #[test]
    fn test_uart_16550_mmio_write_forwards_entire_buffer() {
        let mut fake = FakeMmio::new();
        // The data register holds a single byte, so only check the final byte of the buffer.
        fake.uart().write(b"abc");
        assert_eq!(fake.regs[FakeMmio::DATA], b'c');
    }

    #[test]
    fn test_uart_16550_mmio_write_empty_buffer_is_noop() {
        let mut fake = FakeMmio::new();
        fake.uart().write(b"");
        assert_eq!(fake.regs[FakeMmio::DATA], 0);
    }

    #[test]
    fn test_uart_16550_mmio_write_works_without_explicit_init() {
        let mut fake = FakeMmio::new();
        fake.uart().write(b"A");
        assert_eq!(fake.regs[FakeMmio::DATA], b'A');
        assert_eq!(fake.regs[FakeMmio::INT_EN], 0);
        assert_eq!(fake.regs[FakeMmio::FIFO_CTRL], 0);
        assert_eq!(fake.regs[FakeMmio::LINE_CTRL], 0);
        assert_eq!(fake.regs[FakeMmio::MODEM_CTRL], 0);
    }

    #[test]
    fn test_uart_16550_mmio_read_returns_available_byte() {
        let mut fake = FakeMmio::new();
        fake.set_rx_byte(b'Z');
        assert_eq!(fake.uart().read(), b'Z');
    }

    #[test]
    fn test_uart_16550_mmio_try_read_returns_none_when_empty() {
        let mut fake = FakeMmio::new();
        assert_eq!(fake.uart().try_read(), None);
    }

    #[test]
    fn test_uart_16550_mmio_try_read_returns_available_byte() {
        let mut fake = FakeMmio::new();
        fake.set_rx_byte(b'!');
        assert_eq!(fake.uart().try_read(), Some(b'!'));
    }

    #[test]
    fn test_uart_16550_mmio_null_base_never_panics() {
        // SAFETY: Intentionally violates the constructor's contract (a null base address) to
        // verify the driver no-ops instead of panicking.
        let mut uart = unsafe { Uart16550::new_mmio(0, 1) };
        uart.init();
        uart.write(b"unreachable");
        assert_eq!(uart.try_read(), None);
        assert_eq!(uart.read(), 0);
    }

    #[test]
    fn test_uart_16550_mmio_invalid_stride_never_panics() {
        let mut fake = FakeMmio::new();
        let base = fake.regs.as_mut_ptr() as usize;
        // SAFETY: Intentionally passes an invalid stride to verify the driver no-ops instead of
        // panicking.
        let mut uart = unsafe { Uart16550::new_mmio(base, 0) };
        uart.init();
        uart.write(b"unreachable");
        assert_eq!(uart.try_read(), None);
        assert_eq!(uart.read(), 0);
    }

    #[test]
    fn test_uart_16550_io_invalid_base_never_panics() {
        // SAFETY: `u16::MAX` overflows the device's register range.
        let mut uart = unsafe { Uart16550::new_io(u16::MAX) };
        uart.init();
        uart.write(b"unreachable");
        assert_eq!(uart.try_read(), None);
        assert_eq!(uart.read(), 0);
    }
}
