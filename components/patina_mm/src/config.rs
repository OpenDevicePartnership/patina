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
use alloc::vec::Vec;
use core::{fmt, pin::Pin, ptr::NonNull};

use patina::{Guid, base::UEFI_PAGE_MASK};
use r_efi::efi;
use zerocopy_derive::*;

/// MM Communication Buffer Status
///
/// Shared structure between DXE and MM environments to communicate the status
/// of MM communication operations. This structure is written by DXE before
/// triggering an MMI and read/written by MM during MMI processing.
///
/// This is a structure currently used in some MM Supervisor MM implementations.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct MmCommBufferStatus {
    /// Whether the data in the fixed MM communication buffer is valid when entering from non-MM to MM.
    /// Must be set to TRUE before triggering MMI, will be set to FALSE by MM after processing.
    pub is_comm_buffer_valid: u8,

    /// The channel used to communicate with MM.
    /// FALSE = user buffer, TRUE = supervisor buffer
    pub talk_to_supervisor: u8,

    /// The return status when returning from MM to non-MM.
    pub return_status: u64,

    /// The size in bytes of the output buffer when returning from MM to non-MM.
    pub return_buffer_size: u64,
}

impl Default for MmCommBufferStatus {
    #[coverage(off)]
    fn default() -> Self {
        Self::new()
    }
}

impl MmCommBufferStatus {
    /// Create a new mailbox status with all fields zeroed
    pub const fn new() -> Self {
        Self { is_comm_buffer_valid: 0, talk_to_supervisor: 0, return_status: 0, return_buffer_size: 0 }
    }
}

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
    /// Enable runtime buffer updates (currently via the MM Communication Buffer Update Protocol).
    /// When enabled, the communicator will register a protocol notify to update
    /// the buffer specified by `updatable_buffer_id`.
    pub enable_comm_buffer_updates: bool,
    /// Buffer ID to update when MM Communication Buffer Update Protocol is installed.
    /// Only used when `enable_comm_buffer_updates` is true.
    /// If None when updates are enabled, no buffer will be updated.
    pub updatable_buffer_id: Option<u8>,
}

impl Default for MmCommunicationConfiguration {
    fn default() -> Self {
        MmCommunicationConfiguration {
            acpi_base: AcpiBase::Mmio(0),
            cmd_port: MmiPort::Smi(0xFF),
            data_port: MmiPort::Smi(0x00),
            comm_buffers: Vec::new(),
            enable_comm_buffer_updates: false,
            updatable_buffer_id: None,
        }
    }
}

impl fmt::Display for MmCommunicationConfiguration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "MM Communication Configuration:")?;
        writeln!(f, "  ACPI Base: {}", self.acpi_base)?;
        writeln!(f, "  Command Port: {}", self.cmd_port)?;
        writeln!(f, "  Data Port: {}", self.data_port)?;
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

/// UEFI MM Communicate Header
///
/// A standard header that must be present at the beginning of any MM communication buffer.
///
/// ## Notes
///
/// - This only supports V1 and V2 of the MM Communicate header format.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EfiMmCommunicateHeader {
    /// Allows for disambiguation of the message format.
    /// Used to identify the registered MM handlers that should be given the message.
    header_guid: efi::Guid,
    /// The size of Data (in bytes) and does not include the size of the header.
    message_length: usize,
}

impl EfiMmCommunicateHeader {
    /// Create a new communicate header with the specified GUID and message length.
    pub fn new(header_guid: Guid, message_length: usize) -> Self {
        Self { header_guid: header_guid.to_efi_guid(), message_length }
    }

