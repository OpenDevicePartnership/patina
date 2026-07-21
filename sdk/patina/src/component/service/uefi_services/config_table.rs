//! Configuration table services for Patina components.
//!
//! [`ConfigurationTableServices`] exposes UEFI configuration table installation and lookup. The
//! trait is object-safe and works with the opaque [`ConfigTablePtr`] token.
//!
//! Configuration tables associate a GUID with a pointer to a vendor-defined table (for example
//! ACPI or SMBIOS tables). The UEFI Specification allows at most one table per GUID.
//!
//! Component authors should install and retrieve fixed-size tables through
//! [`ConfigurationTableServicesExt`] rather than using [`ConfigurationTableServices`] directly.
//!
//! Implement [`ConfigTable`] to bind a GUID to a concrete Rust type, then use
//! [`ConfigurationTableServicesExt::install`] (or [`ConfigurationTableServicesExt::install_or_replace`]
//! for a table that is republished, such as after each record added to it) and [`ConfigurationTableServicesExt::get`].
//! These methods never expose a raw pointer to the caller, and verify (at runtime) that a lookup's
//! requested type matches the type it was installed with, so [`ConfigurationTableServicesExt::get`] is not `unsafe`.
//!
//! [`ConfigTable`] still requires a fixed-size Rust type, but that type may be a self-describing
//! header for a table with trailing variable-length data. Override [`ConfigTable::table_len`] to report
//! the table's real, total size, and read the whole table (header plus trailing data) with
//! [`ConfigurationTableServicesExt::get_bytes`]. Tables that cannot be represented as a single Rust
//! type at all, because their total size depends on data assembled outside of Patina's control,
//! can fall back to using the `unsafe` [`ConfigurationTableServices::install_table`] with a raw
//! [`ConfigTablePtr`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::any::TypeId;
use core::ffi::c_void;
use core::ptr::NonNull;

use crate::base::error::EfiError;
use crate::base::guid::BinaryGuid;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// An opaque pointer to a vendor configuration table.
///
/// This token is consumed by [`ConfigurationTableServices::install_table`] and returned by
/// [`ConfigurationTableServices::get_table`]. Components should generally use the typed methods on
/// [`ConfigurationTableServicesExt`] rather than handling this token directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigTablePtr(NonNull<c_void>);

impl ConfigTablePtr {
    /// Wraps a raw table pointer.
    ///
    /// This is intended for use by service implementations and the typed extension methods, not
    /// component authors.
    #[doc(hidden)]
    pub fn from_raw(table: *mut c_void) -> Option<Self> {
        NonNull::new(table).map(Self)
    }

    /// Returns the raw table pointer.
    ///
    /// This is intended for use by service implementations and the typed extension methods, not
    /// component authors.
    #[doc(hidden)]
    pub fn as_raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

/// Errors that can occur when using [`ConfigurationTableServices`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigTableError {
    /// A provided parameter was invalid.
    InvalidParameter,
    /// The requested table (or the system table) was not found.
    NotFound,
    /// The system is out of resources to complete the operation.
    OutOfResources,
    /// A table is already installed under the requested GUID.
    AlreadyExists,
    /// An unexpected internal error occurred.
    Internal,
}

impl From<ConfigTableError> for EfiError {
    fn from(value: ConfigTableError) -> Self {
        match value {
            ConfigTableError::InvalidParameter => EfiError::InvalidParameter,
            ConfigTableError::NotFound => EfiError::NotFound,
            ConfigTableError::OutOfResources => EfiError::OutOfResources,
            ConfigTableError::AlreadyExists => EfiError::AlreadyStarted,
            ConfigTableError::Internal => EfiError::Unsupported,
        }
    }
}

impl From<EfiError> for ConfigTableError {
    fn from(value: EfiError) -> Self {
        match value {
            EfiError::InvalidParameter => ConfigTableError::InvalidParameter,
            EfiError::NotFound => ConfigTableError::NotFound,
            EfiError::OutOfResources => ConfigTableError::OutOfResources,
            EfiError::AlreadyStarted => ConfigTableError::AlreadyExists,
            _ => ConfigTableError::Internal,
        }
    }
}

