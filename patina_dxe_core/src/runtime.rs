//! DXE Core Runtime Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    ffi::c_void,
    mem, ptr,
    sync::atomic::{AtomicBool, Ordering},
};
use mu_pi::{list_entry, protocols::runtime};
use patina_sdk::base::UEFI_PAGE_SIZE;
use r_efi::efi;
use spin::Mutex;

use crate::{
    allocator::core_allocate_pool, events::EVENT_DB, image::core_relocate_runtime_images,
    protocols::core_install_protocol_interface, systemtables::SYSTEM_TABLE,
};

struct RuntimeData {
    runtime_arch_ptr: *mut runtime::Protocol,
    virtual_map: *mut efi::MemoryDescriptor,
    virtual_map_desc_size: usize,
    virtual_map_index: usize,
}

impl RuntimeData {
    const fn new() -> Self {
        Self {
            runtime_arch_ptr: ptr::null_mut(),
            virtual_map: ptr::null_mut(),
            virtual_map_desc_size: 0,
            virtual_map_index: 0,
        }
    }
}

unsafe impl Sync for RuntimeData {}
unsafe impl Send for RuntimeData {}

static RUNTIME_DATA: Mutex<RuntimeData> = Mutex::new(RuntimeData::new());

pub extern "efiapi" fn set_virtual_address_map(
    memory_map_size: usize,
    descriptor_size: usize,
    descriptor_version: u32,
    virtual_map: *mut efi::MemoryDescriptor,
) -> efi::Status {
    //
    // Can only switch to virtual addresses once the memory map is locked down,
    // and can only set it once
    //
    {
        let mut runtime_data = RUNTIME_DATA.lock();
        unsafe {
            let rt_arch_protocol = &*runtime_data.runtime_arch_ptr;

            if !rt_arch_protocol.at_runtime.load(Ordering::SeqCst)
                || rt_arch_protocol.virtual_mode.load(Ordering::SeqCst)
            {
                return efi::Status::UNSUPPORTED;
            }
        }

        if descriptor_version != efi::MEMORY_DESCRIPTOR_VERSION
            || descriptor_size < mem::size_of::<efi::MemoryDescriptor>()
        {
            return efi::Status::UNSUPPORTED;
        }

        unsafe { (*runtime_data.runtime_arch_ptr).virtual_mode.store(true, Ordering::SeqCst) };
        runtime_data.virtual_map_desc_size = descriptor_size;
        runtime_data.virtual_map_index = memory_map_size / descriptor_size;
        runtime_data.virtual_map = virtual_map;
    }

    // TODO: Add status code reporting (need to check runtime eligibility)

    // Signal EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE events (externally registered events)
    EVENT_DB.signal_group(efi::EVENT_GROUP_VIRTUAL_ADDRESS_CHANGE);

    // Convert runtime images
    core_relocate_runtime_images();

    // Convert runtime services pointers
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().get_time
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().set_time
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().get_wakeup_time
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().set_wakeup_time
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().reset_system
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE
                .lock()
                .as_mut()
                .expect("Invalid system table.")
                .runtime_services_mut()
                .get_next_high_mono_count
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().get_variable
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().set_variable
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().get_next_variable_name
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().query_variable_info
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").runtime_services_mut().update_capsule
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE
                .lock()
                .as_mut()
                .expect("Invalid system table.")
                .runtime_services_mut()
                .query_capsule_capabilities
        ) as *mut *mut c_void,
    );
    SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").checksum_runtime_services();

    // Convert system table runtime fields
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").system_table_mut().firmware_vendor
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").system_table_mut().configuration_table
        ) as *mut *mut c_void,
    );
    convert_pointer(
        0,
        core::ptr::addr_of_mut!(
            SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").system_table_mut().runtime_services
        ) as *mut *mut c_void,
    );
    SYSTEM_TABLE.lock().as_mut().expect("Invalid system table.").checksum();

    {
        let mut runtime_data = RUNTIME_DATA.lock();
        runtime_data.virtual_map = ptr::null_mut();
        runtime_data.virtual_map_index = 0;
    }

    efi::Status::SUCCESS
}

pub extern "efiapi" fn convert_pointer(debug_disposition: usize, convert_address: *mut *mut c_void) -> efi::Status {
    if convert_address.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let address = unsafe { *convert_address as usize };

    if address == 0 {
        if debug_disposition & efi::OPTIONAL_POINTER as usize != 0 {
            return efi::Status::SUCCESS;
        }
        return efi::Status::INVALID_PARAMETER;
    }

    let runtime_data = RUNTIME_DATA.lock();
    if !runtime_data.virtual_map.is_null() {
        let map_index = runtime_data.virtual_map_index;
        let map = runtime_data.virtual_map;

        for i in 0..map_index {
            let desc = unsafe { &*(map as *const efi::MemoryDescriptor).add(i) };
            assert!(
                ((desc.number_of_pages as usize) < 0xffffffff) || (mem::size_of::<usize>() > 4),
                "Memory descriptor page count overflow"
            );

            if (desc.attribute & efi::MEMORY_RUNTIME) == efi::MEMORY_RUNTIME && address as u64 >= desc.physical_start {
                let virt_end_of_range = desc
                    .physical_start
                    .checked_add(desc.number_of_pages * UEFI_PAGE_SIZE as u64)
                    .expect("Virtual address exceeds expected range");

                if (address as u64) < virt_end_of_range {
                    unsafe {
                        convert_address.write(
                            (address - (desc.physical_start as usize) + (desc.virtual_start as usize)) as *mut c_void,
                        )
                    };
                    return efi::Status::SUCCESS;
                }
            }
        }
    }
    efi::Status::NOT_FOUND
}

