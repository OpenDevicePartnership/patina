use core::ffi::c_void;
use r_efi::efi;
use uefi_protocol_db::SpinLockedProtocolDb as ProtocolDb;

mod protocols;

static PRIVATE_DATA: PrivateData = PrivateData::new();

struct PrivateData {
    protocol_db: ProtocolDb,
}

impl PrivateData {
    pub const fn new() -> Self {
        Self { protocol_db: ProtocolDb::new() }
    }
}

#[cfg(not(tarpaulin_include))]
pub fn with_protocol_db<R, F: FnOnce(&ProtocolDb) -> R>(f: F) -> R {
    f(&PRIVATE_DATA.protocol_db)
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
        protocols::core_install_protocol_interface(&PRIVATE_DATA.protocol_db, handle, protocol, interface)
    }

    #[cfg(not(tarpaulin_include))]
    pub fn core_locate_device_path(
        protocol: efi::Guid,
        device_path: *const r_efi::protocols::device_path::Protocol,
    ) -> Result<(*mut r_efi::protocols::device_path::Protocol, efi::Handle), efi::Status> {
        protocols::core_locate_device_path(&PRIVATE_DATA.protocol_db, protocol, device_path)
    }
}

// Private efiapi functions to register with boot services table
impl BootServices {
    pub fn register_services(bs: &mut efi::BootServices) {
        Self::register_protocol_services(bs);
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

    #[cfg(not(tarpaulin_include))]
    extern "efiapi" fn install_protocol_interface(
        handle: *mut efi::Handle,
        protocol: *mut efi::Guid,
        interface_type: efi::InterfaceType,
        interface: *mut c_void,
    ) -> efi::Status {
        protocols::install_protocol_interface(&PRIVATE_DATA.protocol_db, handle, protocol, interface_type, interface)
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
        protocols::register_protocol_notify(&PRIVATE_DATA.protocol_db, protocol, event, registration)
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
}