    /// Returns the communicate header as a slice of bytes using safe conversion.
    ///
    /// Useful if byte-level access to the header structure is needed.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: EfiMmCommunicateHeader is repr(C) with well-defined layout and size
        unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, Self::size()) }
    }

    /// Returns the size of the header in bytes.
    pub const fn size() -> usize {
        core::mem::size_of::<Self>()
    }

    /// Get the header GUID from the communication buffer.
    ///
    /// Returns `Some(guid)` if the buffer has been properly initialized with a GUID,
    /// or `None` if the buffer is not initialized.
    ///
    /// # Returns
    ///
    /// The GUID from the communication header if available.
    ///
    /// # Errors
    ///
    /// Returns an error if the communication buffer header cannot be read.
    pub fn header_guid(&self) -> Guid<'_> {
        Guid::from_ref(&self.header_guid)
    }

    /// Returns the message length from this communicate header.
    ///
    /// The length represents the size of the message data that follows the header.
    ///
    /// # Returns
    ///
    /// The length in bytes of the message data (excluding the header size).
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
    /// Handler GUID tracked independently to check against comm buffer contents
    private_recipient: Option<efi::Guid>,
    /// Message length tracked independently to check against comm buffer contents
    private_message_length: usize,
    /// Whether this buffer is enabled and should be used for communication.
    /// Disabled buffers are skipped when searching for buffers by ID.
    enabled: bool,
    /// Pointer to the MM communication buffer status mailbox structure.
    /// This is used to communicate status between DXE and MM environments.
    /// If None, this is a buffer without mailbox status support.
    status_mailbox: Option<NonNull<MmCommBufferStatus>>,
}

impl CommunicateBuffer {
    /// The minimum required buffer size to hold a communication header.
    const MINIMUM_BUFFER_SIZE: usize = EfiMmCommunicateHeader::size();

    /// The offset in the buffer where the message starts.
    const MESSAGE_START_OFFSET: usize = EfiMmCommunicateHeader::size();

    /// Creates a new `CommunicateBuffer` with the given buffer and ID.
    pub fn new(mut buffer: Pin<&'static mut [u8]>, id: u8) -> Self {
        let length = buffer.len();
        log::debug!(target: "mm_comm", "Creating new CommunicateBuffer: id={}, size=0x{:X}", id, length);
        buffer.fill(0);

        let ptr: NonNull<[u8]> = NonNull::from_mut(Pin::into_inner(buffer));

        log::trace!(target: "mm_comm", "CommunicateBuffer {} created successfully at address {:p}", id, ptr);
        Self {
            buffer: ptr,
            id,
            length,
            private_recipient: None,
            private_message_length: 0,
            enabled: true,
            status_mailbox: None,
        }
    }

    /// Returns a reference to the buffer as a slice of bytes.
    /// This is only used for internal operations.
    fn as_slice(&self) -> &[u8] {
        // SAFETY: The pointer was validated during CommunicateBuffer construction
        unsafe { self.buffer.as_ref() }
    }