pub fn init_runtime_support(rt: &mut efi::RuntimeServices) {
    rt.convert_pointer = convert_pointer;
    rt.set_virtual_address_map = set_virtual_address_map;

    match core_allocate_pool(efi::RUNTIME_SERVICES_DATA, mem::size_of::<runtime::Protocol>()) {
        Err(err) => panic!("Failed to allocate the Runtime Architecture Protocol: {err:?}"),
        Ok(allocation) => unsafe {
            let allocation_ptr = allocation as *mut runtime::Protocol;

            let image_head_ptr = ptr::addr_of_mut!(allocation_ptr.as_mut().unwrap().image_head);
            let event_head_ptr = ptr::addr_of_mut!(allocation_ptr.as_mut().unwrap().event_head);

            allocation_ptr.write(runtime::Protocol {
                // The Rust usage of the protocol won't actually use image_head or event_head,
                // so pass empty linked lists (just heads that point to themselves).
                image_head: list_entry::Entry { forward_link: image_head_ptr, back_link: image_head_ptr },
                event_head: list_entry::Entry { forward_link: event_head_ptr, back_link: event_head_ptr },
                memory_descriptor_size: mem::size_of::<efi::MemoryDescriptor>(), // Should be 16-byte aligned
                memory_descriptor_version: efi::MEMORY_DESCRIPTOR_VERSION,
                memory_map_size: 0,
                memory_map_physical: ptr::null_mut(),
                memory_map_virtual: ptr::null_mut(),
                virtual_mode: AtomicBool::new(false),
                at_runtime: AtomicBool::new(false),
            });
            RUNTIME_DATA.lock().runtime_arch_ptr = allocation_ptr;
            // Install the protocol on a new handle
            core_install_protocol_interface(None, runtime::PROTOCOL_GUID, allocation)
                .expect("Failed to install the Runtime Architecture protocol");
        },
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::{RUNTIME_DATA, convert_pointer, init_runtime_support, set_virtual_address_map};
    use crate::test_support;
    use core::{ffi::c_void, mem};
    use r_efi::efi;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                test_support::init_test_gcd(None);
                test_support::reset_allocators();
                test_support::init_test_protocol_db();
            }
            f();
        })
        .unwrap();
    }

    fn fake_runtime_services() -> efi::RuntimeServices {
        let runtime_services = mem::MaybeUninit::zeroed();
        let mut runtime_services: efi::RuntimeServices = unsafe { runtime_services.assume_init() };
        runtime_services.hdr.signature = efi::RUNTIME_SERVICES_SIGNATURE;
        runtime_services.hdr.revision = efi::RUNTIME_SERVICES_REVISION;
        runtime_services.hdr.header_size = mem::size_of::<efi::RuntimeServices>() as u32;
        runtime_services
    }

    unsafe fn get_memory(size: usize) -> &'static mut [u8] {
        let addr = unsafe { alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size, 0x1000).unwrap()) };
        unsafe { core::slice::from_raw_parts_mut(addr, size) }
    }

    #[test]
    fn init_should_initialize_convert_pointer_and_set_virtual_address_map() {
        with_locked_state(|| {
            let mut rt = fake_runtime_services();

            init_runtime_support(&mut rt);

            assert_eq!(rt.convert_pointer as usize, convert_pointer as usize);
            assert_eq!(rt.set_virtual_address_map as usize, set_virtual_address_map as usize);
        });
    }

    #[test]
    fn test_convert_pointer() {
        with_locked_state(|| {
            let mut rt = fake_runtime_services();

            init_runtime_support(&mut rt);

            let address_ptr = unsafe { get_memory(0x1000).as_ptr() as *mut c_void };
            unsafe { (address_ptr as *mut usize).write(0x1000) };
            let mut desc = efi::MemoryDescriptor {
                r#type: efi::RUNTIME_SERVICES_DATA,
                physical_start: 0x1000,
                virtual_start: 0x2000,
                number_of_pages: 1,
                attribute: efi::MEMORY_RUNTIME | efi::MEMORY_WB,
            };

            {
                let mut runtime_data = RUNTIME_DATA.lock();
                runtime_data.virtual_map = &mut desc;
                runtime_data.virtual_map_index = 1;
            }

            // let convert_address = &mut address as *mut _ as *mut *mut c_void;
            unsafe {
                assert_eq!(convert_pointer(0, address_ptr as *mut *mut c_void), efi::Status::SUCCESS);
                assert_eq!(*(address_ptr as *mut usize), 0x2000);

                (address_ptr as *mut usize).write(0x3000);
                assert_eq!(convert_pointer(0, address_ptr as *mut *mut c_void), efi::Status::NOT_FOUND);
                assert_eq!(*(address_ptr as *mut usize), 0x3000);

                (address_ptr as *mut usize).write(0);
                assert_eq!(convert_pointer(0, address_ptr as *mut *mut c_void), efi::Status::INVALID_PARAMETER);
                assert_eq!(*(address_ptr as *mut usize), 0);

                (address_ptr as *mut usize).write(0x1000);
                assert_eq!(
                    convert_pointer(efi::OPTIONAL_POINTER as usize, address_ptr as *mut *mut c_void),
                    efi::Status::SUCCESS
                );
                assert_eq!(*(address_ptr as *mut usize), 0x2000);

                (address_ptr as *mut usize).write(0);
                assert_eq!(
                    convert_pointer(efi::OPTIONAL_POINTER as usize, address_ptr as *mut *mut c_void),
                    efi::Status::SUCCESS
                );
                assert_eq!(*(address_ptr as *mut usize), 0);
            }
        });
    }
}
