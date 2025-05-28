// use core::{cell::OnceCell, iter::Once};

// use patina_sdk::component::{
//     hob::{FromHob, Hob},
//     params::Config,
//     IntoComponent,
// };
// use spin::{rwlock::RwLock, Mutex};

// use alloc::string::String;

// use crate::performance_table::FirmwareBasicBootPerfDataRecord;

// #[derive(Default)]
// struct FirmwarePerformanceDxeInit {
//     oem_id: String,
//     oem_table_id: u64,
//     oem_revision: u32,
//     creator_id: u32,
//     creator_revision: u32,
// }

// #[derive(Copy, Clone, FromHob)]
// #[hob = "C095791A-3001-47B2-80C9-EAC7319F2FA4"]
// pub struct FirmwarePerformanceHob {
//     pub reset_end: u64,
// }

// #[derive(IntoComponent)]
// pub struct FirmwarePerformanceDxe {
//     boot_performance_table: Mutex<FirmwarePerformanceAcpiTable>,
//     acpi_boot_performance_table: OnceCell<FirmwarePerformanceAcpiTable>,
//     received_acpi_boot_performance_table: OnceCell<FirmwarePerformanceAcpiTable>,
// }

// impl FirmwarePerformanceDxe {
//     pub const fn new() -> Self {
//         Self {
//             boot_performance_table: Mutex::new(FirmwarePerformanceAcpiTable {
//                 header: AcpiFpdtPerformanceTableHeader::new_boot_performance_table(),
//                 basic_boot_record: FirmwareBasicBootPerfDataRecord::new(),
//             }),
//         }
//     }
// }

// impl FirmwarePerformanceDxe {
//     fn entry_point(
//         self,
//         _cfg: Config<FirmwarePerformanceDxeInit>,
//         firmware_performance_hob: Hob<FirmwarePerformanceHob>,
//     ) -> patina_sdk::error::Result<()> {
//         // Get Report Status Code Handler Protocol.
//         // Register report status code listener for OS Loader load and start.
//         // Register the notify function to install FPDT at EndOfDxe.
//         // Register the notify function to update FPDT on ExitBootServices Event.
//         // Retrieve GUID HOB data that contains the ResetEnd.

//         // SHERRY: i assume this is the right FBPT to refer to but i could be wrong
//         // mBootPerformanceTableTemplate is the global in C
//         self.boot_performance_table.lock().basic_boot_record.reset_end = firmware_performance_hob.reset_end;
//         Ok(())
//     }
// }

// struct AcpiFpdtPerformanceTableHeader {
//     signature: u32,
//     length: u32,
// }

// impl AcpiFpdtPerformanceTableHeader {
//     pub const fn new_boot_performance_table() -> Self {
//         Self { signature: 0x54504246, length: core::mem::size_of::<FirmwarePerformanceAcpiTable>() as u32 }
//     }
// }

// struct FirmwarePerformanceAcpiTable {
//     header: AcpiFpdtPerformanceTableHeader,
//     basic_boot_record: FirmwareBasicBootPerfDataRecord,
// }

// unsafe impl Sync for FirmwarePerformanceAcpiTable {}
// unsafe impl Send for FirmwarePerformanceAcpiTable {}

// fn fpdt_end_of_dxe_event_notify() {
//     // Get AcpiTable Protocol.
//     // SHERRY: we could probably use ACPI services here

//     //
// }
