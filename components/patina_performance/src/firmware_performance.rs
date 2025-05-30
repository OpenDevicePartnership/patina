use core::mem;
use core::{
    cell::{OnceCell, RefCell},
    ffi::c_void,
    iter::Once,
};

use mu_pi::protocols::runtime;
use patina_sdk::component::service::memory;
use patina_sdk::runtime_services::{self, RuntimeServices, StandardRuntimeServices};
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
use r_efi::efi::{self, Status};
use spin::{rwlock::RwLock, Mutex};

use alloc::boxed::Box;
use alloc::string::String;

use crate::performance_table::{FirmwareBasicBootPerfDataRecord, FirmwarePerformanceVariable};

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
pub struct FirmwarePerformanceDxe<B: BootServices + 'static, R: RuntimeServices + 'static> {
    boot_performance_table: Mutex<BootPerformanceTable>,
    firmware_performance_table: Mutex<FirmwarePerformanceTable>,
    memory_manager: OnceCell<Service<dyn MemoryManager>>,
    boot_services: OnceCell<B>,
    runtime_services: OnceCell<R>,
}

impl<B: BootServices, R: RuntimeServices> FirmwarePerformanceDxe<B, R>
where
    B: BootServices,
{
    pub const fn new() -> Self {
        Self {
            boot_performance_table: Mutex::new(BootPerformanceTable {
                header: AcpiFpdtPerformanceTableHeader::new_boot_performance_table(),
                basic_boot_record: FirmwareBasicBootPerfDataRecord::new(),
            }),
            firmware_performance_table: Mutex::new(FirmwarePerformanceTable {
                header: AcpiDescriptionHeader {
                    signature: 0x54504246, // 'FPDT'
                    length: mem::size_of::<FirmwarePerformanceTable>() as u32,
                    revision: 1,
                    checksum: 0,          // will be calculated later
                    oem_id: [0; 6],       // to be filled in
                    oem_table_id: [0; 8], // to be filled in
                    oem_revision: 0,      // to be filled in
                    creator_id: 0,        // to be filled in
                    creator_revision: 0,  // to be filled in
                },
                basic_boot_record: AcpiFpdtBootPerformanceTablePointerRecord {
                    header: AcpiFpdtPerformanceRecordHeader::new_boot_performance_table(),
                    reserved: 0,
                    boot_performance_table_header: 0, // to be filled in later
                },
            }),
            memory_manager: OnceCell::new(),
            boot_services: OnceCell::new(),
            runtime_services: OnceCell::new(),
        }
    }
}

impl<B, R> FirmwarePerformanceDxe<B, R>
where
    B: BootServices,
    R: RuntimeServices,
{
    fn entry_point(
        self,
        _cfg: Config<FirmwarePerformanceDxeInit>,
        firmware_performance_hob: Hob<FirmwarePerformanceHob>,
        memory_manager: Service<dyn MemoryManager>,
        runtime_services: StandardRuntimeServices,
        // acpi_provider: Service<dyn AcpiProvider>,
    ) -> patina_sdk::error::Result<()> {
        // Get Report Status Code Handler Protocol.
        // Register report status code listener for OS Loader load and start.
        // Register the notify function to install FPDT at EndOfDxe.

        let ctx = Box::new(FpdtContext {
            firmware_table: &self.firmware_performance_table,
            boot_table: &self.boot_performance_table,
            memory_manager,
            runtime_services: &*self.runtime_services.get().unwrap(),
        });

        // also need to init any uninited fields
        self.boot_services.get().unwrap().create_event_ex(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(fpdt_notify_end_of_dxe),
            ctx,
            &EVENT_GROUP_END_OF_DXE,
        )?;
        // Register the notify function to update FPDT on ExitBootServices Event. (similar to above)
        // Retrieve GUID HOB data that contains the ResetEnd.

        // SHERRY: i assume this is the right FBPT to refer to but i could be wrong
        // mBootPerformanceTableTemplate is the global in C
        self.boot_performance_table.lock().basic_boot_record.reset_end = firmware_performance_hob.reset_end;
        Ok(())
    }
}

