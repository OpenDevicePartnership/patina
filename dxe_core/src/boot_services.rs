extern crate alloc;

mod driver;
mod events;
mod misc;
mod protocols;

use alloc::vec::Vec;
use crate::{systemtables::EfiSystemTable, event_db::EventDb, protocol_db::ProtocolDb};
use mu_pi::protocols::{cpu_arch, timer, metronome, watchdog};
use tpl_lock::TplMutex;
use r_efi::efi;
use core::ffi::c_void;
use core::sync::atomic;

static BOOT_SERVICES: BootServices = BootServices::new();

pub struct SystemState {
    // /event.rs related state
    current_tpl: atomic::AtomicUsize,
    time: atomic::AtomicU64,
    cpu_arch_ptr: atomic::AtomicPtr<cpu_arch::Protocol>,
    event_notifies_in_progress: atomic::AtomicBool,
    event_db_initialized: atomic::AtomicBool,
    // /misc.rs related state
    metronome_arch_ptr: atomic::AtomicPtr<metronome::Protocol>,
    watchdog_arch_ptr: atomic::AtomicPtr<watchdog::Protocol>,
    pre_exit_boot_services_signal: atomic::AtomicBool,
}

impl SystemState {
    pub const fn new() -> Self {
        SystemState {
            current_tpl: atomic::AtomicUsize::new(efi::TPL_APPLICATION),
            time: atomic::AtomicU64::new(0),
            cpu_arch_ptr: atomic::AtomicPtr::new(core::ptr::null_mut()),
            event_notifies_in_progress: atomic::AtomicBool::new(false),
            event_db_initialized: atomic::AtomicBool::new(false),
            metronome_arch_ptr: atomic::AtomicPtr::new(core::ptr::null_mut()),
            watchdog_arch_ptr: atomic::AtomicPtr::new(core::ptr::null_mut()),
            pre_exit_boot_services_signal: atomic::AtomicBool::new(false),
        }
    }

    /// Returns the current time of the system
    fn time(&self) -> u64 {
        self.time.load(atomic::Ordering::SeqCst)
    }

    /// Sets the current time of the system
    fn set_time(&self, time: u64) {
        self.time.store(time, atomic::Ordering::SeqCst)
    }

    /// Returns the current TPL level of the system
    fn tpl(&self) -> usize {
        self.current_tpl.load(atomic::Ordering::SeqCst)
    }

    /// Sets the current TPL level of the system
    fn set_tpl(&self, tpl: usize) {
        self.current_tpl.store(tpl, atomic::Ordering::SeqCst)
    }

    fn set_interrupt_state(&self, enable: bool) {
        let cpu_arch_ptr = self.cpu_arch_ptr.load(atomic::Ordering::SeqCst);
        if let Some(cpu_arch) = unsafe { cpu_arch_ptr.as_mut() } {
            match enable {
                true => {
                    (cpu_arch.enable_interrupt)(cpu_arch_ptr);
                }
                false => {
                    (cpu_arch.disable_interrupt)(cpu_arch_ptr);
                }
            };
        }
    }
}

pub struct BootServices {
    protocol_db: TplMutex<ProtocolDb>,
    event_db: TplMutex<EventDb>,
    system_state: SystemState,
}

/// Public functionality for use within the core.
impl BootServices {
    pub const fn new() -> Self {
        BootServices {
            protocol_db: TplMutex::new(efi::TPL_NOTIFY, ProtocolDb::new(), "ProtocolLock"),
            event_db: TplMutex::new(efi::TPL_HIGH_LEVEL, EventDb::new(), "EventLock"),
            system_state: SystemState::new(),
        }
    }

    /// Provides direct access to the protocol_db, skipping the EFI interface
    #[cfg(not(tarpaulin_include))]
    pub fn with_protocol_db<R, F: FnOnce(&mut ProtocolDb) -> R>(f: F) -> R {
        f(&mut BOOT_SERVICES.protocol_db.lock())
    }

    /// Provides direct access to the event_db, skipping the EFI interface
    #[cfg(not(tarpaulin_include))]
    pub fn with_event_db<R, F: FnOnce(&mut EventDb) -> R>(f: F) -> R {
        f(&mut BOOT_SERVICES.event_db.lock())
    }

