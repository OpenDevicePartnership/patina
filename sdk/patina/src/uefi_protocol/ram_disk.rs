//! UEFI RAM Disk Protocol bindings.
//!
//! Used to install a contiguous range of system memory as a virtual block device the system
//! firmware can boot from.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use r_efi::efi;

use crate::uefi_protocol::ProtocolInterface;

/// Well-known `RamDiskType` GUID for a generic virtual disk.
pub const RAM_DISK_VIRTUAL_DISK_GUID: efi::Guid =
    efi::Guid::from_fields(0x77ab535a, 0x45fc, 0x624b, 0x55, 0x60, &[0xf7, 0xb2, 0x81, 0xd1, 0xf9, 0x6e]);

/// Well-known `RamDiskType` GUID for a virtual CD (ISO9660) image.
pub const RAM_DISK_VIRTUAL_CD_GUID: efi::Guid =
    efi::Guid::from_fields(0x3d5abd30, 0x4175, 0x87ce, 0x6d, 0x64, &[0xd2, 0xad, 0xe5, 0x23, 0xc4, 0xbb]);

/// Well-known `RamDiskType` GUID for a persistent virtual disk.
pub const RAM_DISK_PERSISTENT_VIRTUAL_DISK_GUID: efi::Guid =
    efi::Guid::from_fields(0x5cea02c9, 0x4d07, 0x69d3, 0x26, 0x9f, &[0x44, 0x96, 0xfb, 0xe0, 0x96, 0xf9]);

/// Well-known `RamDiskType` GUID for a persistent virtual CD.
pub const RAM_DISK_PERSISTENT_VIRTUAL_CD_GUID: efi::Guid =
    efi::Guid::from_fields(0x08018188, 0x42cd, 0xbb48, 0x10, 0x0f, &[0x53, 0x87, 0xd5, 0x3d, 0xed, 0x3d]);

/// FFI type for `EFI_RAM_DISK_REGISTER_RAMDISK`.
///
/// Installs a `[ram_disk_base, ram_disk_base + ram_disk_size)` memory range as a RAM disk of
/// type `ram_disk_type`. When `parent_device_path` is null, the protocol creates a virtual
/// device-path root; otherwise the new RAM disk is appended under the supplied path. On
/// success the resulting device path is written to `*device_path` (caller-owned, do not free
/// before calling `Unregister`).
pub type RegisterFn = extern "efiapi" fn(
    ram_disk_base: u64,
    ram_disk_size: u64,
    ram_disk_type: *mut efi::Guid,
    parent_device_path: *mut efi::protocols::device_path::Protocol,
    device_path: *mut *mut efi::protocols::device_path::Protocol,
) -> efi::Status;

/// FFI type for `EFI_RAM_DISK_UNREGISTER_RAMDISK`.
///
/// Removes a previously-installed RAM disk identified by the device path returned from
/// `Register`. The backing memory is **not** freed by this call — the caller owns its
/// lifetime.
pub type UnregisterFn = extern "efiapi" fn(device_path: *mut efi::protocols::device_path::Protocol) -> efi::Status;

/// FFI binding for `EFI_RAM_DISK_PROTOCOL`.
#[repr(C)]
pub struct Protocol {
    /// Installs a host memory range as a RAM disk and produces its device path.
    pub register: RegisterFn,
    /// Removes a previously-installed RAM disk identified by its device path.
    pub unregister: UnregisterFn,
}

// SAFETY: Layout matches the UEFI RAM Disk Protocol struct (two efiapi function pointers).
// PROTOCOL_GUID matches the UEFI-spec value (AB38A0DF-6873-44A9-87E6-D4EB56148449).
unsafe impl ProtocolInterface for Protocol {
    const PROTOCOL_GUID: crate::BinaryGuid = crate::BinaryGuid::from_string("AB38A0DF-6873-44A9-87E6-D4EB56148449");
}

