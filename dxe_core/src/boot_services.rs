use crate::protocol_db::SpinLockedProtocolDb as ProtocolDb;
use core::ffi::c_void;
use mu_pi::protocols::{cpu_arch, timer};
use r_efi::efi;
use uefi_event::SpinLockedEventDb as EventDb;

mod events;
mod protocols;

use events::EventState;

static PRIVATE_DATA: PrivateData = PrivateData::new();

struct PrivateData {
    protocol_db: ProtocolDb,
    event_db: EventDb,
    event_state: EventState,
}

impl PrivateData {
    pub const fn new() -> Self {
        Self { protocol_db: ProtocolDb::new(), event_db: EventDb::new(), event_state: EventState::new() }
    }
}

#[cfg(not(tarpaulin_include))]
pub fn with_protocol_db<R, F: FnOnce(&ProtocolDb) -> R>(f: F) -> R {
    f(&PRIVATE_DATA.protocol_db)
}

#[cfg(not(tarpaulin_include))]
pub fn with_event_db<R, F: FnOnce(&EventDb) -> R>(f: F) -> R {
    f(&PRIVATE_DATA.event_db)
}

#[cfg(not(tarpaulin_include))]
pub fn with_event_state<R, F: FnOnce(&EventState) -> R>(f: F) -> R {
    f(&PRIVATE_DATA.event_state)
}

pub struct BootServices;

// Public functionality for use within the Core.
impl BootServices {
    #[cfg(not(tarpaulin_include))]
    pub fn core_install_protocol_interface(
        handle: Option<efi::Handle>,
        protocol: efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        protocols::core_install_protocol_interface(
            &PRIVATE_DATA.protocol_db,
            &PRIVATE_DATA.event_db,
            &PRIVATE_DATA.event_state,
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
        protocols::core_locate_device_path(&PRIVATE_DATA.protocol_db, protocol, device_path)
    }

    #[cfg(not(tarpaulin_include))]
    pub fn gcd_map_change(map_change_type: uefi_gcd::gcd::MapChangeType) {
        events::gcd_map_change(&PRIVATE_DATA.event_db, &PRIVATE_DATA.event_state, map_change_type);
    }
}

// Private efiapi functions to register with boot services table
impl BootServices {
    pub fn register_services(bs: &mut efi::BootServices) {
        Self::register_protocol_services(bs);
        Self::register_event_services(bs);
    }

    fn register_protocol_services(bs: &mut efi::BootServices) {
        with_protocol_db(|db| db.init_protocol_db());

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
            core::mem::transmute::<
                *const (),
                extern "efiapi" fn(*mut *mut c_void, *mut c_void, *mut c_void) -> efi::Status,
            >(ptr)
        };
        bs.uninstall_multiple_protocol_interfaces = unsafe {
            let ptr = protocols::uninstall_multiple_protocol_interfaces as *const ();
            core::mem::transmute::<*const (), extern "efiapi" fn(*mut c_void, *mut c_void, *mut c_void) -> efi::Status>(
                ptr,
            )
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
        let event = with_event_db(|db| {
            db.create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(Self::cpu_arch_available), None, None)
                .expect("Failed to create timer available callback.")
        });

        with_protocol_db(|db| {
            db.register_protocol_notify(cpu_arch::PROTOCOL_GUID, event)
                .expect("Failed to register protocol notify on timer arch callback.")
        });

        //set up call back for timer arch protocol installation.
        let event = with_event_db(|db| {
            db.create_event(efi::EVT_NOTIFY_SIGNAL, efi::TPL_CALLBACK, Some(Self::timer_available_callback), None, None)
                .expect("Failed to create timer available callback.")
        });

        with_protocol_db(|db| {
            db.register_protocol_notify(timer::PROTOCOL_GUID, event)
                .expect("Failed to register protocol notify on timer arch callback.")
        });

