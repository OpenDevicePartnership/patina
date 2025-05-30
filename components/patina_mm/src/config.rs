//! Management Mode (MM) Configuration
//!
//! Defines the configuration necessary for the MM environment to be initialized and used by components
//! dependent on MM details.
//!
//! ## MM Configuration Usage
//!
//! It is expected that the MM configuration will be initialized by the environment that registers services for the
//! platform. The configuration can have platform-fixed values assigned during its initialization. It should be common
//! for at least the communication buffers to be populated as a mutable configuration during boot time. It is
//! recommended for a "MM Configuration" component to handle all MM configuration details with minimal other MM related
//! dependencies and lock the configuration so it is available for components that depend on the immutable configuration
//! to perform MM operations.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;
use alloc::vec::Vec;
use core::fmt;
use core::pin::Pin;
use core::ptr::NonNull;

use r_efi::efi;

/// Management Mode (MM) Configuration
///
/// A standardized configuration structure for MM components to use when initializing and using MM services.
#[derive(Debug, Clone)]
pub struct MmCommunicationConfiguration {
    pub acpi_base: AcpiBase,
    pub cmd_port: MmiPort,
    pub data_port: MmiPort,
    pub comm_buffers: Vec<CommunicateBuffer>,
}

impl Default for MmCommunicationConfiguration {
    fn default() -> Self {
        MmCommunicationConfiguration {
            acpi_base: AcpiBase::Mmio(0),
            cmd_port: MmiPort::Smi(0xFF),
            data_port: MmiPort::Smi(0x00),
            comm_buffers: Vec::new(),
        }
    }
}

/// UEFI MM Communicate Header
///
/// A standard header that must be present at the beginning of any MM communication buffer.
///
/// ## Notes
///
/// - This only supports V1 and V2 of the MM Communicate header format.
#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct EfiMmCommunicateHeader {
    /// Allows for disambiguation of the message format.
    /// Used to identify the registered MM handlers that should be given the message.
    pub header_guid: efi::Guid,
    /// The size of Data (in bytes) and does not include the size of the header.
    pub message_length: usize,
}

impl EfiMmCommunicateHeader {
    /// Returns the communicate header as a slice of bytes.
    pub fn as_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, EfiMmCommunicateHeader::size()) }
    }
    /// Returns the size of the header in bytes.
    pub const fn size() -> usize {
        core::mem::size_of::<Self>()
    }
}

/// MM Communicator Service Status Codes
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CommunicateBufferStatus {
    /// The buffer is too small to hold the header.
    TooSmallForHeader,
    /// The buffer is too small to hold the message.
    TooSmallForMessage,
    /// A valid recipient GUID was not provided.
    InvalidRecipient,
    /// The buffer is empty.
    Empty,
}

/// Management Mode (MM) Communicate Buffer
///
/// A buffer used for communication between the MM handler and the caller.
#[derive(Clone)]
pub struct CommunicateBuffer {
    /// Pointer to the buffer in memory.
    buffer: NonNull<u8>,
    /// ID of the buffer.
    id: u8,
    /// Length of the total buffer in bytes.
    length: usize,
    /// Message length in bytes.
    message_length: usize,
    /// Recipient GUID of the MM handler.
    recipient: Option<efi::Guid>,
}

impl CommunicateBuffer {
    /// Creates a new `CommunicateBuffer` with the given buffer and ID.
    ///
    /// ## Safety
    ///
    /// - The buffer must be a valid pointer to a memory region of at least `size` bytes.
    /// - The buffer must not be null.
    /// - The buffer must have a static lifetime.
    /// - The buffer must not be moved in memory while it is being used.
    /// - The buffer must not be used by any other code.
    pub unsafe fn new(buffer: Pin<&'static mut [u8]>, id: u8) -> Self {
        let length = buffer.len();
        let buffer_ptr =
            NonNull::new(Pin::into_inner(buffer).as_mut_ptr()).expect("CommunicateBuffer::new: null buffer pointer");
        Self { buffer: buffer_ptr, id, length, message_length: 0, recipient: None }
    }

