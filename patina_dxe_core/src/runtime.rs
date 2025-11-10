//! DXE Core Runtime Support
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{ffi::c_void, ptr};

use alloc::collections::LinkedList;
use patina::error::EfiError;
use r_efi::efi;
use spin::Mutex;

use crate::{events::EVENT_DB, pecoff::relocation::RelocationBlock, protocols::PROTOCOL_DB};
use patina::pi::{list_entry, protocols::runtime};

struct RuntimeData {
    runtime_arch_ptr: *mut runtime::Protocol,
    runtime_images: LinkedList<runtime::ImageEntry, &'static crate::allocator::UefiAllocator>,
    runtime_events: LinkedList<runtime::EventEntry, &'static crate::allocator::UefiAllocator>,
}

unsafe impl Sync for RuntimeData {}
unsafe impl Send for RuntimeData {}

static RUNTIME_DATA: Mutex<RuntimeData> = Mutex::new(RuntimeData::new());

impl RuntimeData {
    const fn new() -> Self {
        Self {
            runtime_arch_ptr: ptr::null_mut(),
            runtime_images: LinkedList::new_in(&crate::allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR),
            runtime_events: LinkedList::new_in(&crate::allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR),
        }
    }

    fn update_protocol_lists(&mut self) {
        if self.runtime_arch_ptr.is_null() {
            return;
        }

        // SAFETY: The protocol is identified by it's GUID and should be a valid
        //         pointer to a EFI_RUNTIME_ARCH_PROTOCOL structure.
        unsafe {
            // Update the image links
            let mut prev = &mut (*self.runtime_arch_ptr).image_head;
            for entry in self.runtime_images.iter_mut() {
                prev.forward_link = (&mut entry.link) as *mut _;
                entry.link.back_link = prev as *mut _;
                prev = &mut entry.link;
            }
            prev.forward_link = &mut (*self.runtime_arch_ptr).image_head as *mut _;
            (*self.runtime_arch_ptr).image_head.back_link = prev as *mut _;

            // Update the event links
            let mut prev = &mut (*self.runtime_arch_ptr).event_head;
            for entry in self.runtime_events.iter_mut() {
                prev.forward_link = (&mut entry.link) as *mut _;
                entry.link.back_link = prev as *mut _;
                prev = &mut entry.link;
            }
            prev.forward_link = &mut (*self.runtime_arch_ptr).event_head as *mut _;
            (*self.runtime_arch_ptr).event_head.back_link = prev as *mut _;
        }
    }
}

pub fn init_runtime_support(_rt: &mut efi::RuntimeServices) {
    // Setup a event callback for the runtime protocol.
    let event = EVENT_DB
        .create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(runtime_protocol_notify), None, None)
        .expect("Failed to create runtime protocol installation callback.");

    PROTOCOL_DB
        .register_protocol_notify(runtime::PROTOCOL_GUID, event)
        .expect("Failed to register protocol notify on runtime protocol.");
}

pub fn finalize_runtime_support() {
    let data = RUNTIME_DATA.lock();
    if !data.runtime_arch_ptr.is_null() {
        unsafe { (*data.runtime_arch_ptr).at_runtime.store(true, core::sync::atomic::Ordering::Relaxed) };
    }
}

extern "efiapi" fn runtime_protocol_notify(_event: efi::Event, _context: *mut c_void) {
    log::info!("Runtime protocol installed. Setting up pointers.");
    let ptr = PROTOCOL_DB.locate_protocol(runtime::PROTOCOL_GUID).expect("Failed to locate runtime protocol.");
    let mut data = RUNTIME_DATA.lock();
    data.runtime_arch_ptr = ptr as *mut runtime::Protocol;
    data.update_protocol_lists();
}

pub fn add_runtime_event(
    event: efi::Event,
    event_type: u32,
    notify_tpl: efi::Tpl,
    notify_fn: Option<efi::EventNotify>,
    event_group: Option<efi::Guid>,
    context: Option<*mut c_void>,
) -> Result<(), EfiError> {
    let mut data = RUNTIME_DATA.lock();

    // The event modules will separate out the event group from the event type for consistency,
    // but the runtime architectural protocol expects them to be combined. Merge them here.
    let event_type = match event_group {
        Some(efi::EVENT_GROUP_VIRTUAL_ADDRESS_CHANGE) => efi::EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
        _ => event_type,
    };

    let function = notify_fn.ok_or(EfiError::InvalidParameter)?;
    data.runtime_events.push_back(runtime::EventEntry {
        event_type,
        notify_tpl,
        notify_function: function,
        context: context.unwrap_or(ptr::null_mut()),
        event,
        link: list_entry::Entry { forward_link: ptr::null_mut(), back_link: ptr::null_mut() },
    });

    data.update_protocol_lists();
    Ok(())
}