/// Install `bytes` as a virtual block device using the UEFI RAM Disk Protocol.
///
/// Leaks `bytes` into a stable heap allocation, locates the RAM Disk protocol, and asks the
/// firmware to register the buffer as a virtual disk of type `RAM_DISK_VIRTUAL_DISK_GUID`. The
/// returned device path can be passed to consumers of UEFI boot device paths.
///
/// The backing memory lives for the rest of the firmware's lifetime — there is no `Drop` impl
/// that frees it. The caller may later invoke the protocol's `unregister` to remove the RAM
/// disk's device-path entry, but the memory itself stays leaked.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `bytes` - Bytes to publish as the virtual block device's contents
///
/// # Returns
///
/// Returns `Ok(DevicePathBuf)` containing an owned copy of the device path the firmware assigns
/// to the new RAM disk. Returns an error if the RAM Disk protocol is not present, or the
/// firmware rejects the register call.
#[cfg(feature = "unstable-device-path")]
pub fn install<B: crate::boot_services::BootServices>(
    boot_services: &B,
    bytes: &[u8],
) -> crate::error::Result<crate::device_path::paths::DevicePathBuf> {
    use alloc::boxed::Box;
    use core::ptr;

    use crate::{
        device_path::paths::{DevicePath, DevicePathBuf},
        error::EfiError,
    };

    let buffer: Box<[u8]> = bytes.into();
    let leaked: &'static mut [u8] = Box::leak(buffer);
    let base = leaked.as_ptr() as u64;
    let size = leaked.len() as u64;

    // SAFETY: locate_protocol with None returns a static mut reference for the registered
    // protocol; we treat it as &mut for the single register call below.
    let proto = unsafe { boot_services.locate_protocol::<Protocol>(None) }.map_err(EfiError::from)?;

    let mut type_guid = RAM_DISK_VIRTUAL_DISK_GUID;
    let mut device_path_out: *mut efi::protocols::device_path::Protocol = ptr::null_mut();

    let status = (proto.register)(base, size, &mut type_guid, ptr::null_mut(), &mut device_path_out);
    if status != efi::Status::SUCCESS {
        return Err(EfiError::from(status));
    }

    if device_path_out.is_null() {
        return Err(EfiError::DeviceError);
    }

    // SAFETY: device_path_out was just written by the firmware and is null-terminated per spec.
    let dp_ref =
        unsafe { DevicePath::try_from_ptr(device_path_out as *const u8) }.map_err(|_| EfiError::DeviceError)?;
    Ok(DevicePathBuf::from(dp_ref))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi_protocol::ProtocolInterface;

    #[test]
    fn protocol_guid_matches_uefi_spec() {
        // EFI_RAM_DISK_PROTOCOL_GUID per the UEFI specification.
        let expected =
            efi::Guid::from_fields(0xab38a0df, 0x6873, 0x44a9, 0x87, 0xe6, &[0xd4, 0xeb, 0x56, 0x14, 0x84, 0x49]);
        assert_eq!(
            *Protocol::PROTOCOL_GUID.0.as_bytes(),
            *expected.as_bytes(),
            "RAM Disk protocol GUID must match the UEFI specification"
        );
    }

    #[test]
    fn ram_disk_type_guids_are_distinct() {
        let virtual_disk = *RAM_DISK_VIRTUAL_DISK_GUID.as_bytes();
        let virtual_cd = *RAM_DISK_VIRTUAL_CD_GUID.as_bytes();
        let persistent = *RAM_DISK_PERSISTENT_VIRTUAL_DISK_GUID.as_bytes();
        let persistent_cd = *RAM_DISK_PERSISTENT_VIRTUAL_CD_GUID.as_bytes();

        assert_ne!(virtual_disk, virtual_cd);
        assert_ne!(virtual_disk, persistent);
        assert_ne!(virtual_disk, persistent_cd);
        assert_ne!(virtual_cd, persistent);
        assert_ne!(virtual_cd, persistent_cd);
        assert_ne!(persistent, persistent_cd);
    }

    #[cfg(feature = "unstable-device-path")]
    mod install_tests {
        use core::sync::atomic::{AtomicU64, Ordering};

        use alloc::boxed::Box;

        use super::*;
        use crate::boot_services::MockBootServices;

        static CAPTURED_REGISTER_BASE: AtomicU64 = AtomicU64::new(0);
        static CAPTURED_REGISTER_SIZE: AtomicU64 = AtomicU64::new(0);

        /// Static EndEntire-only device path bytes (type=0x7F, subtype=0xFF, length=4), used as
        /// the return value of mock register fns.
        static MOCK_REGISTER_DEVICE_PATH: [u8; 4] = [0x7F, 0xFF, 0x04, 0x00];

        extern "efiapi" fn mock_register_returns_path(
            ram_disk_base: u64,
            ram_disk_size: u64,
            _ram_disk_type: *mut efi::Guid,
            _parent_device_path: *mut efi::protocols::device_path::Protocol,
            device_path: *mut *mut efi::protocols::device_path::Protocol,
        ) -> efi::Status {
            CAPTURED_REGISTER_BASE.store(ram_disk_base, Ordering::SeqCst);
            CAPTURED_REGISTER_SIZE.store(ram_disk_size, Ordering::SeqCst);
            // SAFETY: caller-provided out-ptr is valid; static path lives forever.
            unsafe {
                *device_path = MOCK_REGISTER_DEVICE_PATH.as_ptr() as *mut efi::protocols::device_path::Protocol;
            }
            efi::Status::SUCCESS
        }

        extern "efiapi" fn mock_register_returns_error(
            _ram_disk_base: u64,
            _ram_disk_size: u64,
            _ram_disk_type: *mut efi::Guid,
            _parent_device_path: *mut efi::protocols::device_path::Protocol,
            _device_path: *mut *mut efi::protocols::device_path::Protocol,
        ) -> efi::Status {
            efi::Status::OUT_OF_RESOURCES
        }

        extern "efiapi" fn mock_unregister(_device_path: *mut efi::protocols::device_path::Protocol) -> efi::Status {
            efi::Status::SUCCESS
        }

        fn leaked_ram_disk_protocol(register: RegisterFn) -> &'static mut Protocol {
            Box::leak(Box::new(Protocol { register, unregister: mock_unregister }))
        }

        #[test]
        fn install_locate_failure() {
            let mut mock = MockBootServices::new();
            mock.expect_locate_protocol::<Protocol>().returning(|_| Err(efi::Status::NOT_FOUND));

            let result = install(&mock, &[0xAB; 16]);
            assert!(result.is_err(), "missing RAM Disk Protocol must surface as Err");
        }

        #[test]
        fn install_register_failure_propagates() {
            let mut mock = MockBootServices::new();
            mock.expect_locate_protocol::<Protocol>()
                .returning(|_| Ok(leaked_ram_disk_protocol(mock_register_returns_error)));

            let result = install(&mock, &[0xAB; 16]);
            assert!(result.is_err(), "register OUT_OF_RESOURCES must surface as Err");
        }

        #[test]
        fn install_forwards_base_and_size_to_register() {
            let mut mock = MockBootServices::new();
            mock.expect_locate_protocol::<Protocol>()
                .returning(|_| Ok(leaked_ram_disk_protocol(mock_register_returns_path)));

            let payload = [0xAB; 256];
            let _result = install(&mock, &payload).expect("register should succeed");

            assert!(CAPTURED_REGISTER_BASE.load(Ordering::SeqCst) != 0, "register received non-zero heap base");
            assert_eq!(CAPTURED_REGISTER_SIZE.load(Ordering::SeqCst), 256, "register received buffer length");
        }
    }
}
