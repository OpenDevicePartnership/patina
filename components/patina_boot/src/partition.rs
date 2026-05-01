//! Partition I/O helpers for boot orchestrators.
//!
//! Functions in this module operate on partitions identified by a `DevicePathBuf` and use the
//! UEFI protocols already published by the platform's storage stack. They are intended to be
//! called from platforms implementing custom boot flows.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::vec::Vec;
use core::ptr;

use patina::{
    boot_services::BootServices,
    device_path::paths::DevicePathBuf,
    error::{EfiError, Result},
};
use r_efi::{efi, protocols::block_io};

/// Write-protect the NVMe boot partition addressed by `device_path` until the next power cycle.
///
/// Resolves the controller handle by walking `device_path` for the NVMe Pass-Thru protocol,
/// then issues an NVMe Set Features admin command for FID 0x11 (Boot Partition Write Protection
/// Configuration). Both BP0 and BP1 are placed in "Write Protect Until Power Cycle" state (001b).
///
/// The lock is volatile: a controller reset or power cycle clears it.
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `device_path` - Device path resolving to (or descending from) an NVMe controller. The path
///   is consumed for protocol lookup only; partition or namespace nodes are tolerated.
///
/// # Returns
///
/// Returns `Ok(())` once the controller acknowledges the Set Features command. Returns an error
/// if no NVMe Pass-Thru protocol is reachable on the path, or if the controller rejects the
/// command.
pub fn lock_partition_write<B: BootServices>(boot_services: &B, device_path: &DevicePathBuf) -> Result<()> {
    let mut path_ptr = device_path.as_ref() as *const _ as *mut efi::protocols::device_path::Protocol;

    // SAFETY: path_ptr points into a valid DevicePathBuf for the duration of this call.
    let handle = unsafe { boot_services.locate_device_path(&nvme_pass_thru::PROTOCOL_GUID, &mut path_ptr) }
        .map_err(EfiError::from)?;

    // SAFETY: handle was returned by locate_device_path for the NVMe Pass-Thru GUID.
    let protocol = unsafe {
        boot_services.handle_protocol_unchecked(handle, &nvme_pass_thru::PROTOCOL_GUID).map_err(EfiError::from)?
    } as *mut nvme_pass_thru::Protocol;

    // SAFETY: protocol is a non-null, properly aligned pointer to a Protocol owned by the controller.
    unsafe { lock_partition_write_inner(protocol) }
}

/// Issue the NVMe Set Features admin command for BPWPS via the supplied Pass-Thru protocol.
///
/// Separated from `lock_partition_write` because it dereferences a raw protocol pointer and
/// invokes its function pointer directly. Tests use mock protocol function pointers to exercise
/// the dispatch path.
///
/// # Safety
///
/// `protocol` must be a valid, non-null pointer to an `EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL`
/// instance owned by an NVMe controller for the duration of this call.
unsafe fn lock_partition_write_inner(protocol: *mut nvme_pass_thru::Protocol) -> Result<()> {
    use nvme_pass_thru::{
        BPWPS_LOCK_BP0_BP1, CMD_FLAG_CDW10_VALID, CMD_FLAG_CDW11_VALID, Command, CommandPacket, Completion,
        FID_BOOT_PARTITION_WRITE_PROTECTION, OPCODE_SET_FEATURES, QUEUE_TYPE_ADMIN, TIMEOUT_NS_1_SEC,
    };

    let mut nvme_cmd = Command { cdw0: OPCODE_SET_FEATURES as u32, ..Command::zero() };
    nvme_cmd.flags = CMD_FLAG_CDW10_VALID | CMD_FLAG_CDW11_VALID;
    nvme_cmd.cdw10 = FID_BOOT_PARTITION_WRITE_PROTECTION as u32;
    nvme_cmd.cdw11 = BPWPS_LOCK_BP0_BP1;

    let mut completion = Completion::zero();

    let mut packet = CommandPacket {
        command_timeout: TIMEOUT_NS_1_SEC,
        transfer_buffer: ptr::null_mut(),
        transfer_length: 0,
        metadata_buffer: ptr::null_mut(),
        metadata_length: 0,
        queue_type: QUEUE_TYPE_ADMIN,
        nvme_cmd: &mut nvme_cmd,
        nvme_completion: &mut completion,
    };

    // SAFETY: caller guarantees `protocol` is valid; packet pointers are kept alive across the call.
    let pass_thru = unsafe { (*protocol).pass_thru };
    let status = pass_thru(protocol, 0, &mut packet, ptr::null_mut());
    if status != efi::Status::SUCCESS {
        return Err(EfiError::from(status));
    }

    // NVMe completion DW3 carries Status Field in bits 31:17. Non-zero indicates the controller
    // rejected the command (e.g., feature unsupported or BP already permanently locked).
    let status_field = (completion.dw3 >> 17) & 0x7FFF;
    if status_field != 0 {
        log::error!("NVMe Set Features BPWPS rejected: status field {:#x}", status_field);
        return Err(EfiError::from(efi::Status::DEVICE_ERROR));
    }

    Ok(())
}