pub fn remove_runtime_event(event: efi::Event) -> Result<(), EfiError> {
    let mut data = RUNTIME_DATA.lock();
    for _ in data.runtime_events.extract_if(|entry| entry.event == event) {}
    data.update_protocol_lists();
    Ok(())
}

pub fn add_runtime_image(
    image_base: *mut c_void,
    image_size: u64,
    relocation_data: &[RelocationBlock],
    handle: efi::Handle,
) -> Result<(), EfiError> {
    let mut data = RUNTIME_DATA.lock();

    let relocation_data = crate::pecoff::flatten_runtime_relocation_data(relocation_data);
    data.runtime_images.push_back(runtime::ImageEntry {
        image_base,
        image_size,
        relocation_data: relocation_data.as_mut_ptr() as *mut _,
        handle,
        link: list_entry::Entry { forward_link: ptr::null_mut(), back_link: ptr::null_mut() },
    });

    data.update_protocol_lists();
    Ok(())
}

pub fn remove_runtime_image(image_handle: efi::Handle) -> Result<(), EfiError> {
    let mut data = RUNTIME_DATA.lock();
    for _ in data.runtime_images.extract_if(|entry| entry.handle == image_handle) {}
    data.update_protocol_lists();
    Ok(())
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;
    use core::{ptr, sync::atomic::AtomicBool};

    fn setup_protocol_and_data() -> RuntimeData {
        let protocol = runtime::Protocol {
            image_head: list_entry::Entry { forward_link: ptr::null_mut(), back_link: ptr::null_mut() },
            event_head: list_entry::Entry { forward_link: ptr::null_mut(), back_link: ptr::null_mut() },
            memory_descriptor_size: 0,
            memory_descriptor_version: 0,
            memory_map_size: 0,
            memory_map_physical: ptr::null_mut(),
            memory_map_virtual: ptr::null_mut(),
            virtual_mode: AtomicBool::new(false),
            at_runtime: AtomicBool::new(false),
        };
        let mut data = RuntimeData::new();
        data.runtime_arch_ptr = Box::leak(Box::new(protocol));
        data
    }

    extern "efiapi" fn dummy_notify(_event: efi::Event, _context: *mut core::ffi::c_void) {
        // Do nothing
    }

    fn new_image(handle: usize) -> runtime::ImageEntry {
        runtime::ImageEntry {
            image_base: ptr::null_mut(),
            image_size: 0,
            relocation_data: ptr::null_mut(),
            handle: handle as efi::Handle,
            link: list_entry::Entry { forward_link: ptr::null_mut(), back_link: ptr::null_mut() },
        }
    }

    fn new_event(event: usize) -> runtime::EventEntry {
        runtime::EventEntry {
            event_type: 0,
            notify_tpl: efi::TPL_APPLICATION,
            notify_function: dummy_notify,
            context: ptr::null_mut(),
            event: event as efi::Event,
            link: list_entry::Entry { forward_link: ptr::null_mut(), back_link: ptr::null_mut() },
        }
    }

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                crate::test_support::init_test_gcd(None);
                crate::test_support::init_test_protocol_db();
            }
            crate::test_support::reset_dispatcher_context();
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_image_list_consistency() {
        // Runtime tests require global synchronization due to shared static allocators
        // that use TPL locks, which cannot be acquired concurrently
        with_locked_state(|| {
            let mut data = setup_protocol_and_data();
            let link_offset = size_of::<u64>() * 4;

            // Add images
            for i in 0..10 {
                data.runtime_images.push_back(new_image(i));
            }
            data.update_protocol_lists();

            // SAFETY: Parsing a C-style linked list is inherently unsafe, but if the
            //         update_protocol_lists function is correct, this should be safe.
            unsafe {
                // Walk the linked list starting from the head and make sure all entries are present.
                let mut protocol_link = (*data.runtime_arch_ptr).image_head.forward_link;
                let mut count = 0;
                let mut prev = &*(&(*data.runtime_arch_ptr).image_head as *const _) as *const list_entry::Entry;
                while !core::ptr::eq(protocol_link, &mut (*data.runtime_arch_ptr).image_head as *mut _) {
                    let entry = ((protocol_link as *const u8).byte_sub(link_offset) as *const runtime::ImageEntry)
                        .as_ref()
                        .unwrap();
                    assert_eq!(entry.handle as usize, count);
                    assert_eq!(entry.link.back_link, prev as *mut _);
                    count += 1;
                    protocol_link = entry.link.forward_link;
                    prev = &entry.link as *const _;
                    assert!(count <= 10, "Too many entries in the image list.");
                }
                assert_eq!(count, 10, "Not all entries were found in the image list.");
            }

            // Remove all the odd images
            for i in (0..10).filter(|x| x % 2 == 1) {
                for _ in data.runtime_images.extract_if(|entry| entry.handle == i as efi::Handle) {}
            }
            data.update_protocol_lists();

            // SAFETY: Parsing a C-style linked list is inherently unsafe, but if the
            //         update_protocol_lists function is correct, this should be safe.
            unsafe {
                // Walk the linked list starting from the head and make sure all entries are present.
                let mut protocol_link = (*data.runtime_arch_ptr).image_head.forward_link;
                let mut count = 0;
                let mut prev = &*(&(*data.runtime_arch_ptr).image_head as *const _) as *const list_entry::Entry;
                while !core::ptr::eq(protocol_link, &mut (*data.runtime_arch_ptr).image_head as *mut _) {
                    let entry = ((protocol_link as *const u8).byte_sub(link_offset) as *const runtime::ImageEntry)
                        .as_ref()
                        .unwrap();
                    assert_eq!(entry.handle as usize, count * 2);
                    assert_eq!(entry.link.back_link, prev as *mut _);
                    count += 1;
                    protocol_link = entry.link.forward_link;
                    prev = &entry.link as *const _;
                    assert!(count <= 5, "Too many entries in the image list.");
                }
                assert_eq!(count, 5, "Not all entries were found in the image list.");
            }
        });
    }

    #[test]
    fn test_event_list_consistency() {
        // Runtime tests require global synchronization due to shared static allocators
        // that use TPL locks, which cannot be acquired concurrently
        with_locked_state(|| {
            let mut data = setup_protocol_and_data();
            let link_offset = size_of::<u64>() * 5;

            // Add events
            for i in 0..10 {
                data.runtime_events.push_back(new_event(i));
            }
            data.update_protocol_lists();

            // SAFETY: Parsing a C-style linked list is inherently unsafe, but if the
            //         update_protocol_lists function is correct, this should be safe.
            unsafe {
                // Walk the linked list starting from the head and make sure all entries are present.
                let mut protocol_link = (*data.runtime_arch_ptr).event_head.forward_link;
                let mut count = 0;
                let mut prev = &*(&(*data.runtime_arch_ptr).event_head as *const _) as *const list_entry::Entry;
                while !core::ptr::eq(protocol_link, &mut (*data.runtime_arch_ptr).event_head as *mut _) {
                    let entry = ((protocol_link as *const u8).byte_sub(link_offset) as *const runtime::EventEntry)
                        .as_ref()
                        .unwrap();
                    assert_eq!(entry.event as usize, count);
                    assert_eq!(entry.link.back_link, prev as *mut _);
                    count += 1;
                    protocol_link = entry.link.forward_link;
                    prev = &entry.link as *const _;
                    assert!(count <= 10, "Too many entries in the event list.");
                }
                assert_eq!(count, 10, "Not all entries were found in the event list.");
            }

            // Remove all the odd events
            for i in (0..10).filter(|x| x % 2 == 1) {
                for _ in data.runtime_events.extract_if(|entry| entry.event == i as efi::Event) {}
            }
            data.update_protocol_lists();

            // SAFETY: Parsing a C-style linked list is inherently unsafe, but if the
            //         update_protocol_lists function is correct, this should be safe.
            unsafe {
                // Walk the linked list starting from the head and make sure all entries are present.
                let mut protocol_link = (*data.runtime_arch_ptr).event_head.forward_link;
                let mut count = 0;
                let mut prev = &*(&(*data.runtime_arch_ptr).event_head as *const _) as *const list_entry::Entry;
                while !core::ptr::eq(protocol_link, &mut (*data.runtime_arch_ptr).event_head as *mut _) {
                    let entry = ((protocol_link as *const u8).byte_sub(link_offset) as *const runtime::EventEntry)
                        .as_ref()
                        .unwrap();
                    assert_eq!(entry.event as usize, count * 2);
                    assert_eq!(entry.link.back_link, prev as *mut _);
                    count += 1;
                    protocol_link = entry.link.forward_link;
                    prev = &entry.link as *const _;
                    assert!(count <= 5, "Too many entries in the event list.");
                }
                assert_eq!(count, 5, "Not all entries were found in the event list.");
            }
        });
    }
}