    /// Returns a mutable reference to the buffer as a slice of bytes.
    /// This is only used for internal operations.
    fn as_slice_mut(&mut self) -> &mut [u8] {
        // SAFETY: The pointer was validated during CommunicateBuffer construction
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
        log::trace!(target: "mm_comm", "Creating CommunicateBuffer from raw parts: id={}, ptr={:p}, size=0x{:X}", id, buffer, size);

        if size < Self::MINIMUM_BUFFER_SIZE {
            log::error!(target: "mm_comm", "Buffer {} too small: size=0x{:X}, minimum=0x{:X}", id, size, Self::MINIMUM_BUFFER_SIZE);
            return Err(CommunicateBufferStatus::TooSmallForHeader);
        }

        if buffer.is_null() {
            log::error!(target: "mm_comm", "Buffer {} has null pointer", id);
            return Err(CommunicateBufferStatus::NoBuffer);
        }

        if (buffer as usize) & UEFI_PAGE_MASK != 0 {
            log::error!(target: "mm_comm", "Buffer {} not page aligned: address=0x{:X}, mask=0x{:X}", id, buffer as usize, UEFI_PAGE_MASK);
            return Err(CommunicateBufferStatus::NotAligned);
        }

        if buffer as usize > usize::MAX - size {
            log::error!(target: "mm_comm", "Buffer {} address overflow: ptr=0x{:X}, size=0x{:X}", id, buffer as usize, size);
            return Err(CommunicateBufferStatus::AddressValidationFailed);
        }

        log::debug!(target: "mm_comm", "CommunicateBuffer {} validation passed, creating buffer", id);
        // SAFETY: Caller guarantees pointer validity per function safety contract
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
    /// - If provided, the status_mailbox_address points to a valid MmCommBufferStatus structure
    pub unsafe fn from_firmware_region(
        address: u64,
        size_bytes: usize,
        buffer_id: u8,
        status_mailbox_address: Option<u64>,
    ) -> Result<Self, CommunicateBufferStatus> {
        // Check that the address provided is addressable on this system.
        // A 32-bit system will fail this if the address is over 4GB.
        let address = usize::try_from(address).map_err(|_| CommunicateBufferStatus::AddressValidationFailed)?;

        if address.checked_add(size_bytes).is_none() {
            return Err(CommunicateBufferStatus::AddressValidationFailed);
        }

        let ptr = address as *mut u8;

        log::info!(
            target: "mm_comm",
            "Creating CommunicateBuffer from firmware region: addr=0x{:X}, size=0x{:X}, id={}",
            address,
            size_bytes,
            buffer_id
        );

        // SAFETY: Caller guarantees firmware memory region is valid and stable per the function safety contract
        let mut buffer = unsafe { Self::from_raw_parts(ptr, size_bytes, buffer_id)? };

        // Set up the status mailbox if provided
        if let Some(status_addr) = status_mailbox_address {
            let status_addr =
                usize::try_from(status_addr).map_err(|_| CommunicateBufferStatus::AddressValidationFailed)?;
            let status_ptr = status_addr as *mut MmCommBufferStatus;

            // SAFETY: Caller guarantees the status mailbox address is valid
            buffer.status_mailbox = NonNull::new(status_ptr);
            log::info!(target: "mm_comm", "Buffer {} status mailbox configured at address 0x{:X}", buffer_id, status_addr);
        }

        Ok(buffer)
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

    /// Returns whether this buffer is enabled for communication.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Disables this buffer, preventing it from being used for communication.
    /// Disabled buffers are skipped when searching for buffers by ID.
    pub fn disable(&mut self) {
        log::debug!(target: "mm_comm", "Disabling comm buffer {}", self.id);
        self.enabled = false;
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

    /// Sets the communication buffer status to indicate a valid buffer before triggering MMI.
    ///
    /// This must be called before triggering the SW MMI to inform the MM core that
    /// the communication buffer contains valid data to process.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if the status was set successfully
    /// - `Err(CommunicateBufferStatus)` if this buffer has no status mailbox configured
    pub fn set_comm_buffer_valid(&mut self) -> Result<(), CommunicateBufferStatus> {
        if let Some(mut status_ptr) = self.status_mailbox {
            // SAFETY: The status mailbox pointer was validated during buffer creation
            unsafe {
                let status = status_ptr.as_mut();
                status.is_comm_buffer_valid = 1; // TRUE
                status.talk_to_supervisor = 0; // FALSE - use user buffer
                log::trace!(target: "mm_comm", "Buffer {} status mailbox: IsCommBufferValid=TRUE", self.id);
            }
            Ok(())
        } else {
            log::error!(target: "mm_comm", "Buffer {} has no status mailbox configured", self.id);
            Err(CommunicateBufferStatus::NoBuffer)
        }
    }

    /// Reads the return status from the MM communication after MMI completes.
    ///
    /// Returns the return status and buffer size set by the MM handler.
    ///
    /// ## Returns
    ///
    /// - `Ok((return_status, return_buffer_size))` if the status was read successfully
    /// - `Err(CommunicateBufferStatus)` if this buffer has no status mailbox configured
    pub fn get_mm_return_status(&self) -> Result<(u64, u64), CommunicateBufferStatus> {
        if let Some(status_ptr) = self.status_mailbox {
            // SAFETY: The status mailbox pointer was validated during buffer creation
            unsafe {
                let status = status_ptr.as_ref();
                // Copy packed fields to avoid unaligned reference errors
                let is_valid = status.is_comm_buffer_valid;
                let return_status = status.return_status;
                let return_buffer_size = status.return_buffer_size;

                log::trace!(
                    target: "mm_comm",
                    "Buffer {} return status: IsCommBufferValid={}, ReturnStatus=0x{:X}, ReturnBufferSize=0x{:X}",
                    self.id,
                    is_valid,
                    return_status,
                    return_buffer_size
                );
                Ok((return_status, return_buffer_size))
            }
        } else {
            log::error!(target: "mm_comm", "Buffer {} has no status mailbox configured", self.id);
            Err(CommunicateBufferStatus::NoBuffer)
        }
    }

    /// Returns whether this buffer has a status mailbox configured.
    pub fn has_status_mailbox(&self) -> bool {
        self.status_mailbox.is_some()
    }

    /// Resets the communication buffer by clearing all data and resetting internal state.
    pub fn reset(&mut self) {
        // Zero out the entire buffer
        self.as_slice_mut().fill(0);

        // Reset internal state
        self.private_message_length = 0;
        self.private_recipient = None;
    }

    /// Returns the available capacity for the message part of the communicate buffer.
    ///
    /// Note: Zero will be returned if the buffer is too small to hold the header.
    pub fn message_capacity(&self) -> usize {
        self.len().saturating_sub(Self::MESSAGE_START_OFFSET)
    }

    /// Verifies that the internal state matches what is in the memory buffer.
    /// This is intended to catch corruption and ensure the buffer actually matches what has
    /// been requested through the MM Communication API.
    ///
    /// Returns `Ok(())` if state verification passes, otherwise returns the appropriate error.
    fn verify_state_consistency(&self) -> Result<(), CommunicateBufferStatus> {
        if self.len() < Self::MESSAGE_START_OFFSET {
            log::error!(target: "mm_comm", "Buffer {} is too small for the communicate header", self.id);
            return Err(CommunicateBufferStatus::TooSmallForHeader);
        }

        let header_slice = &self.as_slice()[..Self::MESSAGE_START_OFFSET];

        // SAFETY: Buffer size validated, efi::Guid is repr(C) at offset 0
        let memory_guid = unsafe { core::ptr::read(header_slice.as_ptr() as *const efi::Guid) };

        // SAFETY: Buffer size validated, usize at offset 16 after Guid
        let memory_message_length = unsafe { core::ptr::read(header_slice.as_ptr().add(16) as *const usize) };

        // Verify that thee recipient matches
        match self.private_recipient {
            Some(expected_guid) => {
                if memory_guid != expected_guid {
                    log::error!(target: "mm_comm", "Buffer {} GUID mismatch: private={:?}, memory={:?}",
                        self.id, expected_guid, memory_guid);
                    return Err(CommunicateBufferStatus::InvalidRecipient);
                }
            }
            None => {
                // If no recipient is set privately, the memory should contain all zeros for the GUID
                let zero_guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
                if memory_guid != zero_guid {
                    log::error!(target: "mm_comm", "Buffer {} unexpected GUID in memory when none set privately", self.id);
                    return Err(CommunicateBufferStatus::InvalidRecipient);
                }
            }
        }

        // Verify message length matches
        if memory_message_length != self.private_message_length {
            log::error!(target: "mm_comm", "Buffer {} message length mismatch: private={}, memory={}",
                self.id, self.private_message_length, memory_message_length);
            return Err(CommunicateBufferStatus::TooSmallForMessage);
        }

        log::trace!(target: "mm_comm", "Buffer {} state consistency was verified successfully", self.id);
        Ok(())
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
        log::trace!(target: "mm_comm", "Validating capacity for buffer {}: buffer_size={}, message_size={}",
            self.id, self.len(), message_size);

        // First check if buffer can hold the header
        if self.len() < Self::MESSAGE_START_OFFSET {
            log::error!(target: "mm_comm", "Buffer {} too small for header: size={}, header_size={}",
                self.id, self.len(), Self::MESSAGE_START_OFFSET);
            return Err(CommunicateBufferStatus::TooSmallForHeader);
        }

        // Then check if remaining space can hold the message
        let available_message_space = self.len() - Self::MESSAGE_START_OFFSET;
        if message_size > available_message_space {
            log::error!(target: "mm_comm", "Buffer {} too small for message: available_space={}, message_size={}",
                self.id, available_message_space, message_size);
            return Err(CommunicateBufferStatus::TooSmallForMessage);
        }

        log::trace!(target: "mm_comm", "Buffer {} capacity validation passed", self.id);
        Ok(())
    }

    /// Sets the information needed for a communication message to be sent to the MM handler.
    /// Updates both the internal state and the memory buffer, then verifies consistency.
    ///
    /// ## Parameters
    ///
    /// - `recipient`: The GUID of the recipient MM handler.
    pub fn set_message_info(&mut self, recipient: Guid) -> Result<(), CommunicateBufferStatus> {
        log::trace!(target: "mm_comm", "Setting message info for buffer {}: recipient={}", self.id, recipient);

        // Validate capacity first
        self.validate_capacity(0)?;

        // Update private state
        let recipient_efi = recipient.to_efi_guid();
        self.private_recipient = Some(recipient_efi);

        // Update memory buffer using safe byte operations
        let header = EfiMmCommunicateHeader::new(recipient, self.private_message_length);
        let header_bytes = header.as_bytes();
        self.as_slice_mut()[..Self::MESSAGE_START_OFFSET].copy_from_slice(header_bytes);

        // Verify state consistency after update
        self.verify_state_consistency()?;

        log::trace!(target: "mm_comm", "Message info set successfully for buffer {}", self.id);
        Ok(())
    }

    /// Sets the data message used for communication with the MM handler.
    /// Updates both the internal state and the memory buffer, then verifies consistency.
    ///
    /// ## Parameters
    ///
    /// - `message`: The message to be sent to the MM handler. The message length in the communicate header is
    ///   set to the length of this slice.
    pub fn set_message(&mut self, message: &[u8]) -> Result<(), CommunicateBufferStatus> {
        log::trace!(target: "mm_comm", "Setting message for buffer {}: message_size={}", self.id, message.len());

        self.validate_capacity(message.len())?;

        let recipient = self.private_recipient.ok_or_else(|| {
            log::error!(target: "mm_comm", "Buffer {} has no recipient set", self.id);
            CommunicateBufferStatus::InvalidRecipient
        })?;

        // Update private state
        self.private_message_length = message.len();

        log::trace!(target: "mm_comm", "Buffer {}: writing header and message data", self.id);

        // Update memory buffer using safe byte operations for header
        let header = EfiMmCommunicateHeader::new(Guid::from_ref(&recipient), message.len());
        let header_bytes = header.as_bytes();
        self.as_slice_mut()[..Self::MESSAGE_START_OFFSET].copy_from_slice(header_bytes);

        // Copy message data
        self.as_slice_mut()[Self::MESSAGE_START_OFFSET..Self::MESSAGE_START_OFFSET + message.len()]
            .copy_from_slice(message);

        // Verify state consistency after update
        self.verify_state_consistency()?;

        log::debug!(target: "mm_comm", "Buffer {} message set successfully: header_size={}, message_size={}",
            self.id, Self::MESSAGE_START_OFFSET, message.len());
        Ok(())
    }

    /// Returns a copy of the message part of the communicate buffer.
    /// This method uses the internal state and verifies consistency with memory.
    ///
    /// Note: This method extracts the actual message content using verified state tracking.
    pub fn get_message(&self) -> Result<Vec<u8>, CommunicateBufferStatus> {
        // Verify state consistency before proceeding
        self.verify_state_consistency()?;

        if self.private_message_length == 0 {
            log::trace!(target: "mm_comm", "Buffer {} has zero-length message", self.id);
            return Ok(Vec::new());
        }

        let start_offset = Self::MESSAGE_START_OFFSET;
        let end_offset = start_offset + self.private_message_length;

        // Ensure we don't read beyond the buffer
        if end_offset > self.len() {
            log::error!(target: "mm_comm", "Buffer {} message extends beyond buffer: end_offset={}, buffer_len={}",
                self.id, end_offset, self.len());
            return Err(CommunicateBufferStatus::TooSmallForMessage);
        }

        let message = self.as_slice()[start_offset..end_offset].to_vec();
        log::trace!(target: "mm_comm", "Retrieved message from buffer {}: message_size={}", self.id, message.len());
        Ok(message)
    }

    /// Returns the header GUID from the current communicate buffer.
    /// This method uses the internal state and verifies consistency with memory.
    ///
    /// Returns `None` if no recipient has been set.
    pub fn get_header_guid(&self) -> Result<Option<Guid<'_>>, CommunicateBufferStatus> {
        // Verify state consistency first
        self.verify_state_consistency()?;

        log::trace!(target: "mm_comm", "Buffer {} header GUID retrieved from private state", self.id);
        Ok(self.private_recipient.as_ref().map(Guid::from_ref))
    }

    /// Returns the message length from the current communicate buffer.
    /// This method uses the internal state and verifies consistency with memory.
    pub fn get_message_length(&self) -> Result<usize, CommunicateBufferStatus> {
        // Verify state consistency first
        self.verify_state_consistency()?;

        log::trace!(target: "mm_comm", "Buffer {} message length retrieved from private state: len={}",
            self.id, self.private_message_length);
        Ok(self.private_message_length)
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

impl fmt::Display for MmiPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MmiPort::Smi(value) => write!(f, "SMI(0x{value:04X})"),
            MmiPort::Smc(value) => write!(f, "SMC(0x{value:08X})"),
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

impl fmt::Display for AcpiBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcpiBase::Mmio(addr) => write!(f, "MMIO(0x{addr:X})"),
            AcpiBase::Io(port) => write!(f, "IO(0x{port:04X})"),
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

    #[repr(align(4096))]
    struct AlignedBuffer([u8; 64]);

    #[test]
    fn test_set_message_info_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();
        let expected_bytes = recipient_guid.as_bytes();

        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());

        // Test that state verification works
        assert!(comm_buffer.get_header_guid().is_ok());
        assert_eq!(comm_buffer.get_header_guid().unwrap().as_ref().map(|g| g.as_bytes()), Some(expected_bytes));
    }

    #[test]
    fn test_set_message_info_failure_too_small_for_header() {
        let buffer: &'static mut [u8; 2] = Box::leak(Box::new([0u8; 2]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();

        // The buffer is too small to hold the header, so this should fail
        assert_eq!(comm_buffer.set_message_info(recipient_guid), Err(CommunicateBufferStatus::TooSmallForHeader));
    }

    #[test]
    fn test_set_message_failure_too_small_for_message() {
        let buffer: &'static mut [u8; CommunicateBuffer::MINIMUM_BUFFER_SIZE] =
            Box::leak(Box::new([0u8; CommunicateBuffer::MINIMUM_BUFFER_SIZE]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();

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

        let recipient_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();
        assert!(comm_buffer.set_message_info(recipient_guid).is_ok());

        let message = b"MM Handler!";
        assert!(comm_buffer.set_message(message).is_ok());
        assert_eq!(comm_buffer.len(), 64);
        assert!(!comm_buffer.is_empty());
        assert_eq!(comm_buffer.id(), 1);

        // Test that we can retrieve the message
        let retrieved_message = comm_buffer.get_message().unwrap();
        assert_eq!(retrieved_message, message);

        // Test that state verification is successful
        assert_eq!(comm_buffer.get_message_length().unwrap(), message.len());
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

        let recipient_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();
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

        let test_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();

        assert!(comm_buffer.set_message_info(test_guid).is_ok(), "Failed to set the message info");
        assert!(comm_buffer.set_message(MESSAGE).is_ok(), "Failed to set the message");

        let retrieved_message = comm_buffer.get_message().unwrap();
        assert_eq!(retrieved_message, MESSAGE.to_vec());
    }

    #[test]
    fn test_set_message_info_multiple_times_success() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let recipient_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();
        assert!(comm_buffer.set_message_info(recipient_guid.clone()).is_ok());
        assert_eq!(
            comm_buffer.get_header_guid().unwrap().as_ref().map(|g| g.as_bytes()),
            Some(recipient_guid.as_bytes())
        );

        let message = b"MM Handler!";
        assert!(comm_buffer.set_message(message).is_ok());
        assert_eq!(comm_buffer.get_message().unwrap(), message.to_vec());
        assert_eq!(comm_buffer.len(), 64);
        assert_eq!(comm_buffer.get_message_length().unwrap(), message.len());

        // Update with new recipient
        let recipient_guid2 =
            Guid::from_fields(0x3210FEDC, 0xABCD, 0xABCD, 0x12, 0x23, [0x12, 0x34, 0x56, 0x78, 0x90, 0xAB]);
        assert!(comm_buffer.set_message_info(recipient_guid2.clone()).is_ok());
        assert_eq!(
            comm_buffer.get_header_guid().unwrap().as_ref().map(|g| g.as_bytes()),
            Some(recipient_guid2.as_bytes())
        );

        // Message should still be there but header should be updated
        assert_eq!(comm_buffer.get_message().unwrap(), message.to_vec());
        assert_eq!(comm_buffer.len(), 64);
        assert_eq!(comm_buffer.get_message_length().unwrap(), message.len());
    }

    #[test]
    fn test_from_raw_parts_zero_size() {
        let buffer: &'static mut [u8; 0] = Box::leak(Box::new([]));
        let size = buffer.len();
        let id = 1;
        // SAFETY: Test validates error handling for zero-sized buffer
        let result = unsafe { CommunicateBuffer::from_raw_parts(buffer.as_mut_ptr(), size, id) };
        assert!(matches!(result, Err(CommunicateBufferStatus::TooSmallForHeader)));
    }

    #[test]
    fn test_from_raw_parts_null_pointer() {
        let buffer: *mut u8 = core::ptr::null_mut();
        let size = 64;
        let id = 1;
        // SAFETY: Test validates error handling for null pointer
        let result = unsafe { CommunicateBuffer::from_raw_parts(buffer, size, id) };
        assert!(matches!(result, Err(CommunicateBufferStatus::NoBuffer)));
    }

    #[test]
    fn test_set_comm_buffer_valid_without_status_mailbox() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(&mut buffer[..]), 1);
        assert!(matches!(comm_buffer.set_comm_buffer_valid(), Err(CommunicateBufferStatus::NoBuffer)));
    }

