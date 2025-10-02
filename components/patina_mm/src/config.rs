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
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;
use alloc::{format, string::String, vec::Vec};
use core::fmt;
use core::pin::Pin;
use core::ptr::NonNull;

use patina::base::UEFI_PAGE_MASK;
use r_efi::efi;

/// Management Mode (MM) Configuration
///
/// A standardized configuration structure for MM components to use when initializing and using MM services.
#[derive(Debug, Clone)]
pub struct MmCommunicationConfiguration {
    /// ACPI base address used to access the ACPI Fixed hardware register set.
    pub acpi_base: AcpiBase,
    /// MMI Port for sending commands to the MM handler.
    pub cmd_port: MmiPort,
    /// MMI Port for receiving data from the MM handler.
    pub data_port: MmiPort,
    /// List of Management Mode (MM) Communicate Buffers
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

impl fmt::Display for MmCommunicationConfiguration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "MM Communication Configuration:")?;
        writeln!(f, "  ACPI Base: {}", format_acpi_base(&self.acpi_base))?;
        writeln!(f, "  Command Port: {}", format_mmi_port(&self.cmd_port))?;
        writeln!(f, "  Data Port: {}", format_mmi_port(&self.data_port))?;
        writeln!(f, "  Communication Buffers ({}):", self.comm_buffers.len())?;

        if self.comm_buffers.is_empty() {
            writeln!(f, "    <none>")
        } else {
            for buffer in &self.comm_buffers {
                writeln!(f, "    Buffer {:#04X}: ptr={:p}, len=0x{:X}", buffer.id(), buffer.as_ptr(), buffer.len(),)?;
            }
            Ok(())
        }
    }
}

fn format_mmi_port(port: &MmiPort) -> String {
    match port {
        MmiPort::Smi(value) => format!("SMI(0x{value:04X})"),
        MmiPort::Smc(value) => format!("SMC(0x{value:08X})"),
    }
}

fn format_acpi_base(base: &AcpiBase) -> String {
    match base {
        AcpiBase::Mmio(addr) => format!("MMIO(0x{addr:X})"),
        AcpiBase::Io(port) => format!("IO(0x{port:04X})"),
    }
}

/// UEFI MM Communicate Header
///
/// A standard header that must be present at the beginning of any MM communication buffer.
///
/// ## Notes
///
/// - This only supports V1 and V2 of the MM Communicate header format.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(crate) struct EfiMmCommunicateHeader {
    /// Allows for disambiguation of the message format.
    /// Used to identify the registered MM handlers that should be given the message.
    header_guid: efi::Guid,
    /// The size of Data (in bytes) and does not include the size of the header.
    message_length: usize,
}

impl EfiMmCommunicateHeader {
    /// Create a new communicate header with the specified GUID and message length.
    pub const fn new(header_guid: efi::Guid, message_length: usize) -> Self {
        Self { header_guid, message_length }
    }

    /// Returns the communicate header as a slice of bytes using safe conversion.
    ///
    /// Useful if byte-level access to the header structure is needed.
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            // SAFETY: EfiMmCommunicateHeader is repr(C) with a well-defined layout
            core::slice::from_raw_parts(self as *const _ as *const u8, Self::size())
        }
    }

    /// Returns the size of the header in bytes.
    pub const fn size() -> usize {
        core::mem::size_of::<Self>()
    }

    /// Returns the header GUID from this communicate header.
    ///
    /// # Returns
    ///
    /// The GUID that identifies the registered MM handler recipient.
    #[allow(dead_code)]
    pub const fn header_guid(&self) -> efi::Guid {
        self.header_guid
    }

    /// Returns the message length from this communicate header.
    ///
    /// The length represents the size of the message data that follows the header.
    ///
    /// # Returns
    ///
    /// The length in bytes of the message data (excluding the header size).
    #[allow(dead_code)]
    pub const fn message_length(&self) -> usize {
        self.message_length
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
    /// A comm buffer was not provided (null pointer).
    NoBuffer,
    /// The does not meet the alignment requirements.
    NotAligned,
    /// Buffer creation failed due to address space validation errors.
    AddressValidationFailed,
}