/// Configuration table installation and lookup services.
///
/// This trait is object-safe and deals in opaque tokens. Component authors should generally use the
/// type-safe methods provided by [`ConfigurationTableServicesExt`] instead of this trait directly.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn ConfigurationTableServices>`](crate::component::service::Service) parameter to
/// their entry point.
///
/// # Examples
///
/// ```rust,no_run
/// use patina::BinaryGuid;
/// use patina::component::service::{
///     Service,
///     uefi_services::config_table::{ConfigTable, ConfigurationTableServices, ConfigurationTableServicesExt},
/// };
/// use patina::error::Result;
///
/// #[repr(C)]
/// struct VendorTable {
///     version: u32,
/// }
///
/// impl ConfigTable for VendorTable {
///     const TABLE_GUID: BinaryGuid = BinaryGuid::from_string("0fedcba9-8765-4321-fedc-ba9876543210");
/// }
///
/// static TABLE: VendorTable = VendorTable { version: 1 };
///
/// fn entry_point(config: Service<dyn ConfigurationTableServices>) -> Result<()> {
///     // Publish the table so an OS or later component can find it by GUID.
///     config.install(&TABLE)?;
///     Ok(())
/// }
/// ```
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait ConfigurationTableServices {
    /// Installs or replaces the configuration table associated with `guid`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::NotFound`] if the system table is not available, or
    /// [`ConfigTableError::OutOfResources`] if the table could not be stored.
    ///
    /// # Safety
    ///
    /// `table` must point to valid, initialized memory of the type expected by consumers of `guid`, and
    /// that memory must remain valid for as long as the table stays installed. Since a configuration
    /// table may be looked up at any later point, including by other components or by the OS after
    /// `ExitBootServices`, this is effectively a `'static` requirement.
    unsafe fn install_table(&self, guid: BinaryGuid, table: ConfigTablePtr) -> Result<(), ConfigTableError>;

    /// Removes the configuration table associated with `guid`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::NotFound`] if no table is installed for `guid`.
    fn remove_table(&self, guid: BinaryGuid) -> Result<(), ConfigTableError>;

    /// Returns the configuration table associated with `guid`, if present.
    fn get_table(&self, guid: BinaryGuid) -> Option<ConfigTablePtr>;

    /// Installs the configuration table associated with `guid`, recording `type_id` alongside it.
    ///
    /// This is the primitive backing [`ConfigurationTableServicesExt::install`]. Component authors
    /// should generally use that method instead.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::AlreadyExists`] if a table is already installed under `guid`.
    ///
    /// # Safety
    ///
    /// `table` must point to valid, initialized memory of the type identified by `type_id`, and that
    /// memory must remain valid for as long as the table stays installed (effectively `'static`), since
    /// [`ConfigurationTableServicesExt::get`] later trusts `type_id` alone before casting the pointer.
    unsafe fn install_typed_table(
        &self,
        guid: BinaryGuid,
        type_id: TypeId,
        table: ConfigTablePtr,
    ) -> Result<(), ConfigTableError>;

    /// Returns the configuration table associated with `guid`, if present and if it was installed
    /// with the same `type_id`.
    ///
    /// This is the primitive backing [`ConfigurationTableServicesExt::get`]. Component authors
    /// should generally use that method instead.
    fn get_typed_table(&self, guid: BinaryGuid, type_id: TypeId) -> Option<ConfigTablePtr>;

    /// Removes the configuration table associated with `guid`, along with its recorded type.
    ///
    /// This is the primitive backing [`ConfigurationTableServicesExt::remove`]. Component authors
    /// should generally use that method instead.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::NotFound`] if no table is installed for `guid`.
    fn remove_typed_table(&self, guid: BinaryGuid) -> Result<(), ConfigTableError>;

    /// Installs the configuration table associated with `guid`, replacing any existing table (typed
    /// or not) under the same GUID and recording `type_id` alongside it.
    ///
    /// This is the primitive backing [`ConfigurationTableServicesExt::install_or_replace`]. Component
    /// authors should generally use that method instead.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::NotFound`] if the system table is not available, or
    /// [`ConfigTableError::OutOfResources`] if the table could not be stored.
    ///
    /// # Safety
    ///
    /// `table` must point to valid, initialized memory of the type identified by `type_id`, and that
    /// memory must remain valid for as long as the table stays installed (effectively `'static`), since
    /// [`ConfigurationTableServicesExt::get`] later trusts `type_id` alone before casting the pointer.
    unsafe fn replace_typed_table(
        &self,
        guid: BinaryGuid,
        type_id: TypeId,
        table: ConfigTablePtr,
    ) -> Result<(), ConfigTableError>;
}