// this is because the event callback needs to own the data it receives
// but we also need to access the tables across multiple callbacks/local code
// so we pass references instead of letting the callback own the data
// this is evil evil non-rust-friendly code
struct FpdtContext<R>
where
    R: RuntimeServices,
{
    firmware_table: *const spin::Mutex<FirmwarePerformanceTable>,
    boot_table: *const spin::Mutex<BootPerformanceTable>,
    memory_manager: Service<dyn MemoryManager>,
    runtime_services: *const R,
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

struct AcpiDescriptionHeader {
    signature: u32,
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

struct FirmwarePerformanceTable {
    header: AcpiDescriptionHeader,
    basic_boot_record: AcpiFpdtBootPerformanceTablePointerRecord,
}

struct AcpiFpdtBootPerformanceTablePointerRecord {
    header: AcpiFpdtPerformanceRecordHeader,
    reserved: u32,
    boot_performance_table_header: u64,
}

struct AcpiFpdtPerformanceRecordHeader {
    record_type: u16,
    length: u8,
    revision: u8,
}

impl AcpiFpdtPerformanceRecordHeader {
    pub const fn new_boot_performance_table() -> Self {
        Self {
            record_type: 0x0000, // FPDT_BOOT_PERFORMANCE_TABLE_POINTER
            length: mem::size_of::<AcpiFpdtBootPerformanceTablePointerRecord>() as u8,
            revision: 1,
        }
    }
}

unsafe impl Sync for BootPerformanceTable {}
unsafe impl Send for BootPerformanceTable {}

// This makes me uncomfortable because it'll require the use of lots of statics
// It is also technically possible to pass in the Service as context (i think????) - this is hard bc of copying Services / lifetimes
// basically to avoid statics everything needs to be passed in as context
// i think we could also coalesce everything into one static for a slightly less bad solution
// events suck bc it forces us to comply by this non-rust-friendly interface
extern "efiapi" fn fpdt_notify_end_of_dxe<R>(_event: efi::Event, context: Box<FpdtContext<R>>)
where
    R: RuntimeServices,
{
    let (firmware_performance_table, boot_performance_table, memory_manager, runtime_services) =
        (context.firmware_table, context.boot_table, context.memory_manager, context.runtime_services);

    let fw_mutex: &spin::Mutex<FirmwarePerformanceTable> = unsafe { &*firmware_performance_table };
    let bp_mutex: &spin::Mutex<BootPerformanceTable> = unsafe { &*boot_performance_table };

    let mut fw = fw_mutex.lock();
    let mut bp = bp_mutex.lock();

    install_firmware_performance_data_table(&mut fw, &mut bp, memory_manager, unsafe { &*runtime_services }).unwrap();

    // copy boot performance table to reserved memory
    // also, the boot performance table get modified during runtime (before end of DXE) - but how?
    // in c mAcpiBootPerformanceTable and the template are possibly redundant
    // need to update firmware performance table pointers - i can't access here unless i make it static
}

fn install_firmware_performance_data_table<R>(
    firmware_performance_table: &mut FirmwarePerformanceTable,
    boot_performance_table: &mut BootPerformanceTable,
    memory_manager: Service<dyn MemoryManager>,
    runtime_services: &R,
) -> Result<(), FirmwarePerformanceError>
where
    R: RuntimeServices,
{
    let performance_variable = runtime_services
        .get_variable::<FirmwarePerformanceVariable>(
            &[0],
            &FirmwarePerformanceVariable::ADDRESS_VARIABLE_GUID,
            Some(mem::size_of::<FirmwarePerformanceVariable>()),
        )
        .map(|(v, _)| v.boot_performance_table_pointer)
        .map_err(|_| FirmwarePerformanceError::GenericError)?;

    let options = AllocationOptions::new()
        .with_memory_type(patina_sdk::efi_types::EfiMemoryType::ReservedMemoryType)
        .with_strategy(PageAllocationStrategy::Any);
    let alloc =
        memory_manager.allocate_pages(uefi_size_to_pages!(mem::size_of::<BootPerformanceTable>()), options).unwrap();

    // fill in basic boot performance record
    boot_performance_table.basic_boot_record.reset_end = 1;

    // fill in the firmware performance table header
    firmware_performance_table.header.signature = 0x54504246; // 'FPDT'

    // use acpi service (tbd) to install the table
    Ok(())
}

enum FirmwarePerformanceError {
    GenericError,
}

type StatusCodeType = u32;
type StatusCodeValue = u32;

#[repr(C)]
pub struct StatusCodeData {
    pub header_size: u16,
    pub size: u16,
    pub status_code_type: efi::Guid,
    // this is a hack. we include some extra data (that won't be seen by the C code)
    // at least i think it won't be seen by the C code. idk
    boot_table: *const spin::Mutex<BootPerformanceTable>,
}

extern "efiapi" fn fpdt_status_code_listener_dxe(
    code_type: StatusCodeType,
    value: StatusCodeValue,
    instance: u32,
    guid: *const r_efi::efi::Guid,
    data: *const StatusCodeData,
) -> efi::Status {
    // This function is a placeholder for the status code listener.
    // It can be used to log or handle specific status codes during the DXE phase.
    // For now, it does nothing.
    // In a real implementation, you would handle specific codes here.
    // e.g., if code_type == some_code_type && value == some_value { ... }

    // how to update boot performance table EBS time without a global?
    let status_code_data: &StatusCodeData = unsafe { &*data };
    let boot_mutex: &spin::Mutex<BootPerformanceTable> = unsafe { &*status_code_data.boot_table };
    let mut bp = boot_mutex.lock();
    bp.basic_boot_record.exit_boot_services_exit = 1; // this is a placeholder, replace with actual time
    efi::Status::SUCCESS
}