/// Management Mode (MM) Communicate Buffer
///
/// A buffer used for communication between the MM handler and the caller.
#[derive(Clone)]
pub struct CommunicateBuffer {
    /// Pointer to the buffer in memory.
    buffer: NonNull<[u8]>,
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
    /// The minimum required buffer size to hold a communication header.
    const MINIMUM_BUFFER_SIZE: usize = EfiMmCommunicateHeader::size();

    /// The offset in the buffer where the message starts.
    const MESSAGE_START_OFFSET: usize = EfiMmCommunicateHeader::size();

    /// Creates a new `CommunicateBuffer` with the given buffer and ID.
    pub fn new(mut buffer: Pin<&'static mut [u8]>, id: u8) -> Self {
        let length = buffer.len();
        buffer.fill(0);

        let ptr: NonNull<[u8]> = NonNull::from_mut(Pin::into_inner(buffer));

        Self { buffer: ptr, id, length, message_length: 0, recipient: None }
    }

    /// Returns a reference to the buffer as a slice of bytes.
    /// This is only used for internal operations.
    fn as_slice(&self) -> &[u8] {
        // SAFETY: The buffer pointer was validated before being stored
        //         in this CommunicateBuffer instance
        unsafe { self.buffer.as_ref() }
    }

    /// Returns a mutable reference to the buffer as a slice of bytes.
    /// This is only used for internal operations.
    fn as_slice_mut(&mut self) -> &mut [u8] {
        // SAFETY: The buffer pointer was validated before being stored
        //         in this CommunicateBuffer instance
        unsafe { self.buffer.as_mut() }
    }

    /// Creates a new `CommunicateBuffer` from a raw pointer and size.
    ///
    /// ## Safety
    ///
    /// - The buffer must be a valid pointer to a memory region of at least `size` bytes.
    /// - The buffer pointer must not be null.
    /// - The buffer must have a static lifetime.
    /// - The buffer must not be moved in memory while it is being used.
    /// - The buffer must not be used by any other code.
    /// - The buffer must be page (4k) aligned so paging attributes can be applied to it.
    /// - The buffer size must be sufficient to hold at least the MM communication header.
    pub unsafe fn from_raw_parts(buffer: *mut u8, size: usize, id: u8) -> Result<Self, CommunicateBufferStatus> {
        if size < Self::MINIMUM_BUFFER_SIZE {
            return Err(CommunicateBufferStatus::TooSmallForHeader);
        }

        if buffer.is_null() {
            return Err(CommunicateBufferStatus::NoBuffer);
        }

        if (buffer as usize) & UEFI_PAGE_MASK != 0 {
            return Err(CommunicateBufferStatus::NotAligned);
        }

        if buffer as usize > usize::MAX - size {
            return Err(CommunicateBufferStatus::AddressValidationFailed);
        }

        // SAFETY: Safety is upheld by the caller to this function (the function is marked unsafe)
        unsafe { Ok(Self::new(Pin::new(core::slice::from_raw_parts_mut(buffer, size)), id)) }
    }