/// A Rust type that can be installed as a fixed-size UEFI configuration table.
///
/// Implementing this trait binds a given type to the GUID it is published under, so
/// [`ConfigurationTableServicesExt::install`] and [`ConfigurationTableServicesExt::get`] can install
/// and retrieve it without handling a raw pointer or repeating the GUID at each call site.
///
/// This trait is only for tables whose size is known at compile time. Dynamically-sized tables must use
/// the `unsafe` [`ConfigurationTableServices::install_table`] with a raw [`ConfigTablePtr`] instead.
///
/// ## Example
///
/// ```rust
/// use patina::BinaryGuid;
/// use patina::component::service::{
///     Service,
///     uefi_services::config_table::{ConfigTable, ConfigurationTableServices, ConfigurationTableServicesExt},
/// };
/// use patina::error::Result;
///
/// #[repr(C)]
/// struct VendorTable {
///     version: u32,
/// }
///
/// impl ConfigTable for VendorTable {
///     const TABLE_GUID: BinaryGuid = BinaryGuid::from_string("0fedcba9-8765-4321-fedc-ba9876543210");
/// }
///
/// static TABLE: VendorTable = VendorTable { version: 1 };
///
/// fn entry_point(config: Service<dyn ConfigurationTableServices>) -> Result<()> {
///     config.install(&TABLE)?;
///     let installed: Option<&VendorTable> = config.get::<VendorTable>();
///     Ok(())
/// }
/// ```
pub trait ConfigTable: Sized + 'static {
    /// The GUID this table type is published under.
    const TABLE_GUID: BinaryGuid;

    /// The total size, in bytes, of the table starting at `self`'s address, including any
    /// trailing variable-length data laid out immediately after `self` in the same allocation.
    ///
    /// Defaults to `size_of::<Self>()`, which is correct for a table with no trailing data.
    /// Override this for a self-describing header (for example, one with its own `Length` field)
    /// whose real total size is larger than the header type alone.
    ///
    /// [`ConfigurationTableServicesExt::get_bytes`] uses this value to return the whole table.
    fn table_len(&self) -> usize {
        size_of::<Self>()
    }
}

/// Type-safe extension methods for [`ConfigurationTableServices`].
///
/// [`Self::install`], [`Self::install_or_replace`], [`Self::get`], [`Self::get_bytes`], and
/// [`Self::remove`] work with any `T: `[`ConfigTable`]. The GUID is always `T::TABLE_GUID`, and the
/// underlying service verifies the installed type before a lookup casts the pointer, so
/// [`Self::get`] is not `unsafe`.
pub trait ConfigurationTableServicesExt: ConfigurationTableServices {
    /// Installs `table` as a [`ConfigTable`].
    ///
    /// Fails rather than silently replacing an existing table if one is already installed under
    /// `T::TABLE_GUID`. Use [`Self::install_or_replace`] for a table that is expected to be
    /// republished.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::AlreadyExists`] if a table is already installed under
    /// `T::TABLE_GUID`.
    fn install<T: ConfigTable>(&self, table: &'static T) -> Result<(), ConfigTableError> {
        let ptr = ConfigTablePtr::from_raw(core::ptr::from_ref(table) as *mut c_void)
            .ok_or(ConfigTableError::InvalidParameter)?;
        // SAFETY: `ptr` was derived from `table`, a `&'static T`, so it remains valid for as long as
        // the table could possibly stay installed.
        unsafe { self.install_typed_table(T::TABLE_GUID, TypeId::of::<T>(), ptr) }
    }

    /// Installs `table` as a [`ConfigTable`], replacing any table already installed under
    /// `T::TABLE_GUID`.
    ///
    /// Use this instead of [`Self::install`] for a table that is republished throughout boot
    /// (for example, after each record added to it), rather than installed exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::NotFound`] if the system table is not available, or
    /// [`ConfigTableError::OutOfResources`] if the table could not be stored.
    fn install_or_replace<T: ConfigTable>(&self, table: &'static T) -> Result<(), ConfigTableError> {
        let ptr = ConfigTablePtr::from_raw(core::ptr::from_ref(table) as *mut c_void)
            .ok_or(ConfigTableError::InvalidParameter)?;
        // SAFETY: `ptr` was derived from `table`, a `&'static T`, so it remains valid for as long as
        // the table could possibly stay installed.
        unsafe { self.replace_typed_table(T::TABLE_GUID, TypeId::of::<T>(), ptr) }
    }