    #[test]
    fn test_get_mm_return_status_without_status_mailbox() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let comm_buffer = CommunicateBuffer::new(Pin::new(&mut buffer[..]), 1);
        assert!(matches!(comm_buffer.get_mm_return_status(), Err(CommunicateBufferStatus::NoBuffer)));
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

        // SAFETY: Test buffer is 4K-aligned, valid, and leaked for static lifetime
        let result = unsafe { CommunicateBuffer::from_firmware_region(addr, size, id, None) };
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

        // SAFETY: Test validates error handling for address overflow
        let result = unsafe { CommunicateBuffer::from_firmware_region(addr, size, id, None) };
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
        // SAFETY: Test buffer is 4K-aligned, valid, and owned by test
        let comm_buffer = unsafe { CommunicateBuffer::from_raw_parts(buffer.as_mut_ptr(), size, id).unwrap() };

        assert_eq!(comm_buffer.len(), size);
        assert_eq!(comm_buffer.id(), id);

        // Test that the buffer is zeroed initially
        assert_eq!(comm_buffer.get_header_guid().unwrap(), None);
        assert_eq!(comm_buffer.get_message_length().unwrap(), 0);
    }

    #[test]
    fn test_state_consistency_verification() {
        let buffer: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let mut comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        let test_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();
        let test_message = b"test message";

        assert!(comm_buffer.set_message_info(test_guid.clone()).is_ok());
        assert!(comm_buffer.set_message(test_message).is_ok());

        // Test that the getters pass consistency checks and return the expected values
        assert_eq!(comm_buffer.get_header_guid().unwrap().as_ref().map(|g| g.as_bytes()), Some(test_guid.as_bytes()));
        assert_eq!(comm_buffer.get_message_length().unwrap(), test_message.len());
        assert_eq!(comm_buffer.get_message().unwrap(), test_message.to_vec());
    }

    #[test]
    fn test_buffer_too_small_for_header_operations() {
        let buffer: &'static mut [u8; 2] = Box::leak(Box::new([0u8; 2]));
        let comm_buffer = CommunicateBuffer::new(Pin::new(buffer), 1);

        // All operations should fail with appropriate errors for undersized buffers
        assert!(matches!(comm_buffer.get_header_guid(), Err(CommunicateBufferStatus::TooSmallForHeader)));
        assert!(matches!(comm_buffer.get_message_length(), Err(CommunicateBufferStatus::TooSmallForHeader)));
        assert!(matches!(comm_buffer.get_message(), Err(CommunicateBufferStatus::TooSmallForHeader)));
    }

    // Tests for other structures remain the same as they don't depend on CommunicateBuffer
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
    fn test_acpibase_display_msg() {
        let acpi_base_mmio = AcpiBase::Mmio(0x12345678);
        let display_msg_mmio = format!("{acpi_base_mmio}");
        assert_eq!(display_msg_mmio, "MMIO(0x12345678)");

        let acpi_base_io = AcpiBase::Io(0x1234);
        let display_msg_io = format!("{acpi_base_io}");
        assert_eq!(display_msg_io, "IO(0x1234)");
    }

    #[test]
    fn test_mmiport_display_msg() {
        let smi_port = MmiPort::Smi(0xFF);
        let display_msg_smi = format!("{smi_port}");
        assert_eq!(display_msg_smi, "SMI(0x00FF)");

        let smc_port = MmiPort::Smc(0x12345678);
        let display_msg_smc = format!("{smc_port}");
        assert_eq!(display_msg_smc, "SMC(0x12345678)");
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

    #[test]
    fn test_efi_mm_communicate_header_header_guid() {
        let test_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();
        let message_length = 42usize;

        let header = EfiMmCommunicateHeader::new(test_guid.clone(), message_length);

        let returned_guid = header.header_guid();
        assert_eq!(returned_guid.as_bytes(), test_guid.as_bytes());

        let test_guid2 =
            Guid::from_fields(0xDEADBEEF, 0xCAFE, 0xDCBA, 0x12, 0x34, [0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        let header2 = EfiMmCommunicateHeader::new(test_guid2.clone(), message_length);

        let returned_guid2 = header2.header_guid();
        assert_eq!(returned_guid2.as_bytes(), test_guid2.as_bytes());

        assert_ne!(returned_guid.as_bytes(), returned_guid2.as_bytes());
    }

    #[test]
    fn test_efi_mm_communicate_header_message_length() {
        let test_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();

        let test_lengths = [0usize, 1, 42, 1024, usize::MAX];

        for &length in &test_lengths {
            let header = EfiMmCommunicateHeader::new(test_guid.clone(), length);
            assert_eq!(header.message_length(), length);
        }
    }

    #[test]
    fn test_efi_mm_communicate_header_size() {
        let expected_size = core::mem::size_of::<r_efi::efi::Guid>() + core::mem::size_of::<usize>();
        assert_eq!(EfiMmCommunicateHeader::size(), expected_size);

        let test_guid = Guid::try_from_string("12345678-1234-5678-90AB-CDEF01234567").unwrap();
        let header = EfiMmCommunicateHeader::new(test_guid, 42);
        assert_eq!(header.as_bytes().len(), EfiMmCommunicateHeader::size());
    }

    #[test]
    fn test_mm_communication_configuration_display() {
        let default_config = MmCommunicationConfiguration::default();
        let display_output = format!("{}", default_config);

        let expected_lines = [
            "MM Communication Configuration:",
            "  ACPI Base: MMIO(0x0)",
            "  Command Port: SMI(0x00FF)",
            "  Data Port: SMI(0x0000)",
            "  Communication Buffers (0):",
            "    <none>",
        ];

        for expected_line in &expected_lines {
            assert!(
                display_output.contains(expected_line),
                "Display output should contain: '{}'\nActual output:\n{}",
                expected_line,
                display_output
            );
        }

        let buffer1: &'static mut [u8; 64] = Box::leak(Box::new([0u8; 64]));
        let buffer2: &'static mut [u8; 128] = Box::leak(Box::new([0u8; 128]));

        let comm_buffer1 = CommunicateBuffer::new(Pin::new(buffer1), 1);
        let comm_buffer2 = CommunicateBuffer::new(Pin::new(buffer2), 2);

        let populated_config = MmCommunicationConfiguration {
            acpi_base: AcpiBase::Io(0x1234),
            cmd_port: MmiPort::Smc(0x87654321),
            data_port: MmiPort::Smi(0xABCD),
            comm_buffers: vec![comm_buffer1, comm_buffer2],
            enable_comm_buffer_updates: false,
            updatable_buffer_id: None,
        };

        let populated_display = format!("{}", populated_config);

        assert!(populated_display.contains("MM Communication Configuration:"));
        assert!(populated_display.contains("  ACPI Base: IO(0x1234)"));
        assert!(populated_display.contains("  Command Port: SMC(0x87654321)"));
        assert!(populated_display.contains("  Data Port: SMI(0xABCD)"));
        assert!(populated_display.contains("  Communication Buffers (2):"));

        assert!(populated_display.contains("Buffer 0x01:"));
        assert!(populated_display.contains("Buffer 0x02:"));
        assert!(populated_display.contains("len=0x40")); // 64 bytes = 0x40
        assert!(populated_display.contains("len=0x80")); // 128 bytes = 0x80
    }
}