    /// Creates a `CommunicateBuffer` from a validated firmware-provided memory region.
    ///
    /// This is the recommended method for creating communicate buffers from HOB data or other
    /// firmware-provided memory regions.
    ///
    /// ## Parameters
    ///
    /// - `address` - Physical address of the communication buffer
    /// - `size_bytes` - Size of the buffer in bytes
    /// - `buffer_id` - Unique identifier for this buffer
    ///   - Can be used in future calls to refer to the buffer
    ///
    /// ## Returns
    ///
    /// - `Ok(CommunicateBuffer)` - Successfully created and validated buffer
    /// - `Err(CommunicateBufferStatus)` - Validation failed with specific error
    ///
    /// ## Safety
    ///
    /// The caller must ensure:
    /// - The memory region is valid and accessible throughout buffer lifetime
    /// - The memory is not used by other components concurrently
    /// - The firmware has guaranteed the memory region is stable and properly mapped
    pub unsafe fn from_firmware_region(
        address: u64,
        size_bytes: usize,
        buffer_id: u8,
    ) -> Result<Self, CommunicateBufferStatus> {
        // Check that the address provided is addressable on this system.
        // A 32-bit system will fail this if the address is over 4GB.
        let address = usize::try_from(address).map_err(|_| CommunicateBufferStatus::AddressValidationFailed)?;

        if address.checked_add(size_bytes).is_none() {
            return Err(CommunicateBufferStatus::AddressValidationFailed);
        }

        let ptr = address as *mut u8;

        unsafe { Self::from_raw_parts(ptr, size_bytes, buffer_id) }
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

    /// Returns a pointer to the underlying buffer memory.
    ///
    /// This method provides controlled access to the buffer pointer for operations
    /// that require direct memory access, such as registering with hardware or
    /// passing to external APIs.
    ///
    /// ## Safety Considerations
    ///
    /// While this method is safe to call, the returned pointer should be used
    /// with caution. The caller must ensure they do not:
    ///
    /// - Write beyond the buffer boundaries (use `len()` to check size)
    /// - Modify buffer contents without proper coordination with buffer state
    /// - Use the pointer after the buffer has been dropped
    pub fn as_ptr(&self) -> *mut u8 {
        self.buffer.as_ptr().cast::<u8>()
    }

    /// Returns the available capacity for the message part of the communicate buffer.
    ///
    /// Note: Zero will be returned if the buffer is too small to hold the header.
    pub fn message_capacity(&self) -> usize {
        self.len().saturating_sub(Self::MESSAGE_START_OFFSET)
    }

    /// Validates that the buffer can accommodate a header and message of the given size.
    ///
    /// ## Arguments
    /// - `message_size` - The size of the message to validate
    ///
    /// ## Returns
    /// - `Ok(())` - The buffer can safely hold the header and message
    /// - `Err(status)` - Buffer validation failed
    fn validate_capacity(&self, message_size: usize) -> Result<(), CommunicateBufferStatus> {
        // First check if buffer can hold the header
        if self.len() < Self::MESSAGE_START_OFFSET {
            return Err(CommunicateBufferStatus::TooSmallForHeader);
        }

        // Then check if remaining space can hold the message
        let available_message_space = self.len() - Self::MESSAGE_START_OFFSET;
        if message_size > available_message_space {
            return Err(CommunicateBufferStatus::TooSmallForMessage);
        }

        Ok(())
    }

    /// Sets the information needed for a communication message to be sent to the MM handler.
    ///
    /// ## Parameters
    ///
    /// - `recipient`: The GUID of the recipient MM handler.
    pub fn set_message_info(&mut self, recipient: efi::Guid) -> Result<(), CommunicateBufferStatus> {
        if self.len() < Self::MESSAGE_START_OFFSET {
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
    ///   set to the length of this slice.
    pub fn set_message(&mut self, message: &[u8]) -> Result<(), CommunicateBufferStatus> {
        self.validate_capacity(message.len())?;

        let recipient = if let Some(recipient) = self.recipient {
            recipient
        } else {
            return Err(CommunicateBufferStatus::InvalidRecipient);
        };

        self.message_length = message.len();
        let message_length = self.message_length;

        let header = EfiMmCommunicateHeader::new(recipient, message_length);
        let slice = self.as_slice_mut();
        slice[..Self::MESSAGE_START_OFFSET].copy_from_slice(header.as_bytes());
        slice[Self::MESSAGE_START_OFFSET..Self::MESSAGE_START_OFFSET + message_length].copy_from_slice(message);

        Ok(())
    }

    /// Returns a slice to the message part of the communicate buffer.
    pub fn get_message(&self) -> Vec<u8> {
        self.as_slice()[Self::MESSAGE_START_OFFSET..].to_vec()
    }
}

#[coverage(off)]
impl fmt::Debug for CommunicateBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "CommunicateBuffer(id: 0x{:X}. len: 0x{:X})", self.id(), self.len())?;
        for (i, chunk) in self.as_slice().chunks(16).enumerate() {
            // Print the offset
            write!(f, "{:08X}: ", i * 16)?;
            // Print the hex values
            for byte in chunk {
                write!(f, "{byte:02X} ")?;
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
            MmiPort::Smi(port) => write!(f, "MmiPort::Smi(0x{port:04X})"),
            MmiPort::Smc(port) => write!(f, "MmiPort::Smc(0x{port:08X})"),
        }
    }
}

/// ACPI Base Address
///
/// Represents the base address for ACPI MMIO or IO ports. This is the address used to access the ACPI Fixed hardware
/// register set.
#[derive(PartialEq, Copy, Clone)]
pub enum AcpiBase {
    /// Memory-mapped IO (MMIO) base address for ACPI
    Mmio(usize),
    /// IO port base address for ACPI
    Io(u16),
}

impl fmt::Debug for AcpiBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcpiBase::Mmio(addr) => write!(f, "AcpiBase::Mmio(0x{addr:X})"),
            AcpiBase::Io(port) => write!(f, "AcpiBase::Io(0x{port:04X})"),
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
    /// Returns the IO port if this is an IO base, otherwise returns 0.
    pub fn get_io_value(&self) -> u16 {
        match self {
            AcpiBase::Mmio(_) => 0,
            AcpiBase::Io(port) => *port,
        }
    }