    /// Returns the table installed under `T::TABLE_GUID`, if present.
    ///
    /// Returns `None` if no table is installed under `T::TABLE_GUID`, or if the installed table was
    /// not installed as `T` (for example, using [`ConfigurationTableServices::install_table`] with a
    /// mismatched type).
    fn get<T: ConfigTable>(&self) -> Option<&'static T> {
        let ptr = self.get_typed_table(T::TABLE_GUID, TypeId::of::<T>())?;
        // SAFETY: `get_typed_table` only returns `Some` when the table under `T::TABLE_GUID` was
        // recorded with `TypeId::of::<T>()`, which only happens in `install`, so the pointer was
        // installed from a `&'static T` and is valid, aligned, and lives for a `'static` lifetime.
        Some(unsafe { &*(ptr.as_raw() as *const T) })
    }

    /// Returns the whole table installed under `T::TABLE_GUID` as bytes, including any
    /// trailing variable-length data, if present.
    ///
    /// Returns `None` if no table is installed under `T::TABLE_GUID`, or if the installed table was
    /// not installed as `T`.
    ///
    /// # Safety
    ///
    /// The caller must ensure `T::table_len` accurately reports the number of bytes allocated
    /// starting at the installed table's address.
    unsafe fn get_bytes<T: ConfigTable>(&self) -> Option<&'static [u8]> {
        let ptr = self.get_typed_table(T::TABLE_GUID, TypeId::of::<T>())?;
        // SAFETY: see `Self::get` for why `ptr` is a valid, aligned, `'static` `T`.
        let table = unsafe { &*(ptr.as_raw() as *const T) };
        // SAFETY: the caller guarantees `table.table_len()` bytes are valid to read starting here.
        Some(unsafe { core::slice::from_raw_parts(ptr.as_raw() as *const u8, table.table_len()) })
    }

    /// Removes the table installed under `T::TABLE_GUID`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTableError::NotFound`] if no table is installed under `T::TABLE_GUID`.
    fn remove<T: ConfigTable>(&self) -> Result<(), ConfigTableError> {
        self.remove_typed_table(T::TABLE_GUID)
    }
}