    /// Provides direct access to the system_state, skipping the EFI interface
    #[cfg(not(tarpaulin_include))]
    pub fn with_system_state<R, F: FnOnce(&SystemState) -> R>(f: F) -> R {
        f(&BOOT_SERVICES.system_state)
    }

    pub fn gcd_map_change(map_change_type: uefi_gcd::gcd::MapChangeType) {
        events::gcd_map_change(&mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, map_change_type)
    }

    /// "Rust-y" version of `install_protocol_interface` for use within the core.
    #[cfg(not(tarpaulin_include))]
    pub fn core_install_protocol_interface(
        handle: Option<efi::Handle>,
        protocol: efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        protocols::core_install_protocol_interface(
            &mut BOOT_SERVICES.protocol_db.lock(),
            &mut BOOT_SERVICES.event_db.lock(),
            &BOOT_SERVICES.system_state,
            handle,
            protocol,
            interface,
        )
    }

    #[cfg(not(tarpaulin_include))]
    pub fn core_locate_device_path(
        protocol: efi::Guid,
        device_path: *const r_efi::protocols::device_path::Protocol,
    ) -> Result<(*mut r_efi::protocols::device_path::Protocol, efi::Handle), efi::Status> {
        protocols::core_locate_device_path(&mut BOOT_SERVICES.protocol_db.lock(), protocol, device_path)
    }

    #[cfg(not(tarpaulin_include))]
    pub fn core_install_configuration_table(
        vendor_guid: efi::Guid,
        vendor_table: Option<&mut c_void>,
        efi_system_table: &mut EfiSystemTable,
    ) -> Result<(), efi::Status> {
        misc::core_install_configuration_table(
            &mut BOOT_SERVICES.event_db.lock(),
            vendor_guid,
            vendor_table,
            efi_system_table,
        )
    }

    #[cfg(not(tarpaulin_include))]
    pub unsafe fn core_connect_controller(
        handle: efi::Handle,
        driver_handles: Vec<efi::Handle>,
        remaining_device_path: Option<*mut efi::protocols::device_path::Protocol>,
        recursive: bool,
    ) -> Result<(), efi::Status> {
        driver::core_connect_controller(
            &mut BOOT_SERVICES.protocol_db.lock(),
            handle,
            driver_handles,
            remaining_device_path,
            recursive,
        )
    }

    #[cfg(not(tarpaulin_include))]
    pub unsafe fn core_disconnect_controller(
        controller_handle: efi::Handle,
        driver_image_handle: Option<efi::Handle>,
        child_handle: Option<efi::Handle>,
    ) -> Result<(), efi::Status> {
        driver::core_disconnect_controller(
            &mut BOOT_SERVICES.protocol_db.lock(),
            controller_handle,
            driver_image_handle,
            child_handle,
        )
    }
}