    /// Returns a reference to the buffer as a slice of bytes.
    pub fn as_slice(&self) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts(self.buffer.as_ptr(), self.length) }
    }

    /// Returns a mutable reference to the buffer as a slice of bytes.
    pub fn as_slice_mut(&self) -> &'static mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.buffer.as_ptr(), self.length) }
    }

    /// Creates a new `CommunicateBuffer` from a raw pointer and size.
    ///
    /// ## Safety
    ///
    /// - The buffer must be a valid pointer to a memory region of at least `size` bytes.
    /// - The buffer must not be null.
    /// - The buffer must have a static lifetime.
    /// - The buffer must not be moved in memory while it is being used.
    /// - The buffer must not be used by any other code.
    pub unsafe fn from_raw_parts(buffer: *mut u8, size: usize, id: u8) -> Self {
        if size == 0 {
            panic!("CommunicateBuffer::from_raw_parts: size is zero");
        }
        if buffer.is_null() {
            panic!("CommunicateBuffer::from_raw_parts: null buffer pointer");
        }
        Self::new(unsafe { Pin::new_unchecked(core::slice::from_raw_parts_mut(buffer, size)) }, id)
    }

    /// Returns the length of the buffer.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Returns whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the ID of the buffer.
    pub fn id(&self) -> u8 {
        self.id
    }

    /// Sets the information needed for a communication message to be sent to the MM handler.
    ///
    /// ## Parameters
    ///
    /// - `recipient`: The GUID of the recipient MM handler.
    pub fn set_message_info(&mut self, recipient: efi::Guid) -> Result<(), CommunicateBufferStatus> {
        if self.len() < Self::message_start_offset() {
            return Err(CommunicateBufferStatus::TooSmallForHeader);
        }

        self.recipient = Some(recipient);
        Ok(())
    }

    /// Sets the data message used for communication with the MM handler.
    ///
    /// ## Parameters
    ///
    /// - `message`: The message to be sent to the MM handler. The message length in the communicate header is
    ///              set to the length of this slice.
    pub fn set_message(&mut self, message: &[u8]) -> Result<(), CommunicateBufferStatus> {
        if message.len() > self.message_capacity() {
            return Err(CommunicateBufferStatus::TooSmallForMessage);
        }
        let recipient = if let Some(recipient) = self.recipient {
            recipient
        } else {
            return Err(CommunicateBufferStatus::InvalidRecipient);
        };
        self.message_length = message.len();

        self.as_slice_mut()[..Self::message_start_offset()].copy_from_slice(
            EfiMmCommunicateHeader { header_guid: recipient, message_length: self.message_length }.as_bytes(),
        );
        self.as_slice_mut()[Self::message_start_offset()..Self::message_start_offset() + self.message_length]
            .copy_from_slice(message);

        Ok(())
    }

    /// Returns a slice to the message part of the communicate buffer.
    pub fn get_message(&self) -> Vec<u8> {
        self.as_slice()[Self::message_start_offset()..].to_vec()
    }

    /// Returns the available capacity for the message part of the communicate buffer.
    ///
    /// Note: Zero will be returned if the buffer is too small to hold the header.
    pub fn message_capacity(&self) -> usize {
        self.len().saturating_sub(Self::message_start_offset())
    }

    /// Returns the offset in the buffer where the message starts.
    const fn message_start_offset() -> usize {
        EfiMmCommunicateHeader::size()
    }
}

#[cfg(not(tarpaulin_include))]
impl fmt::Debug for CommunicateBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "CommunicateBuffer(id: 0x{:X}. len: 0x{:X})", self.id(), self.len())?;
        for (i, chunk) in self.as_slice().chunks(16).enumerate() {
            // Print the offset
            write!(f, "{:08X}: ", i * 16)?;
            // Print the hex values
            for byte in chunk {
                write!(f, "{:02X} ", byte)?;
            }
            // Add spacing for incomplete rows
            if chunk.len() < 16 {
                write!(f, "{}", "   ".repeat(16 - chunk.len()))?;
            }
            // Print ASCII representation
            write!(f, " |")?;
            for byte in chunk {
                if byte.is_ascii_graphic() || *byte == b' ' {
                    write!(f, "{}", *byte as char)?;
                } else {
                    write!(f, ".")?;
                }
            }
            writeln!(f, "|")?;
        }
        Ok(())
    }
}

/// Management Mode Interrupt (MMI) Port
#[derive(Copy, Clone)]
pub enum MmiPort {
    /// System Management Interrupt (SMI) Port for MM communication
    ///
    /// An SMI Port is a 16-bit integer value which indicates the port used for SMI communication.
    Smi(u16),
    /// Secure Monitor Call (SMC) Function ID for MM communication
    ///
    /// An SMC Function Identifier is a 32-bit integer value which indicates which function is being requested by
    /// the caller. It is always passed as the first argument to every SMC call in R0 or W0.
    Smc(u32),
}

impl fmt::Debug for MmiPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MmiPort::Smi(port) => write!(f, "MmiPort::Smi(0x{:04X})", port),
            MmiPort::Smc(port) => write!(f, "MmiPort::Smc(0x{:08X})", port),
        }
    }
}

/// ACPI Base Address
///
/// Represents the base address for ACPI MMIO or IO ports. This is the address used to access the ACPI Fixed hardware
/// register set.
#[derive(PartialEq, Copy, Clone)]
pub enum AcpiBase {
    Mmio(usize),
    Io(u16),
}