impl<T: ConfigurationTableServices + ?Sized> ConfigurationTableServicesExt for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_configuration_table_services_error_conversions() {
        assert_eq!(EfiError::from(ConfigTableError::InvalidParameter), EfiError::InvalidParameter);
        assert_eq!(EfiError::from(ConfigTableError::NotFound), EfiError::NotFound);
        assert_eq!(EfiError::from(ConfigTableError::OutOfResources), EfiError::OutOfResources);
        assert_eq!(EfiError::from(ConfigTableError::AlreadyExists), EfiError::AlreadyStarted);
        assert_eq!(EfiError::from(ConfigTableError::Internal), EfiError::Unsupported);
        assert_eq!(ConfigTableError::from(EfiError::NotFound), ConfigTableError::NotFound);
        assert_eq!(ConfigTableError::from(EfiError::AlreadyStarted), ConfigTableError::AlreadyExists);
        assert_eq!(ConfigTableError::from(EfiError::DeviceError), ConfigTableError::Internal);
    }

    #[test]
    fn test_configuration_table_services_ptr_from_raw_is_none() {
        assert!(ConfigTablePtr::from_raw(core::ptr::null_mut()).is_none());
    }

    #[repr(C)]
    struct FakeConfigTable {
        value: u32,
    }

    impl ConfigTable for FakeConfigTable {
        const TABLE_GUID: BinaryGuid =
            BinaryGuid::from_fields(0x1111_2222, 0x3333, 0x4444, 0x55, 0x66, &[7, 8, 9, 10, 11, 12]);
    }

    static FAKE_CONFIG_TABLE: FakeConfigTable = FakeConfigTable { value: 42 };

    #[test]
    fn test_config_table_ext_install_then_get() {
        let mut mock = MockConfigurationTableServices::new();
        mock.expect_install_typed_table().times(1).returning(|guid, type_id, _| {
            assert_eq!(guid, FakeConfigTable::TABLE_GUID);
            assert_eq!(type_id, TypeId::of::<FakeConfigTable>());
            Ok(())
        });
        mock.expect_get_typed_table().times(1).returning(|guid, type_id| {
            assert_eq!(guid, FakeConfigTable::TABLE_GUID);
            assert_eq!(type_id, TypeId::of::<FakeConfigTable>());
            ConfigTablePtr::from_raw(&raw const FAKE_CONFIG_TABLE as *mut c_void)
        });

        mock.install(&FAKE_CONFIG_TABLE).unwrap();
        let table = mock.get::<FakeConfigTable>().unwrap();
        assert_eq!(table.value, 42);
    }

    #[test]
    fn test_config_table_ext_install_rejects_duplicate() {
        let mut mock = MockConfigurationTableServices::new();
        mock.expect_install_typed_table().times(1).returning(|_, _, _| Err(ConfigTableError::AlreadyExists));

        assert_eq!(mock.install(&FAKE_CONFIG_TABLE), Err(ConfigTableError::AlreadyExists));
    }

    #[test]
    fn test_config_table_ext_get_returns_none_on_type_mismatch() {
        let mut mock = MockConfigurationTableServices::new();
        mock.expect_get_typed_table().times(1).returning(|_, _| None);

        assert!(mock.get::<FakeConfigTable>().is_none());
    }

    #[test]
    fn test_config_table_ext_remove() {
        let mut mock = MockConfigurationTableServices::new();
        mock.expect_remove_typed_table().times(1).returning(|guid| {
            assert_eq!(guid, FakeConfigTable::TABLE_GUID);
            Ok(())
        });

        assert_eq!(mock.remove::<FakeConfigTable>(), Ok(()));
    }

    #[test]
    fn test_config_table_ext_install_or_replace() {
        let mut mock = MockConfigurationTableServices::new();
        mock.expect_replace_typed_table().times(1).returning(|guid, type_id, _| {
            assert_eq!(guid, FakeConfigTable::TABLE_GUID);
            assert_eq!(type_id, TypeId::of::<FakeConfigTable>());
            Ok(())
        });

        mock.install_or_replace(&FAKE_CONFIG_TABLE).unwrap();
    }

    #[test]
    fn test_config_table_table_len_defaults_to_size_of_self() {
        assert_eq!(FAKE_CONFIG_TABLE.table_len(), core::mem::size_of::<FakeConfigTable>());
    }

    /// A self-describing header whose total size is reported by `table_len` and covers trailing
    /// data past the header itself.
    #[repr(C)]
    struct HeaderWithTrailingData {
        total_len: u32,
    }

    impl ConfigTable for HeaderWithTrailingData {
        const TABLE_GUID: BinaryGuid =
            BinaryGuid::from_fields(0x2222_3333, 0x4444, 0x5555, 0x66, 0x77, &[8, 9, 10, 11, 12, 13]);

        fn table_len(&self) -> usize {
            self.total_len as usize
        }
    }

    #[repr(C)]
    struct HeaderWithTrailingDataBuffer {
        header: HeaderWithTrailingData,
        trailing: [u8; 4],
    }

    static HEADER_WITH_TRAILING_DATA_BUFFER: HeaderWithTrailingDataBuffer = HeaderWithTrailingDataBuffer {
        header: HeaderWithTrailingData { total_len: 8 },
        trailing: [0xaa, 0xbb, 0xcc, 0xdd],
    };

    #[test]
    fn test_config_table_ext_get_bytes_returns_whole_table() {
        let mut mock = MockConfigurationTableServices::new();
        mock.expect_get_typed_table().times(1).returning(|guid, type_id| {
            assert_eq!(guid, HeaderWithTrailingData::TABLE_GUID);
            assert_eq!(type_id, TypeId::of::<HeaderWithTrailingData>());
            ConfigTablePtr::from_raw(&raw const HEADER_WITH_TRAILING_DATA_BUFFER as *mut c_void)
        });

        // SAFETY: `HEADER_WITH_TRAILING_DATA_BUFFER.header.total_len` (8) matches the number of
        // bytes actually allocated for the buffer (a 4-byte header plus 4 trailing bytes).
        let bytes = unsafe { mock.get_bytes::<HeaderWithTrailingData>() }.unwrap();
        assert_eq!(bytes.len(), 8);
        assert_eq!(&bytes[4..], &[0xaa, 0xbb, 0xcc, 0xdd]);
    }

    #[test]
    fn test_config_table_ext_get_bytes_returns_none_on_type_mismatch() {
        let mut mock = MockConfigurationTableServices::new();
        mock.expect_get_typed_table().times(1).returning(|_, _| None);

        // SAFETY: no table is returned, so no memory is read.
        assert!(unsafe { mock.get_bytes::<HeaderWithTrailingData>() }.is_none());
    }
}
