//! CPU Architectural Protocol
//!
//! Abstracts the processor services that are required to implement some of the DXE services.
//!
//! See <https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#cpu-architectural-protocol>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

/// CPU Architectrural Protocol GUID
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.1
pub const PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x26baccb1, 0x6f42, 0x11d4, 0xbc, 0xe7, &[0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81]);

#[repr(C)]
pub enum CpuFlushType {
    EfiCpuFlushTypeWriteBackInvalidate,
    EfiCpuFlushTypeWriteBack,
    EFiCpuFlushTypeInvalidate,
}

/// Flushes a range of the processor's data cache.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.2
pub type FlushDataCache = extern "efiapi" fn(*const Protocol, efi::PhysicalAddress, u64, CpuFlushType) -> efi::Status;

/// Enables interrupt processing by the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.3
pub type EnableInterrupt = extern "efiapi" fn(*const Protocol) -> efi::Status;

/// Disables interrupt processing by the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.4
pub type DisableInterrupt = extern "efiapi" fn(*const Protocol) -> efi::Status;

/// Retrieves the processor's current interrupt state.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.5
pub type GetInterruptState = extern "efiapi" fn(*const Protocol, *mut bool) -> efi::Status;

#[repr(C)]
pub enum CpuInitType {
    EfiCpuInit,
}

/// Generates an INIT on the processor.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.6
pub type Init = extern "efiapi" fn(*const Protocol, CpuInitType) -> efi::Status;

/// Specifies which processor exception to hook.
///
/// # Documentation
/// UEFI Specification version 2.10, Section 18.2.5
pub type EfiExceptionType = isize;

/// Pointer to system context structure.
///
/// # Documentation
/// UEFI Specification version 2.10, Section 18.2.4
pub type EfiSystemContext = efi::protocols::debug_support::SystemContext;

/// Function type definition for interrupt handler.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.7
pub type InterruptHandler = extern "efiapi" fn(EfiExceptionType, EfiSystemContext);

/// Registers a function to be called from the processor interrupt handler.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.7
pub type RegisterInterruptHandler =
    extern "efiapi" fn(*const Protocol, EfiExceptionType, InterruptHandler) -> efi::Status;

/// Returns a timer value from one of the processor's internal timers.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.8
pub type GetTimerValue = extern "efiapi" fn(*const Protocol, u32, *mut u64, *mut u64) -> efi::Status;

/// Change a memory region to support specified memory attributes.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.9
pub type SetMemoryAttributes = extern "efiapi" fn(*const Protocol, efi::PhysicalAddress, u64, u64) -> efi::Status;

/// Abstracts the processor services that are required to implement some of the DXE services.
///
/// # Documentation
/// UEFI Platform Initialization Specification, Release 1.8, Section II-12.3.1
#[repr(C)]
pub struct Protocol {
    pub flush_data_cache: FlushDataCache,
    pub enable_interrupt: EnableInterrupt,
    pub disable_interrupt: DisableInterrupt,
    pub get_interrupt_state: GetInterruptState,
    pub init: Init,
    pub register_interrupt_handler: RegisterInterruptHandler,
    pub get_timer_value: GetTimerValue,
    pub set_memory_attributes: SetMemoryAttributes,
    /// The number of timers that are available in a processor. The value in this field is a constant that must not be
    /// modified after the CPU Architectural Protocol is installed. All consumers must treat this as a read-only field.
    pub number_of_timers: u32,
    /// The size, in bytes, of the alignment required for DMA buffer allocations. This is typically the size of the
    /// largest data cache line in the platform. This value can be determined by looking at the data cache line sizes of
    /// all the caches present in the platform, and returning the largest. This is used by the root bridge I/O abstraction
    /// protocols to guarantee that no two DMA buffers ever share the same cache line. The value in this field is a
    /// constant that must not be modified after the CPU Architectural Protocol is installed. All consumers must treat
    /// this as a read-only field.
    pub dma_buffer_alignment: u32,
}