/// Write `data` to the partition addressed by `device_path` starting at LBA 0.
///
/// Resolves the partition handle via `locate_device_path` against `EFI_BLOCK_IO_PROTOCOL`,
/// then issues `WriteBlocks` followed by `FlushBlocks`. If `data.len()` is not a multiple of
/// the media block size, the trailing partial block is zero-padded.
///
/// Empty `data` is a no-op (returns `Ok(())` without touching the device).
///
/// # Arguments
///
/// * `boot_services` - Boot services interface
/// * `device_path` - Device path resolving to a partition or block device exposing BlockIo
/// * `data` - Bytes to write at LBA 0; must fit within the partition (`data.len() <= (last_block + 1) * block_size`)
///
/// # Returns
///
/// Returns `Ok(())` once `WriteBlocks` and `FlushBlocks` both succeed. Returns an error if
/// the device path doesn't resolve to a BlockIo handle, the media is read-only or absent, or
/// either of the write/flush calls fails.
pub fn write_partition_raw<B: BootServices>(boot_services: &B, device_path: &DevicePathBuf, data: &[u8]) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let mut path_ptr = device_path.as_ref() as *const _ as *mut efi::protocols::device_path::Protocol;

    // SAFETY: path_ptr points into a valid DevicePathBuf for the duration of this call.
    let handle =
        unsafe { boot_services.locate_device_path(&block_io::PROTOCOL_GUID, &mut path_ptr) }.map_err(EfiError::from)?;

    // SAFETY: handle was returned by locate_device_path for the BlockIo GUID.
    let protocol = unsafe { boot_services.handle_protocol::<block_io::Protocol>(handle).map_err(EfiError::from)? };

    // SAFETY: `protocol` is a valid reference returned by handle_protocol; lifetime is bound by the controller.
    unsafe { write_partition_raw_inner(protocol, data) }
}

/// Issue `WriteBlocks` + `FlushBlocks` against the supplied BlockIo protocol.
///
/// Separated from `write_partition_raw` because it dereferences the protocol's `media` raw
/// pointer and invokes function pointers directly. Tests use mock function pointers to
/// exercise the dispatch path.
///
/// # Safety
///
/// `protocol.media` must point to a valid `Media` instance owned by the controller, and the
/// `write_blocks`/`flush_blocks` function pointers must remain valid for the duration of the call.
unsafe fn write_partition_raw_inner(protocol: &mut block_io::Protocol, data: &[u8]) -> Result<()> {
    // SAFETY: protocol.media is set by the controller when BlockIo is published; non-null per spec.
    let media = unsafe { &*protocol.media };

    if !media.media_present {
        return Err(EfiError::from(efi::Status::NO_MEDIA));
    }
    if media.read_only {
        return Err(EfiError::from(efi::Status::WRITE_PROTECTED));
    }

    let block_size = media.block_size as usize;
    if block_size == 0 {
        return Err(EfiError::from(efi::Status::DEVICE_ERROR));
    }

    let aligned_size = data.len().div_ceil(block_size) * block_size;
    let mut buffer = Vec::with_capacity(aligned_size);
    buffer.extend_from_slice(data);
    buffer.resize(aligned_size, 0);

    let media_id = media.media_id;
    let write_blocks = protocol.write_blocks;
    let flush_blocks = protocol.flush_blocks;

    let status = write_blocks(protocol, media_id, 0, aligned_size, buffer.as_mut_ptr() as *mut core::ffi::c_void);
    if status != efi::Status::SUCCESS {
        return Err(EfiError::from(status));
    }

    let status = flush_blocks(protocol);
    if status != efi::Status::SUCCESS {
        return Err(EfiError::from(status));
    }

    Ok(())
}