    /// Returns the MMIO address if this is an MMIO base, otherwise returns 0.
    pub fn get_mmio_value(&self) -> usize {
        match self {
            AcpiBase::Mmio(addr) => *addr,
            AcpiBase::Io(_) => 0,
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use r_efi::efi::Guid;

    #[repr(align(4096))]
    struct AlignedBuffer([u8; 64]);

    #[test]
    fn test_set_message_info_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);

        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());
    }

    #[test]
    fn test_set_message_info_failure_too_small_for_header() {
        let buffer: &'static mut [u8; 2] = Box::leak(Box::new([0u8; 2]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);

        // The buffer is too small to hold the header, so this should fail
        assert_eq!(comm_buffer.set_message_info(recipient_guid), Err(CommunicateBufferStatus::TooSmallForHeader));
    }

    #[test]
    fn test_set_message_failure_too_small_for_message() {
        let buffer: &'static mut [u8; CommunicateBuffer::MINIMUM_BUFFER_SIZE] =
            Box::leak(Box::new([0u8; CommunicateBuffer::MINIMUM_BUFFER_SIZE]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

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
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        // Should fail because no recipient was set
        assert_eq!(
            comm_buffer.set_message("Test message data".as_bytes()),
            Err(CommunicateBufferStatus::InvalidRecipient)
        );
    }

    #[test]
    fn test_set_message_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);
        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());

        let message = b"MM Handler!";
        assert!(comm_buffer.set_message(message).is_ok());
        assert_eq!(comm_buffer.len(), 64);
        assert!(!comm_buffer.is_empty());
        assert_eq!(comm_buffer.id(), 1);

        // Test that we can retrieve the message
        let retrieved_message = comm_buffer.get_message();
        assert_eq!(retrieved_message[..message.len()], *message);
    }

    #[test]
    fn test_set_message_failure_buffer_too_small() {
        // The buffer is too small for the header - capacity validation happens first
        let buffer: &'static mut [u8; 16] = Box::leak(Box::new([0u8; 16]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let message = b"MM Handler!";
        assert_eq!(comm_buffer.set_message(message), Err(CommunicateBufferStatus::TooSmallForHeader));

        // The buffer has room for the header but there is not enough room for the message
        let buffer2: &'static mut [u8; 30] = Box::leak(Box::new([0u8; 30]));
        let mut comm_buffer2 = CommunicateBuffer::new(Pin::new(buffer2), 2);

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);
        assert!(comm_buffer2.set_message_info(recipient_guid).is_ok());

        let long_message = b"This message is too long for the remaining space!";
        assert_eq!(comm_buffer2.set_message(long_message), Err(CommunicateBufferStatus::TooSmallForMessage));
    }

    #[test]
    fn test_get_message_success() {
        const MESSAGE: &[u8] = b"MM Handler!";
        const COMM_BUFFER_SIZE: usize = CommunicateBuffer::MESSAGE_START_OFFSET + MESSAGE.len();

        let buffer: &'static mut [u8; COMM_BUFFER_SIZE] = Box::leak(Box::new([0u8; COMM_BUFFER_SIZE]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let test_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);

        assert!(comm_buffer.set_message_info(test_guid).is_ok(), "Failed to set the message info");
        assert!(comm_buffer.set_message(MESSAGE).is_ok(), "Failed to set the message");

        let retrieved_message = comm_buffer.get_message();
        assert_eq!(retrieved_message[..MESSAGE.len()], *MESSAGE);
    }

    #[test]
    fn test_set_message_info_multiple_times_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid =
            Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x90, 0xAB, &[0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67]);
        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());

        let message = b"MM Handler!";
        assert!(comm_buffer.set_message(message).is_ok());
        let retrieved_message = comm_buffer.get_message();
        assert_eq!(retrieved_message[..message.len()], *message);
        assert_eq!(comm_buffer.len(), 64);

        // Update with new recipient
        let recipient_guid2 =
            Guid::from_fields(0x3210FEDC, 0xABCD, 0xABCD, 0x12, 0x23, &[0x12, 0x34, 0x56, 0x78, 0x90, 0xAB]);
        assert!(comm_buffer.set_message_info(recipient_guid2).is_ok());

        // Message should still be there but header should be updated
        let retrieved_message2 = comm_buffer.get_message();
        assert_eq!(retrieved_message2[..message.len()], *message);
        assert_eq!(comm_buffer.len(), 64);
    }

    #[test]
    fn test_from_raw_parts_zero_size() {
        let buffer: &'static mut [u8; 0] = Box::leak(Box::new([]));
        let size = buffer.len();
        let id = 1;
        let result = unsafe { CommunicateBuffer::from_raw_parts(buffer.as_mut_ptr(), size, id) };
        assert!(matches!(result, Err(CommunicateBufferStatus::TooSmallForHeader)));
    }

    #[test]
    fn test_from_raw_parts_null_pointer() {
        let buffer: *mut u8 = core::ptr::null_mut();
        let size = 64;
        let id = 1;
        let result = unsafe { CommunicateBuffer::from_raw_parts(buffer, size, id) };
        assert!(matches!(result, Err(CommunicateBufferStatus::NoBuffer)));
    }

    #[test]
    fn test_from_firmware_region_success() {
        use patina::base::UEFI_PAGE_SIZE;

        let aligned_buf = Box::new(AlignedBuffer([0u8; 64]));
        let buffer_ptr = aligned_buf.0.as_ptr();
        assert_eq!(buffer_ptr as usize & (UEFI_PAGE_SIZE - 1), 0, "Buffer is not 4K aligned");

        let addr = buffer_ptr as u64;
        let size = 64;
        let id = 1;

        let result = unsafe { CommunicateBuffer::from_firmware_region(addr, size, id) };
        assert!(result.is_ok());
        let comm_buffer = result.unwrap();
        assert_eq!(comm_buffer.len(), size);
        assert_eq!(comm_buffer.id(), id);
    }

    #[test]
    fn test_from_firmware_region_overflow() {
        let addr = u64::MAX;
        let size = 1;
        let id = 1;

        let result = unsafe { CommunicateBuffer::from_firmware_region(addr, size, id) };
        assert!(matches!(result, Err(CommunicateBufferStatus::AddressValidationFailed)));
    }

    #[test]
    fn test_from_raw_parts_success() {
        use patina::base::UEFI_PAGE_SIZE;

        let mut aligned_buf = Box::new(AlignedBuffer([0u8; 64]));
        let buffer = &mut aligned_buf.0;
        assert_eq!(buffer.as_ptr() as usize & (UEFI_PAGE_SIZE - 1), 0, "Buffer is not 4K aligned");

        let size = buffer.len();
        let id = 1;
        let comm_buffer = unsafe { CommunicateBuffer::from_raw_parts(buffer.as_mut_ptr(), size, id).unwrap() };

        assert_eq!(comm_buffer.len(), size);
        assert_eq!(comm_buffer.id(), id);

        // Test that the buffer is zeroed initially
        let message = comm_buffer.get_message();
        assert!(message.iter().all(|&x| x == 0));
    }

    #[test]
    fn test_smiport_debug_msg() {
        let smi_port = MmiPort::Smi(0xFF);
        let debug_msg: String = format!("{smi_port:?}");
        assert_eq!(debug_msg, "MmiPort::Smi(0x00FF)");
    }

    #[test]
    fn test_smcport_debug_msg_smc() {
        let smc_port = MmiPort::Smc(0x12345678);
        let debug_msg = format!("{smc_port:?}");
        assert_eq!(debug_msg, "MmiPort::Smc(0x12345678)");
    }

    #[test]
    fn test_acpibase_debug_msg() {
        let acpi_base_mmio = AcpiBase::Mmio(0x12345678);
        let debug_msg_mmio = format!("{acpi_base_mmio:?}");
        assert_eq!(debug_msg_mmio, "AcpiBase::Mmio(0x12345678)");

        let acpi_base_io = AcpiBase::Io(0x1234);
        let debug_msg_io = format!("{acpi_base_io:?}");
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
