use core::mem;
use core::{
    cell::{OnceCell, RefCell},
    ffi::c_void,
    iter::Once,
};

use patina_sdk::{
    boot_services::{self, event::EventType, tpl::Tpl, BootServices},
    component::{
        hob::{FromHob, Hob},
        params::Config,
        service::{
            memory::{AllocationOptions, MemoryManager, PageAllocationStrategy},
            Service,
        },
        IntoComponent,
    },
    guid::EVENT_GROUP_END_OF_DXE,
    uefi_size_to_pages,
};
use r_efi::efi;
use spin::{rwlock::RwLock, Mutex};

use alloc::boxed::Box;
use alloc::string::String;

use crate::performance_table::FirmwareBasicBootPerfDataRecord;

#[derive(Default)]
struct FirmwarePerformanceDxeInit {
    oem_id: String,
    oem_table_id: u64,
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

#[derive(Copy, Clone, FromHob)]
#[hob = "C095791A-3001-47B2-80C9-EAC7319F2FA4"]
pub struct FirmwarePerformanceHob {
    pub reset_end: u64,
}

#[derive(IntoComponent)]
pub struct FirmwarePerformanceDxe<B: BootServices + 'static> {
    boot_performance_table: Mutex<BootPerformanceTable>,
    memory_manager: OnceCell<Service<dyn MemoryManager>>,
    boot_services: OnceCell<B>,
}

impl<B> FirmwarePerformanceDxe<B>
where
    B: BootServices,
{
    pub const fn new() -> Self {
        Self {
            boot_performance_table: Mutex::new(BootPerformanceTable {
                header: AcpiFpdtPerformanceTableHeader::new_boot_performance_table(),
                basic_boot_record: FirmwareBasicBootPerfDataRecord::new(),
            }),
            memory_manager: OnceCell::new(),
            boot_services: OnceCell::new(),
        }
    }
}

impl<B> FirmwarePerformanceDxe<B>
where
    B: BootServices,
{
    fn entry_point(
        self,
        _cfg: Config<FirmwarePerformanceDxeInit>,
        firmware_performance_hob: Hob<FirmwarePerformanceHob>,
    ) -> patina_sdk::error::Result<()> {
        // Get Report Status Code Handler Protocol.
        // Register report status code listener for OS Loader load and start.
        // Register the notify function to install FPDT at EndOfDxe.

        // also need to init any uninited fields
        self.boot_services.get().unwrap().create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(end_of_dxe),
            Box::new(0), // placeholder, we may need context
            &EVENT_GROUP_END_OF_DXE,
        )?;
        // Register the notify function to update FPDT on ExitBootServices Event.
        // Retrieve GUID HOB data that contains the ResetEnd.

        // SHERRY: i assume this is the right FBPT to refer to but i could be wrong
        // mBootPerformanceTableTemplate is the global in C
        self.boot_performance_table.lock().basic_boot_record.reset_end = firmware_performance_hob.reset_end;
        Ok(())
    }
}

struct AcpiFpdtPerformanceTableHeader {
    signature: u32,
    length: u32,
}

impl AcpiFpdtPerformanceTableHeader {
    pub const fn new_boot_performance_table() -> Self {
        Self { signature: 0x54504246, length: core::mem::size_of::<BootPerformanceTable>() as u32 }
    }
}

struct BootPerformanceTable {
    header: AcpiFpdtPerformanceTableHeader,
    basic_boot_record: FirmwareBasicBootPerfDataRecord,
}

unsafe impl Sync for BootPerformanceTable {}
unsafe impl Send for BootPerformanceTable {}

static MEMORY_MANAGER: Mutex<Service<dyn MemoryManager>> = Mutex::new(Service::new_uninit());

// This makes me uncomfortable because it'll require the use of lots of statics
// It is also technically possible to pass in the memory service as context (i think????) - this is hard bc of copying Services / lifetimes
// events suck bc it forces us to comply by this non-rust-friendly interface
extern "efiapi" fn end_of_dxe(_event: efi::Event, _context: Box<usize>) {
    let options = AllocationOptions::new()
        .with_memory_type(patina_sdk::efi_types::EfiMemoryType::ReservedMemoryType)
        .with_strategy(PageAllocationStrategy::Any);
    let alloc = MEMORY_MANAGER
        .lock()
        .allocate_pages(uefi_size_to_pages!(mem::size_of::<BootPerformanceTable>()), options)
        .unwrap();

    // copy boot performance table to reserved memory
    // need to update firmware performance table pointers - i can't access here unless i make it static
}