/// Minimal FFI bindings for `EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL`.
mod nvme_pass_thru {
    use core::ffi::c_void;
    use r_efi::efi;

    pub const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x52c78312, 0x8edc, 0x4233, 0x98, 0xf2, &[0x1a, 0x1a, 0xa5, 0xe3, 0x88, 0xa5]);

    pub const OPCODE_SET_FEATURES: u8 = 0x09;
    pub const FID_BOOT_PARTITION_WRITE_PROTECTION: u8 = 0x11;

    pub const CMD_FLAG_CDW10_VALID: u8 = 1 << 2;
    pub const CMD_FLAG_CDW11_VALID: u8 = 1 << 3;

    pub const QUEUE_TYPE_ADMIN: u8 = 0;

    /// 1-second timeout in 100-ns units, per the protocol contract.
    pub const TIMEOUT_NS_1_SEC: u64 = 10_000_000;

    /// CDW11 value placing both BP0 and BP1 in "Write Protect Until Power Cycle" (state 001b).
    /// Layout: BP0WPS in bits 2:0, BP1WPS in bits 5:3.
    pub const BPWPS_LOCK_BP0_BP1: u32 = (0b001 << 3) | 0b001;

    pub type PassThruFn = extern "efiapi" fn(
        this: *mut Protocol,
        namespace_id: u32,
        packet: *mut CommandPacket,
        event: *mut c_void,
    ) -> efi::Status;

    #[repr(C)]
    pub struct Protocol {
        pub mode: *mut c_void,
        pub pass_thru: PassThruFn,
        pub get_next_namespace: *mut c_void,
        pub build_device_path: *mut c_void,
        pub get_namespace: *mut c_void,
    }

    #[repr(C)]
    pub struct CommandPacket {
        pub command_timeout: u64,
        pub transfer_buffer: *mut c_void,
        pub transfer_length: u32,
        pub metadata_buffer: *mut c_void,
        pub metadata_length: u32,
        pub queue_type: u8,
        pub nvme_cmd: *mut Command,
        pub nvme_completion: *mut Completion,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct Command {
        pub cdw0: u32,
        pub flags: u8,
        pub nsid: u32,
        pub cdw2: u32,
        pub cdw3: u32,
        pub cdw10: u32,
        pub cdw11: u32,
        pub cdw12: u32,
        pub cdw13: u32,
        pub cdw14: u32,
        pub cdw15: u32,
    }

    impl Command {
        pub const fn zero() -> Self {
            Self {
                cdw0: 0,
                flags: 0,
                nsid: 0,
                cdw2: 0,
                cdw3: 0,
                cdw10: 0,
                cdw11: 0,
                cdw12: 0,
                cdw13: 0,
                cdw14: 0,
                cdw15: 0,
            }
        }
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct Completion {
        pub dw0: u32,
        pub dw1: u32,
        pub dw2: u32,
        pub dw3: u32,
    }

    impl Completion {
        pub const fn zero() -> Self {
            Self { dw0: 0, dw1: 0, dw2: 0, dw3: 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use patina::{
        boot_services::MockBootServices,
        device_path::{node_defs::EndEntire, paths::DevicePathBuf},
    };

    fn create_test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    #[test]
    fn test_lock_partition_write_locate_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        mock.expect_locate_device_path().returning(|_, _| Err(efi::Status::NOT_FOUND));

        let result = lock_partition_write(&mock, &device_path);
        assert!(result.is_err(), "missing NVMe Pass-Thru on path must surface as Err");
    }

    #[test]
    fn test_lock_partition_write_handle_protocol_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        let handle_addr: usize = 1;
        mock.expect_locate_device_path().returning(move |_, _| Ok(handle_addr as efi::Handle));
        mock.expect_handle_protocol_unchecked().returning(|_, _| Err(efi::Status::UNSUPPORTED));

        let result = lock_partition_write(&mock, &device_path);
        assert!(result.is_err(), "unsupported protocol on located handle must surface as Err");
    }

    #[test]
    fn test_lock_partition_write_set_features_payload() {
        use super::nvme_pass_thru::{
            BPWPS_LOCK_BP0_BP1, FID_BOOT_PARTITION_WRITE_PROTECTION, OPCODE_SET_FEATURES, QUEUE_TYPE_ADMIN,
        };

        assert_eq!(OPCODE_SET_FEATURES, 0x09, "Set Features admin opcode");
        assert_eq!(FID_BOOT_PARTITION_WRITE_PROTECTION, 0x11, "BPWPS feature identifier");
        assert_eq!(QUEUE_TYPE_ADMIN, 0, "admin queue selector");
        assert_eq!(BPWPS_LOCK_BP0_BP1, 0x09, "CDW11 must encode BP0WPS=001b in bits 2:0 and BP1WPS=001b in bits 5:3");
    }

    // Tests for lock_partition_write_inner — exercise the unsafe FFI dispatch path with mock
    // protocol function pointers, mirroring the detect_hotkey_from_handles test pattern.

    static CAPTURED_PASS_THRU_OPCODE: AtomicUsize = AtomicUsize::new(0);
    static CAPTURED_PASS_THRU_FLAGS: AtomicUsize = AtomicUsize::new(0);
    static CAPTURED_PASS_THRU_CDW10: AtomicUsize = AtomicUsize::new(0);
    static CAPTURED_PASS_THRU_CDW11: AtomicUsize = AtomicUsize::new(0);
    static CAPTURED_PASS_THRU_QUEUE_TYPE: AtomicUsize = AtomicUsize::new(0);

    extern "efiapi" fn mock_pass_thru_capture_success(
        _this: *mut nvme_pass_thru::Protocol,
        _namespace_id: u32,
        packet: *mut nvme_pass_thru::CommandPacket,
        _event: *mut core::ffi::c_void,
    ) -> efi::Status {
        // SAFETY: caller (helper) constructs a valid CommandPacket whose nvme_cmd points to a
        // valid Command. Test storage of captured values is single-threaded.
        unsafe {
            let pkt = &*packet;
            let cmd = &*pkt.nvme_cmd;
            CAPTURED_PASS_THRU_OPCODE.store((cmd.cdw0 & 0xFF) as usize, Ordering::SeqCst);
            CAPTURED_PASS_THRU_FLAGS.store(cmd.flags as usize, Ordering::SeqCst);
            CAPTURED_PASS_THRU_CDW10.store(cmd.cdw10 as usize, Ordering::SeqCst);
            CAPTURED_PASS_THRU_CDW11.store(cmd.cdw11 as usize, Ordering::SeqCst);
            CAPTURED_PASS_THRU_QUEUE_TYPE.store(pkt.queue_type as usize, Ordering::SeqCst);
        }
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_pass_thru_returns_error(
        _this: *mut nvme_pass_thru::Protocol,
        _namespace_id: u32,
        _packet: *mut nvme_pass_thru::CommandPacket,
        _event: *mut core::ffi::c_void,
    ) -> efi::Status {
        efi::Status::DEVICE_ERROR
    }

    extern "efiapi" fn mock_pass_thru_nonzero_completion_status(
        _this: *mut nvme_pass_thru::Protocol,
        _namespace_id: u32,
        packet: *mut nvme_pass_thru::CommandPacket,
        _event: *mut core::ffi::c_void,
    ) -> efi::Status {
        // SAFETY: caller-provided pointers are valid for the lifetime of the call.
        unsafe {
            let pkt = &*packet;
            (*pkt.nvme_completion).dw3 = 0x2 << 17; // Status Field = 0x2 (Invalid Field in Command)
        }
        efi::Status::SUCCESS
    }

    #[test]
    fn test_lock_partition_write_inner_issues_correct_set_features_command() {
        let mut protocol = nvme_pass_thru::Protocol {
            mode: ptr::null_mut(),
            pass_thru: mock_pass_thru_capture_success,
            get_next_namespace: ptr::null_mut(),
            build_device_path: ptr::null_mut(),
            get_namespace: ptr::null_mut(),
        };

        // SAFETY: protocol is a valid Protocol kept alive on the test stack.
        let result = unsafe { lock_partition_write_inner(&mut protocol) };

        assert!(result.is_ok(), "successful pass-thru must produce Ok");
        assert_eq!(CAPTURED_PASS_THRU_OPCODE.load(Ordering::SeqCst), 0x09, "Set Features opcode");
        assert_eq!(CAPTURED_PASS_THRU_FLAGS.load(Ordering::SeqCst), 0b0000_1100, "CDW10 + CDW11 marked valid");
        assert_eq!(CAPTURED_PASS_THRU_CDW10.load(Ordering::SeqCst), 0x11, "FID = BPWPS");
        assert_eq!(CAPTURED_PASS_THRU_CDW11.load(Ordering::SeqCst), 0x09, "BP0WPS=001b, BP1WPS=001b");
        assert_eq!(CAPTURED_PASS_THRU_QUEUE_TYPE.load(Ordering::SeqCst), 0, "admin queue");
    }

    #[test]
    fn test_lock_partition_write_inner_passthru_failure_propagates() {
        let mut protocol = nvme_pass_thru::Protocol {
            mode: ptr::null_mut(),
            pass_thru: mock_pass_thru_returns_error,
            get_next_namespace: ptr::null_mut(),
            build_device_path: ptr::null_mut(),
            get_namespace: ptr::null_mut(),
        };

        // SAFETY: protocol is a valid Protocol kept alive on the test stack.
        let result = unsafe { lock_partition_write_inner(&mut protocol) };
        assert!(result.is_err(), "pass-thru DEVICE_ERROR must surface as Err");
    }

    #[test]
    fn test_lock_partition_write_inner_nonzero_completion_status_rejected() {
        let mut protocol = nvme_pass_thru::Protocol {
            mode: ptr::null_mut(),
            pass_thru: mock_pass_thru_nonzero_completion_status,
            get_next_namespace: ptr::null_mut(),
            build_device_path: ptr::null_mut(),
            get_namespace: ptr::null_mut(),
        };

        // SAFETY: protocol is a valid Protocol kept alive on the test stack.
        let result = unsafe { lock_partition_write_inner(&mut protocol) };
        assert!(result.is_err(), "non-zero completion status field must surface as Err");
    }

    // Tests for write_partition_raw

    #[test]
    fn test_write_partition_raw_empty_data_is_noop() {
        let device_path = create_test_device_path();
        // No mock expectations: function must short-circuit before touching boot_services.
        let mock = MockBootServices::new();

        let result = write_partition_raw(&mock, &device_path, &[]);
        assert!(result.is_ok(), "empty data must be a no-op");
    }

    #[test]
    fn test_write_partition_raw_locate_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        mock.expect_locate_device_path().returning(|_, _| Err(efi::Status::NOT_FOUND));

        let result = write_partition_raw(&mock, &device_path, &[0xAB; 16]);
        assert!(result.is_err(), "missing BlockIo on path must surface as Err");
    }

    #[test]
    fn test_write_partition_raw_handle_protocol_failure() {
        let device_path = create_test_device_path();
        let mut mock = MockBootServices::new();

        let handle_addr: usize = 1;
        mock.expect_locate_device_path().returning(move |_, _| Ok(handle_addr as efi::Handle));
        mock.expect_handle_protocol::<block_io::Protocol>().returning(|_| Err(efi::Status::UNSUPPORTED));

        let result = write_partition_raw(&mock, &device_path, &[0xAB; 16]);
        assert!(result.is_err(), "missing BlockIo interface on located handle must surface as Err");
    }

    // Tests for write_partition_raw_inner — exercise the unsafe FFI dispatch path with mock
    // BlockIo function pointers, mirroring the detect_hotkey_from_handles test pattern.

    static CAPTURED_WRITE_MEDIA_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static CAPTURED_WRITE_LBA: AtomicU64 = AtomicU64::new(0xFFFF_FFFF_FFFF_FFFF);
    static CAPTURED_WRITE_SIZE: AtomicUsize = AtomicUsize::new(0);
    static FLUSH_BLOCKS_INVOKED: AtomicBool = AtomicBool::new(false);

    fn make_test_media(read_only: bool, present: bool) -> block_io::Media {
        block_io::Media {
            media_id: 0xCAFEBABE,
            removable_media: false,
            media_present: present,
            logical_partition: false,
            read_only,
            write_caching: false,
            block_size: 512,
            io_align: 0,
            last_block: 1024,
            lowest_aligned_lba: 0,
            logical_blocks_per_physical_block: 1,
            optimal_transfer_length_granularity: 0,
        }
    }

    extern "efiapi" fn mock_block_io_reset(_this: *mut block_io::Protocol, _ext: efi::Boolean) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_block_io_read_blocks(
        _this: *mut block_io::Protocol,
        _media_id: u32,
        _lba: efi::Lba,
        _buffer_size: usize,
        _buffer: *mut core::ffi::c_void,
    ) -> efi::Status {
        efi::Status::UNSUPPORTED
    }

    extern "efiapi" fn mock_write_blocks_capture(
        _this: *mut block_io::Protocol,
        media_id: u32,
        lba: efi::Lba,
        buffer_size: usize,
        _buffer: *mut core::ffi::c_void,
    ) -> efi::Status {
        CAPTURED_WRITE_MEDIA_ID.store(media_id, Ordering::SeqCst);
        CAPTURED_WRITE_LBA.store(lba, Ordering::SeqCst);
        CAPTURED_WRITE_SIZE.store(buffer_size, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_write_blocks_returns_error(
        _this: *mut block_io::Protocol,
        _media_id: u32,
        _lba: efi::Lba,
        _buffer_size: usize,
        _buffer: *mut core::ffi::c_void,
    ) -> efi::Status {
        efi::Status::DEVICE_ERROR
    }

    extern "efiapi" fn mock_flush_blocks_success(_this: *mut block_io::Protocol) -> efi::Status {
        FLUSH_BLOCKS_INVOKED.store(true, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    fn make_test_protocol(
        media: *const block_io::Media,
        write_blocks: block_io::ProtocolWriteBlocks,
    ) -> block_io::Protocol {
        block_io::Protocol {
            revision: block_io::REVISION,
            media,
            reset: mock_block_io_reset,
            read_blocks: mock_block_io_read_blocks,
            write_blocks,
            flush_blocks: mock_flush_blocks_success,
        }
    }

    #[test]
    fn test_write_partition_raw_inner_writes_to_lba_zero_with_aligned_size() {
        FLUSH_BLOCKS_INVOKED.store(false, Ordering::SeqCst);
        let media = make_test_media(false, true);
        let mut protocol = make_test_protocol(&media, mock_write_blocks_capture);

        // Data size 600 should round up to 1024 (two 512-byte blocks).
        let data = [0xAB; 600];
        // SAFETY: protocol and media are valid for the lifetime of the call.
        let result = unsafe { write_partition_raw_inner(&mut protocol, &data) };

        assert!(result.is_ok(), "successful write+flush must produce Ok");
        assert_eq!(CAPTURED_WRITE_MEDIA_ID.load(Ordering::SeqCst), 0xCAFEBABE, "media_id forwarded");
        assert_eq!(CAPTURED_WRITE_LBA.load(Ordering::SeqCst), 0, "writes start at LBA 0");
        assert_eq!(CAPTURED_WRITE_SIZE.load(Ordering::SeqCst), 1024, "600 bytes round up to 2 blocks of 512");
        assert!(FLUSH_BLOCKS_INVOKED.load(Ordering::SeqCst), "FlushBlocks must run after WriteBlocks");
    }

    #[test]
    fn test_write_partition_raw_inner_read_only_rejected() {
        let media = make_test_media(true, true);
        let mut protocol = make_test_protocol(&media, mock_write_blocks_capture);

        // SAFETY: protocol and media are valid for the lifetime of the call.
        let result = unsafe { write_partition_raw_inner(&mut protocol, &[0xAB; 16]) };
        assert!(result.is_err(), "read-only media must surface as Err before WriteBlocks runs");
    }

    #[test]
    fn test_write_partition_raw_inner_writeblocks_failure_propagates() {
        let media = make_test_media(false, true);
        let mut protocol = make_test_protocol(&media, mock_write_blocks_returns_error);

        // SAFETY: protocol and media are valid for the lifetime of the call.
        let result = unsafe { write_partition_raw_inner(&mut protocol, &[0xAB; 16]) };
        assert!(result.is_err(), "WriteBlocks DEVICE_ERROR must surface as Err");
    }
}