impl fmt::Debug for AcpiBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcpiBase::Mmio(addr) => write!(f, "AcpiBase::Mmio(0x{:X})", addr),
            AcpiBase::Io(port) => write!(f, "AcpiBase::Io(0x{:04X})", port),
        }
    }
}

impl From<*const u32> for AcpiBase {
    fn from(ptr: *const u32) -> Self {
        let addr = ptr as usize;
        AcpiBase::Mmio(addr)
    }
}

impl From<*const u64> for AcpiBase {
    fn from(ptr: *const u64) -> Self {
        let addr = ptr as usize;
        AcpiBase::Mmio(addr)
    }
}

impl From<usize> for AcpiBase {
    fn from(addr: usize) -> Self {
        AcpiBase::Mmio(addr)
    }
}

impl From<u16> for AcpiBase {
    fn from(port: u16) -> Self {
        AcpiBase::Io(port)
    }
}

impl AcpiBase {
    pub fn get_io_value(&self) -> u16 {
        match self {
            AcpiBase::Mmio(_) => 0,
            AcpiBase::Io(port) => *port,
        }
    }

    pub fn get_mmio_value(&self) -> usize {
        match self {
            AcpiBase::Mmio(addr) => *addr,
            AcpiBase::Io(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r_efi::efi::Guid;

    #[test]
    fn test_set_message_info_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);

        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());
    }

    #[test]
    fn test_set_message_info_failure_too_small_for_header() {
        let buffer: &'static mut [u8; 2] = Box::leak(Box::new([0u8; 2]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);

        // The buffer is too small to hold the header, so this should fail
        assert_eq!(comm_buffer.set_message_info(recipient_guid), Err(CommunicateBufferStatus::TooSmallForHeader));
    }

    #[test]
    fn test_set_message_failure_too_small_for_message() {
        let buffer: &'static mut [u8; CommunicateBuffer::message_start_offset()] =
            Box::leak(Box::new([0u8; CommunicateBuffer::message_start_offset()]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);

        assert_eq!(comm_buffer.set_message_info(recipient_guid), Ok(()));

        assert_eq!(
            comm_buffer.set_message("Test message data".as_bytes()),
            Err(CommunicateBufferStatus::TooSmallForMessage)
        );
    }

    #[test]
    fn test_set_message_failure_invalid_recipient() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        assert_eq!(
            comm_buffer.set_message("Test message data".as_bytes()),
            Err(CommunicateBufferStatus::InvalidRecipient)
        );
    }