/// Private functionality associated with registering EFI services for executed drivers.
impl BootServices {
    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn install_protocol_interface(
        handle: *mut efi::Handle,
        protocol: *mut efi::Guid,
        interface_type: efi::InterfaceType,
        interface: *mut c_void,
    ) -> efi::Status {
        protocols::install_protocol_interface(
            &mut BOOT_SERVICES.protocol_db.lock(),
            &mut BOOT_SERVICES.event_db.lock(),
            &BOOT_SERVICES.system_state,
            handle,
            protocol,
            interface_type,
            interface,
        )
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn uninstall_protocol_interface(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        interface: *mut c_void,
    ) -> efi::Status {
        protocols::uninstall_protocol_interface(&mut BOOT_SERVICES.protocol_db.lock(), handle, protocol, interface)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn reinstall_protocol_interface(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        old_interface: *mut c_void,
        new_interface: *mut c_void,
    ) -> efi::Status {
        protocols::reinstall_protocol_interface(&mut BOOT_SERVICES.protocol_db.lock(), &mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, handle, protocol, old_interface, new_interface)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn register_protocol_notify(
        protocol: *mut efi::Guid,
        event: efi::Event,
        registration: *mut *mut c_void,
    ) -> efi::Status {
        protocols::register_protocol_notify(&mut BOOT_SERVICES.protocol_db.lock(), &mut BOOT_SERVICES.event_db.lock(), protocol, event, registration)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_handle(
        search_type: efi::LocateSearchType,
        protocol: *mut efi::Guid,
        key: *mut c_void,
        buffer_size: *mut usize,
        buffer: *mut efi::Handle,
    ) -> efi::Status {
        protocols::locate_handle(&mut BOOT_SERVICES.protocol_db.lock(), search_type, protocol, key, buffer_size, buffer)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn handle_protocol(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        interface: *mut *mut c_void,
    ) -> efi::Status {
        protocols::handle_protocol(&mut BOOT_SERVICES.protocol_db.lock(), handle, protocol, interface)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn open_protocol(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        interface: *mut *mut c_void,
        agent_handle: efi::Handle,
        controller_handle: efi::Handle,
        attributes: u32,
    ) -> efi::Status {
        protocols::open_protocol(&mut BOOT_SERVICES.protocol_db.lock(), handle, protocol, interface, agent_handle, controller_handle, attributes)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn close_protocol(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        agent_handle: efi::Handle,
        controller_handle: efi::Handle,
    ) -> efi::Status {
        protocols::close_protocol(&mut BOOT_SERVICES.protocol_db.lock(), handle, protocol, agent_handle, controller_handle)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn open_protocol_information(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        entry_buffer: *mut *mut efi::OpenProtocolInformationEntry,
        entry_count: *mut usize,
    ) -> efi::Status {
        protocols::open_protocol_information(&mut BOOT_SERVICES.protocol_db.lock(), handle, protocol, entry_buffer, entry_count)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn protocols_per_handle(
        handle: efi::Handle,
        protocol_buffer: *mut *mut *mut efi::Guid,
        protocol_buffer_count: *mut usize,
    ) -> efi::Status {
        protocols::protocols_per_handle(&mut BOOT_SERVICES.protocol_db.lock(), handle, protocol_buffer, protocol_buffer_count)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_handle_buffer(
        search_type: efi::LocateSearchType,
        protocol: *mut efi::Guid,
        key: *mut c_void,
        buffer_size: *mut usize,
        buffer: *mut *mut efi::Handle,
    ) -> efi::Status {
        protocols::locate_handle_buffer(&mut BOOT_SERVICES.protocol_db.lock(), search_type, protocol, key, buffer_size, buffer)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_protocol(
        protocol: *mut efi::Guid,
        registration: *mut c_void,
        interface: *mut *mut c_void,
    ) -> efi::Status {
        protocols::locate_protocol(&mut BOOT_SERVICES.protocol_db.lock(), protocol, registration, interface)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_device_path(
        protocol: *mut efi::Guid,
        device_path: *mut *mut efi::protocols::device_path::Protocol,
        device: *mut efi::Handle,
    ) -> efi::Status {
        protocols::locate_device_path(&mut BOOT_SERVICES.protocol_db.lock(), protocol, device_path, device)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn create_event(
        event_type: u32,
        notify_tpl: efi::Tpl,
        notify_function: Option<efi::EventNotify>,
        notify_context: *mut c_void,
        event: *mut efi::Event,
    ) -> efi::Status {
        events::create_event(&mut BOOT_SERVICES.event_db.lock(), event_type, notify_tpl, notify_function, notify_context, event)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn create_event_ex(
        event_type: u32,
        notify_tpl: efi::Tpl,
        notify_function: Option<efi::EventNotify>,
        notify_context: *const c_void,
        event_group: *const efi::Guid,
        event: *mut efi::Event,
    ) -> efi::Status {
        events::create_event_ex(&mut BOOT_SERVICES.event_db.lock(), event_type, notify_tpl, notify_function, notify_context, event_group, event)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn close_event(event: efi::Event) -> efi::Status {
        events::close_event(&mut BOOT_SERVICES.event_db.lock(), event)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn signal_event(event: efi::Event) -> efi::Status {
        events::signal_event(&mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, event)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn wait_for_event(
        number_of_events: usize,
        event: *mut efi::Event,
        index: *mut usize,
    ) -> efi::Status {
        events::wait_for_event(&mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, number_of_events, event, index)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn check_event(event: efi::Event) -> efi::Status {
        events::check_event(&mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, event)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn set_timer(event: efi::Event, timer_type: efi::TimerDelay, trigger_time: u64) -> efi::Status {
        events::set_timer(&mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, event, timer_type, trigger_time)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn raise_tpl(new_tpl: efi::Tpl) -> efi::Tpl {
        events::raise_tpl(&BOOT_SERVICES.system_state, new_tpl)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn restore_tpl(old_tpl: efi::Tpl) {
        events::restore_tpl(&mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, old_tpl)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn timer_tick(time: u64) {
        events::timer_tick(&mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, time)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn timer_available_callback(event: efi::Event, context: *mut c_void) {
        events::timer_available_callback(&mut BOOT_SERVICES.protocol_db.lock(), &mut BOOT_SERVICES.event_db.lock(), event,context)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn cpu_arch_available(event: efi::Event, context: *mut c_void) {
        events::cpu_arch_available(&mut BOOT_SERVICES.protocol_db.lock(), &mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, event, context)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn calculate_crc32(data: *mut c_void, data_size: usize, crc_32: *mut u32) -> efi::Status {
        misc::calculate_crc32(data, data_size, crc_32)
    }
    
    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn install_configuration_table(table_guid: *mut efi::Guid, table: *mut c_void) -> efi::Status {
        misc::install_configuration_table(&mut BOOT_SERVICES.event_db.lock(), table_guid, table)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn stall(microseconds: usize) -> efi::Status {
        misc::stall(&BOOT_SERVICES.system_state, microseconds)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn set_watchdog_timer(
        timeout: usize,
        watchdog_code: u64,
        data_size: usize,
        data: *mut efi::Char16,
    ) -> efi::Status {
        misc::set_watchdog_timer(&BOOT_SERVICES.system_state, timeout, watchdog_code, data_size, data)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn metronome_arch_available(event: efi::Event, context: *mut c_void) {
        misc::metronome_arch_available(&mut BOOT_SERVICES.protocol_db.lock(), &mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, event, context)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn watchdog_arch_available(event: efi::Event, context: *mut c_void) {
        misc::watchdog_arch_available(&mut BOOT_SERVICES.protocol_db.lock(), &mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, event, context)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn exit_boot_services(handle: efi::Handle, map_key: usize) -> efi::Status {
        misc::exit_boot_services(&mut BOOT_SERVICES.protocol_db.lock(), &mut BOOT_SERVICES.event_db.lock(), &BOOT_SERVICES.system_state, handle, map_key)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn connect_controller(
        handle: efi::Handle,
        driver_image_handle: *mut efi::Handle,
        remaining_device_path: *mut efi::protocols::device_path::Protocol,
        recursive: efi::Boolean,
    ) -> efi::Status {
        driver::connect_controller(
            &mut BOOT_SERVICES.protocol_db.lock(),
            handle,
            driver_image_handle,
            remaining_device_path,
            recursive,
        )
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn disconnect_controller(
        controller_handle: efi::Handle,
        driver_image_handle: efi::Handle,
        child_handle: efi::Handle,
    ) -> efi::Status {
        driver::disconnect_controller(
            &mut BOOT_SERVICES.protocol_db.lock(),
            controller_handle,
            driver_image_handle,
            child_handle,
        )
    }

    pub fn register_services(bs: &mut efi::BootServices) {
        Self::register_protocol_services(bs);
        Self::register_event_services(bs);
        Self::register_misc_services(bs);
        Self::register_driver_services(bs);
    }

    fn register_protocol_services(bs: &mut efi::BootServices) {
        BootServices::with_protocol_db(|db| db.init());

        bs.install_protocol_interface = Self::install_protocol_interface;
        bs.uninstall_protocol_interface = Self::uninstall_protocol_interface;
        bs.reinstall_protocol_interface = Self::reinstall_protocol_interface;
        bs.register_protocol_notify = Self::register_protocol_notify;
        bs.locate_handle = Self::locate_handle;
        bs.handle_protocol = Self::handle_protocol;
        bs.open_protocol = Self::open_protocol;
        bs.close_protocol = Self::close_protocol;
        bs.open_protocol_information = Self::open_protocol_information;
        bs.protocols_per_handle = Self::protocols_per_handle;
        bs.locate_handle_buffer = Self::locate_handle_buffer;
        bs.locate_protocol = Self::locate_protocol;
        bs.locate_device_path = Self::locate_device_path;

        //This bit of trickery is needed because r_efi definition of (Un)InstallMultipleProtocolInterfaces
        //is not variadic, due to rust only supporting variadic for "unsafe extern C" and not "efiapi"
        //until very recently. For x86_64 "efiapi" and "extern C" match, so we can get away with a
        //transmute here. Fixing it for other architectures more generally would require an upstream
        //change in r_efi to pick up. There is also a bug in r_efi definition for
        //uninstall_multiple_program_interfaces - per spec, the first argument is a handle, but
        //r_efi has it as *mut handle.
        bs.install_multiple_protocol_interfaces = unsafe {
            let ptr = protocols::install_multiple_protocol_interfaces as *const ();
            core::mem::transmute::<*const (), extern "efiapi" fn(*mut *mut c_void, *mut c_void, *mut c_void) -> efi::Status>(
                ptr,
            )
        };
        bs.uninstall_multiple_protocol_interfaces = unsafe {
            let ptr = protocols::uninstall_multiple_protocol_interfaces as *const ();
            core::mem::transmute::<*const (), extern "efiapi" fn(*mut c_void, *mut c_void, *mut c_void) -> efi::Status>(ptr)
        };
    }

    fn register_event_services(bs: &mut efi::BootServices) {
        bs.create_event = Self::create_event;
        bs.create_event_ex = Self::create_event_ex;
        bs.close_event = Self::close_event;
        bs.signal_event = Self::signal_event;
        bs.wait_for_event = Self::wait_for_event;
        bs.check_event = Self::check_event;
        bs.set_timer = Self::set_timer;
        bs.raise_tpl = Self::raise_tpl;
        bs.restore_tpl = Self::restore_tpl;

        //set up call back for cpu arch protocol installation.
        let event = Self::with_event_db(|db| {
            db.create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(Self::cpu_arch_available), None, None)
        }).expect("Failed to create timer available callback.");

        Self::with_protocol_db(|db| {
            db.register_protocol_notify(cpu_arch::PROTOCOL_GUID, event)
        }).expect("Failed to register protocol notify on timer arch callback.");

        //set up call back for timer arch protocol installation.
        let event = Self::with_event_db(|db| {
            db.create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(Self::timer_available_callback), None, None)
        }).expect("Failed to create timer available callback.");

        Self::with_protocol_db(|db| {
            db.register_protocol_notify(timer::PROTOCOL_GUID, event)
        }).expect("Failed to register protocol notify on timer arch callback.");
        
        Self::with_system_state(|st| st.event_db_initialized.store(true, atomic::Ordering::SeqCst));
    }
    
    fn register_misc_services(bs: &mut efi::BootServices) {
        bs.calculate_crc32 = Self::calculate_crc32;
        bs.install_configuration_table = Self::install_configuration_table;
        bs.stall = Self::stall;
        bs.set_watchdog_timer = Self::set_watchdog_timer;
        bs.exit_boot_services = Self::exit_boot_services;

        //set up call back for metronome arch protocol installation.
        let event = Self::with_event_db(|db| {
            db.create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(Self::metronome_arch_available), None, None)
        }).expect("Failed to create metronome available callback.");

        Self::with_protocol_db(|db| {
            db.register_protocol_notify(metronome::PROTOCOL_GUID, event)
        }).expect("Failed to register protocol notify on metronome available.");

        //set up call back for watchdog arch protocol installation.
        let event = Self::with_event_db(|db| {
            db.create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(Self::watchdog_arch_available), None, None)
        }).expect("Failed to create watchdog available callback.");

        Self::with_protocol_db(|db| {
            db.register_protocol_notify(watchdog::PROTOCOL_GUID, event)
        }).expect("Failed to register protocol notify on watchdog available.");
    }

    fn register_driver_services(bs: &mut efi::BootServices) {
        bs.connect_controller = Self::connect_controller;
        bs.disconnect_controller = Self::disconnect_controller;
    }
}

unsafe impl Sync for BootServices {}
unsafe impl Send for BootServices {} 