        with_event_state(|state| state.set_initialized());
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn install_protocol_interface(
        handle: *mut efi::Handle,
        protocol: *mut efi::Guid,
        interface_type: efi::InterfaceType,
        interface: *mut c_void,
    ) -> efi::Status {
        protocols::install_protocol_interface(
            &PRIVATE_DATA.protocol_db,
            &PRIVATE_DATA.event_db,
            &PRIVATE_DATA.event_state,
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
        protocols::uninstall_protocol_interface(&PRIVATE_DATA.protocol_db, handle, protocol, interface)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn reinstall_protocol_interface(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        old_interface: *mut c_void,
        new_interface: *mut c_void,
    ) -> efi::Status {
        protocols::reinstall_protocol_interface(
            &PRIVATE_DATA.protocol_db,
            &PRIVATE_DATA.event_db,
            &PRIVATE_DATA.event_state,
            handle,
            protocol,
            old_interface,
            new_interface,
        )
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn register_protocol_notify(
        protocol: *mut efi::Guid,
        event: efi::Event,
        registration: *mut *mut c_void,
    ) -> efi::Status {
        protocols::register_protocol_notify(
            &PRIVATE_DATA.protocol_db,
            &PRIVATE_DATA.event_db,
            protocol,
            event,
            registration,
        )
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_handle(
        search_type: efi::LocateSearchType,
        protocol: *mut efi::Guid,
        search_key: *mut c_void,
        buffer_size: *mut usize,
        handle_buffer: *mut efi::Handle,
    ) -> efi::Status {
        protocols::locate_handle(
            &PRIVATE_DATA.protocol_db,
            search_type,
            protocol,
            search_key,
            buffer_size,
            handle_buffer,
        )
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn handle_protocol(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        interface: *mut *mut c_void,
    ) -> efi::Status {
        protocols::handle_protocol(&PRIVATE_DATA.protocol_db, handle, protocol, interface)
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
        protocols::open_protocol(
            &PRIVATE_DATA.protocol_db,
            handle,
            protocol,
            interface,
            agent_handle,
            controller_handle,
            attributes,
        )
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn close_protocol(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        agent_handle: efi::Handle,
        controller_handle: efi::Handle,
    ) -> efi::Status {
        protocols::close_protocol(&PRIVATE_DATA.protocol_db, handle, protocol, agent_handle, controller_handle)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn open_protocol_information(
        handle: efi::Handle,
        protocol: *mut efi::Guid,
        entry_buffer: *mut *mut efi::OpenProtocolInformationEntry,
        entry_count: *mut usize,
    ) -> efi::Status {
        protocols::open_protocol_information(&PRIVATE_DATA.protocol_db, handle, protocol, entry_buffer, entry_count)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn protocols_per_handle(
        handle: efi::Handle,
        protocol_buffer: *mut *mut *mut efi::Guid,
        protocol_buffer_count: *mut usize,
    ) -> efi::Status {
        protocols::protocols_per_handle(&PRIVATE_DATA.protocol_db, handle, protocol_buffer, protocol_buffer_count)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_handle_buffer(
        search_type: efi::LocateSearchType,
        protocol: *mut efi::Guid,
        search_key: *mut c_void,
        no_handles: *mut usize,
        buffer: *mut *mut efi::Handle,
    ) -> efi::Status {
        protocols::locate_handle_buffer(
            &PRIVATE_DATA.protocol_db,
            search_type,
            protocol,
            search_key,
            no_handles,
            buffer,
        )
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_protocol(
        protocol: *mut efi::Guid,
        registration: *mut c_void,
        interface: *mut *mut c_void,
    ) -> efi::Status {
        protocols::locate_protocol(&PRIVATE_DATA.protocol_db, protocol, registration, interface)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn locate_device_path(
        protocol: *mut efi::Guid,
        device_path: *mut *mut r_efi::protocols::device_path::Protocol,
        device: *mut efi::Handle,
    ) -> efi::Status {
        protocols::locate_device_path(&PRIVATE_DATA.protocol_db, protocol, device_path, device)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn create_event(
        event_type: u32,
        notify_tpl: efi::Tpl,
        notify_function: Option<efi::EventNotify>,
        notify_context: *mut c_void,
        event: *mut efi::Event,
    ) -> efi::Status {
        events::create_event(&PRIVATE_DATA.event_db, event_type, notify_tpl, notify_function, notify_context, event)
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
        events::create_event_ex(
            &PRIVATE_DATA.event_db,
            event_type,
            notify_tpl,
            notify_function,
            notify_context,
            event_group,
            event,
        )
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn close_event(event: efi::Event) -> efi::Status {
        events::close_event(&PRIVATE_DATA.event_db, event)
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn signal_event(event: efi::Event) -> efi::Status {
        events::signal_event(&PRIVATE_DATA.event_db, &PRIVATE_DATA.event_state, event)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn wait_for_event(
        number_of_events: usize,
        event_array: *mut efi::Event,
        out_index: *mut usize,
    ) -> efi::Status {
        events::wait_for_event(
            &PRIVATE_DATA.event_db,
            &PRIVATE_DATA.event_state,
            number_of_events,
            event_array,
            out_index,
        )
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn check_event(event: efi::Event) -> efi::Status {
        events::check_event(&PRIVATE_DATA.event_db, &PRIVATE_DATA.event_state, event)
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn set_timer(event: efi::Event, timer_type: efi::TimerDelay, trigger_time: u64) -> efi::Status {
        events::set_timer(&PRIVATE_DATA.event_db, &PRIVATE_DATA.event_state, event, timer_type, trigger_time)
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn raise_tpl(new_tpl: efi::Tpl) -> efi::Tpl {
        events::raise_tpl(&PRIVATE_DATA.event_state, new_tpl)
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn restore_tpl(new_tpl: efi::Tpl) {
        events::restore_tpl(&PRIVATE_DATA.event_db, &PRIVATE_DATA.event_state, new_tpl)
    }

    #[cfg(not(tarpaulin_include))]
    pub extern "efiapi" fn timer_tick(time: u64) {
        events::timer_tick(&PRIVATE_DATA.event_db, &PRIVATE_DATA.event_state, time)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn timer_available_callback(event: efi::Event, context: *mut c_void) {
        events::timer_available_callback(&PRIVATE_DATA.protocol_db, &PRIVATE_DATA.event_db, event, context)
    }

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn cpu_arch_available(event: efi::Event, context: *mut c_void) {
        events::cpu_arch_available(
            &PRIVATE_DATA.protocol_db,
            &PRIVATE_DATA.event_db,
            &PRIVATE_DATA.event_state,
            event,
            context,
        )
    }
}