    #[test]
    fn test_set_message_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);
        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());

        let message = b"MM Handler!";
        assert!(comm_buffer.set_message(message).is_ok());
        assert_eq!(comm_buffer.len(), 64);
        assert_eq!(comm_buffer.message_length, message.len());
        assert!(!comm_buffer.is_empty());
        assert_eq!(comm_buffer.id(), 1);

        let stored_message =
            &comm_buffer.as_slice()[EfiMmCommunicateHeader::size()..EfiMmCommunicateHeader::size() + message.len()];
        assert_eq!(stored_message, message);
    }

    #[test]
    fn test_set_message_failure() {
        let buffer: &'static mut [u8; 16] = Box::leak(Box::new([0u8; 16]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        let message = b"MM Handler!"; // Message too large for the total comm buffer
        assert_eq!(comm_buffer.set_message(message), Err(CommunicateBufferStatus::TooSmallForMessage));
    }

    #[test]
    fn test_get_message_success() {
        const MESSAGE: &[u8] = b"MM Handler!";
        const MESSAGE_SIZE: usize = MESSAGE.len();
        const COMM_BUFFER_SIZE: usize = CommunicateBuffer::message_start_offset() + MESSAGE_SIZE;

        let buffer: &'static mut [u8; COMM_BUFFER_SIZE] = Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        let message = MESSAGE;
        assert!(
            comm_buffer
                .set_message_info(Guid::from_fields(
                    0x12345678,
                    0x1234,
                    0x5678,
                    0x90,
                    0xAB,
                    &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]
                ))
                .is_ok(),
            "Failed to set the message info"
        );
        assert!(comm_buffer.set_message(message).is_ok(), "Failed to set the message");

        let stored_message: Vec<u8> = comm_buffer.get_message();
        assert_eq!(
            stored_message,
            &comm_buffer.as_slice().to_vec()
                [CommunicateBuffer::message_start_offset()..CommunicateBuffer::message_start_offset() + message.len()]
        );
    }

    #[test]
    fn test_set_message_info_multiple_times_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = unsafe { CommunicateBuffer::new(Pin::new(buffer), 1) };

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);
        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());
        assert_eq!(comm_buffer.recipient, Some(recipient_guid));

        let message = b"MM Handler!";
        assert!(comm_buffer.set_message(message).is_ok());
        assert_eq!(comm_buffer.get_message()[..message.len()], message.to_vec());
        assert_eq!(comm_buffer.len(), 64);
        assert_eq!(comm_buffer.message_length, message.len());

        let recipient_guid2 =
            Guid::from_fields(0x3210FEDC, 0xABCD, 0xABCD, 0x12, 0x23, &[0x12, 0x34, 0x56, 0x78, 0x90, 0xAB]);
        assert!(comm_buffer.set_message_info(recipient_guid2).is_ok());
        assert_eq!(comm_buffer.recipient, Some(recipient_guid2));
        assert_eq!(comm_buffer.get_message()[..message.len()], message.to_vec());
        assert_eq!(comm_buffer.len(), 64);
        assert_eq!(comm_buffer.message_length, message.len());
    }

    #[test]
    #[should_panic(expected = "CommunicateBuffer::from_raw_parts: size is zero")]
    fn test_from_raw_parts_zero_size() {
        let buffer: &'static mut [u8; 0] = Box::leak(Box::new([]));
        let size = buffer.len();
        let id = 1;
        unsafe { CommunicateBuffer::from_raw_parts(buffer.as_mut_ptr(), size, id) };
    }

    #[test]
    #[should_panic(expected = "CommunicateBuffer::from_raw_parts: null buffer pointer")]
    fn test_from_raw_parts_null_pointer() {
        let buffer: *mut u8 = core::ptr::null_mut();
        let size = 64;
        let id = 1;
        unsafe { CommunicateBuffer::from_raw_parts(buffer, size, id) };
    }

    #[test]
    fn test_from_raw_parts_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));

        let header = unsafe { &mut *(buffer.as_mut_ptr() as *mut EfiMmCommunicateHeader) };
        header.header_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);
        header.message_length = 16;

        let size = buffer.len();
        let id = 1;
        let comm_buffer = unsafe { CommunicateBuffer::from_raw_parts(buffer.as_mut_ptr(), size, id) };

        assert_eq!(comm_buffer.len(), size);
        assert_eq!(comm_buffer.id, id);
        assert_eq!(comm_buffer.as_slice().len(), size);
        assert_eq!(
            comm_buffer.as_slice()[0..EfiMmCommunicateHeader::size()],
            header.as_bytes()[0..EfiMmCommunicateHeader::size()]
        );
    }

    #[test]
    fn test_smiport_debug_msg() {
        let smi_port = MmiPort::Smi(0xFF);
        let debug_msg: String = format!("{:?}", smi_port);
        assert_eq!(debug_msg, "MmiPort::Smi(0x00FF)");
    }

    #[test]
    fn test_smcport_debug_msg_smc() {
        let smc_port = MmiPort::Smc(0x12345678);
        let debug_msg = format!("{:?}", smc_port);
        assert_eq!(debug_msg, "MmiPort::Smc(0x12345678)");
    }

    #[test]
    fn test_acpibase_debug_msg() {
        let acpi_base_mmio = AcpiBase::Mmio(0x12345678);
        let debug_msg_mmio = format!("{:?}", acpi_base_mmio);
        assert_eq!(debug_msg_mmio, "AcpiBase::Mmio(0x12345678)");

        let acpi_base_io = AcpiBase::Io(0x1234);
        let debug_msg_io = format!("{:?}", acpi_base_io);
        assert_eq!(debug_msg_io, "AcpiBase::Io(0x1234)");
    }

    #[test]
    fn test_acpibase_get_io_value() {
        let acpi_base = AcpiBase::Io(0x1234);
        assert_eq!(acpi_base.get_io_value(), 0x1234);
    }

    #[test]
    fn test_acpibase_get_mmio_value() {
        let acpi_base = AcpiBase::Mmio(0x12345678);
        assert_eq!(acpi_base.get_mmio_value(), 0x12345678);
    }

    #[test]
    fn test_acpibase_from_u32_ptr() {
        let ptr: *const u32 = 0x12345678 as *const u32;
        let acpi_base: AcpiBase = ptr.into();
        assert_eq!(acpi_base, AcpiBase::Mmio(0x12345678));
    }

    #[test]
    fn test_acpibase_from_u64_ptr() {
        let ptr: *const u64 = 0x0123456789ABCDEF as *const u64;
        let acpi_base: AcpiBase = ptr.into();
        assert_eq!(acpi_base, AcpiBase::Mmio(0x0123456789ABCDEF));
    }

    #[test]
    fn test_acpibase_from_usize() {
        let addr: usize = 0x12345678;
        let acpi_base: AcpiBase = addr.into();
        assert_eq!(acpi_base, AcpiBase::Mmio(0x12345678));
    }

    #[test]
    fn test_acpibase_from_u16() {
        let port: u16 = 0x1234;
        let acpi_base: AcpiBase = port.into();
        assert_eq!(acpi_base, AcpiBase::Io(0x1234));
    }
}
