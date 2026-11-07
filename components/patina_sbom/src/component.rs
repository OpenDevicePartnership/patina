//! Component publishing an SBOM ACPI table at Ready to Boot.
//!
//! Copyright (C) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use crate::hob::SbomHob;
use alloc::vec::Vec;
use core::mem::{self, size_of};
use patina::{
    component::{
        component,
        hob::Hob,
        service::{
            Service,
            uefi_services::{
                event::{EventServices, EventServicesExt, Tpl},
                protocol::{ProtocolServices, ProtocolServicesExt},
            },
        },
    },
    error::EfiError,
    uefi::acpi_table::{
        ACPI_CHECKSUM_OFFSET, AcpiDescriptionHeader,
        sbom::{EFI_ACPI_SBOM_TABLE_REVISION, EFI_ACPI_SBOM_TABLE_SIGNATURE},
    },
    uefi::{acpi_table::AcpiTableProtocol, event::READY_TO_BOOT_EVENT_GROUP_GUID},
    vendor_protocols::sbom::{PROTOCOL_GUID as SBOM_PROTOCOL_GUID, SbomProtocol},
};
use zerocopy::IntoBytes;

/// Publishes a combined ACPI SBOM table once, at Ready to Boot.
pub struct SbomComponent {
    header: AcpiDescriptionHeader,
    entries: Vec<u8>,
}

#[component]
impl SbomComponent {
    /// Creates a publisher with the platform's ACPI OEM and creator metadata.
    pub const fn new(
        oem_id: [u8; 6],
        oem_table_id: [u8; 8],
        oem_revision: u32,
        creator_id: u32,
        creator_revision: u32,
    ) -> Self {
        Self {
            header: AcpiDescriptionHeader {
                signature: EFI_ACPI_SBOM_TABLE_SIGNATURE.to_le(),
                length: 0,
                revision: EFI_ACPI_SBOM_TABLE_REVISION,
                checksum: 0,
                oem_id,
                oem_table_id,
                oem_revision: oem_revision.to_le(),
                creator_id: creator_id.to_le(),
                creator_revision: creator_revision.to_le(),
            },
            entries: Vec::new(),
        }
    }

    fn entry_point(
        mut self,
        hobs: Option<Hob<SbomHob>>,
        events: Service<dyn EventServices>,
        protocols: Service<dyn ProtocolServices>,
    ) -> Result<(), EfiError> {
        if let Some(hobs) = hobs {
            for hob in &hobs {
                append_entry(&mut self.entries, hob.entry().as_bytes())?;
            }
        }
        let mut publisher = SbomPublisher { header: self.header, entries: self.entries, protocols, events };
        events.on_event_group_self_managed(READY_TO_BOOT_EVENT_GROUP_GUID, Tpl::Callback, move |event| {
            if let Err(error) = publisher.publish() {
                log::error!("SBOM: failed to publish ACPI table: {error}");
            }
            if let Err(error) = publisher.events.close_event(event) {
                log::error!("SBOM: failed to close Ready to Boot event: {error:?}");
            }
        })?;
        Ok(())
    }
}

struct SbomPublisher {
    header: AcpiDescriptionHeader,
    entries: Vec<u8>,
    protocols: Service<dyn ProtocolServices>,
    events: Service<dyn EventServices>,
}

impl SbomPublisher {
    fn publish(&mut self) -> Result<(), EfiError> {
        let protocols = &self.protocols;
        let mut entries = mem::take(&mut self.entries);
        for handle in protocols.locate_handles(SBOM_PROTOCOL_GUID)? {
            let interface = protocols.interface_on_handle(handle, SBOM_PROTOCOL_GUID)?;
            // SAFETY: The SBOM provider guarantees readable descriptor and entry storage. At TPL Callback,
            // image-unload application code cannot run while this immediate copy is performed.
            // The pointer is used only here and is never retained after the notification.
            let protocol = unsafe { &*interface.as_raw().cast::<SbomProtocol>() };
            // SAFETY: The provider guarantees the declared allocation; it is copied immediately
            // while image-unload application code cannot run.
            match unsafe { protocol.entry() } {
                Ok(entry) => append_entry(&mut entries, entry.as_bytes())?,
                Err(error) => log::warn!("SBOM: ignoring invalid entry on handle {handle:?}: {error}"),
            }
        }
        if entries.is_empty() {
            return Ok(());
        }
        let table = build_table(self.header, &entries)?;
        protocols.with_protocol::<AcpiTableProtocol, _>(|protocol| protocol.install(&table).map(|_| ()))?
    }
}

fn build_table(mut header: AcpiDescriptionHeader, entries: &[u8]) -> Result<Vec<u8>, EfiError> {
    let length = table_length(entries.len())?;
    header.length = length.to_le();
    header.checksum = 0;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length as usize).map_err(|_| EfiError::OutOfResources)?;
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(entries);
    let checksum = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    *bytes.get_mut(ACPI_CHECKSUM_OFFSET).ok_or(EfiError::LoadError)? = checksum;
    Ok(bytes)
}

fn append_entry(bytes: &mut Vec<u8>, entry: &[u8]) -> Result<(), EfiError> {
    table_length(bytes.len().checked_add(entry.len()).ok_or(EfiError::OutOfResources)?)?;
    bytes.try_reserve(entry.len()).map_err(|_| EfiError::OutOfResources)?;
    bytes.extend_from_slice(entry);
    Ok(())
}

fn table_length(entries_length: usize) -> Result<u32, EfiError> {
    let length = size_of::<AcpiDescriptionHeader>().checked_add(entries_length).ok_or(EfiError::OutOfResources)?;
    u32::try_from(length).map_err(|_| EfiError::OutOfResources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{boxed::Box, vec};
    use core::{cell::RefCell, ffi::c_void, ptr::NonNull, slice};
    use patina::{
        component::{
            hob::FromHob,
            service::uefi_services::{
                event::{Event, EventError, MockEventServices},
                protocol::{Handle, MockProtocolServices, ProtocolError, ProtocolPtr},
            },
        },
        protocol::ProtocolInterface,
        standard::efi,
        uefi::acpi_table::sbom::{
            EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH, EFI_ACPI_SBOM_TABLE_ENTRY_REVISION, SbomTable,
        },
    };

    #[repr(C)]
    struct TestAcpiProtocol {
        install: unsafe extern "efiapi" fn(*const AcpiTableProtocol, *const c_void, usize, *mut usize) -> efi::Status,
        uninstall: unsafe extern "efiapi" fn(*const AcpiTableProtocol, usize) -> efi::Status,
        tables: RefCell<Vec<Vec<u8>>>,
        status: efi::Status,
    }

    impl TestAcpiProtocol {
        fn new(status: efi::Status) -> Box<Self> {
            Box::new(Self {
                install: Self::install,
                uninstall: Self::uninstall,
                tables: RefCell::new(Vec::new()),
                status,
            })
        }

        unsafe extern "efiapi" fn install(
            this: *const AcpiTableProtocol,
            table: *const c_void,
            length: usize,
            key: *mut usize,
        ) -> efi::Status {
            // SAFETY: TestAcpiProtocol is repr(C) with the ACPI protocol's two-function prefix, and
            // the publisher provides a live table buffer and writable key for this call.
            let (this, bytes) = unsafe { (&*this.cast::<Self>(), slice::from_raw_parts(table.cast::<u8>(), length)) };
            this.tables.borrow_mut().push(bytes.to_vec());
            // SAFETY: The publisher provides a writable key for the duration of this call.
            unsafe { *key = 1 };
            this.status
        }

        unsafe extern "efiapi" fn uninstall(_: *const AcpiTableProtocol, _: usize) -> efi::Status {
            efi::Status::UNSUPPORTED
        }
    }

    fn publisher() -> SbomComponent {
        SbomComponent::new(*b"OEMID ", *b"PLATFORM", 0x12345678, u32::from_le_bytes(*b"TOOL"), 0x87654321)
    }

    fn initialized_publisher(protocols: MockProtocolServices) -> SbomPublisher {
        let component = publisher();
        SbomPublisher {
            header: component.header,
            entries: component.entries,
            protocols: Service::mock(Box::new(protocols)),
            events: Service::mock(Box::new(MockEventServices::new())),
        }
    }

    fn entry(payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![EFI_ACPI_SBOM_TABLE_ENTRY_REVISION];
        bytes.extend_from_slice(&EFI_ACPI_SBOM_TABLE_ENTRY_HEADER_LENGTH.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&[0, 1, 0]);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn events() -> MockEventServices {
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(1).returning(|group, tpl, mut callback| {
            assert_eq!(group, READY_TO_BOOT_EVENT_GROUP_GUID);
            assert_eq!(tpl, Tpl::Callback);
            let event = Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap();
            callback(event);
            Ok(event)
        });
        events.expect_close_event().times(1).returning(|_| Ok(()));
        events
    }

    fn protocols(acpi: &TestAcpiProtocol, entries: Vec<Vec<u8>>) -> MockProtocolServices {
        let mut protocols = MockProtocolServices::new();
        let count = entries.len();
        protocols.expect_locate_handles().times(1).returning(move |guid| {
            assert_eq!(guid, SBOM_PROTOCOL_GUID);
            Ok((1..=count).map(|id| Handle::from_raw(id as *mut c_void).unwrap()).collect())
        });
        if count != 0 {
            let descriptors: Vec<_> = entries
                .iter()
                .map(|bytes| Box::new(SbomProtocol { entry: bytes.as_ptr(), entry_size: bytes.len() }))
                .collect();
            protocols.expect_interface_on_handle().times(count).returning_st(move |handle, guid| {
                assert_eq!(guid, SBOM_PROTOCOL_GUID);
                let index = handle.as_raw() as usize - 1;
                assert_eq!(descriptors[index].entry_size, entries[index].len());
                Ok(ProtocolPtr::from_raw(core::ptr::from_ref(&*descriptors[index]).cast_mut().cast()).unwrap())
            });
        }
        let address = core::ptr::from_ref(acpi) as usize;
        protocols.expect_locate_interface().times(1).returning(move |guid| {
            assert_eq!(guid, AcpiTableProtocol::PROTOCOL_GUID);
            Ok(ProtocolPtr::from_raw(address as *mut c_void).unwrap())
        });
        protocols
    }

    fn assert_table(bytes: &[u8], payloads: &[&[u8]]) {
        let (table, remaining) = SbomTable::parse_prefix(bytes).unwrap();
        assert!(remaining.is_empty());
        let header = table.header;
        assert_eq!(header.oem_id, *b"OEMID ");
        assert_eq!(header.oem_table_id, *b"PLATFORM");
        assert_eq!(u32::from_le(header.oem_revision), 0x12345678);
        assert_eq!(u32::from_le(header.creator_id), u32::from_le_bytes(*b"TOOL"));
        assert_eq!(u32::from_le(header.creator_revision), 0x87654321);
        assert_eq!(u32::from_le(header.length) as usize, bytes.len());
        assert_eq!(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)), 0);
        let parsed: Vec<_> = table.entries().map(|entry| &entry.unwrap().payload).collect();
        assert_eq!(parsed, payloads);
    }

    #[test]
    fn test_sbom_component_hob_and_protocol_entries_publish_once() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let hobs =
            Hob::mock(vec![SbomHob::parse(&entry(b"hob-one")).unwrap(), SbomHob::parse(&entry(b"hob-two")).unwrap()]);
        publisher()
            .entry_point(
                Some(hobs),
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![entry(b"image-one"), entry(b"image-two")]))),
            )
            .unwrap();
        let tables = acpi.tables.borrow();
        assert_eq!(tables.len(), 1);
        assert_table(&tables[0], &[b"hob-one", b"hob-two", b"image-one", b"image-two"]);
    }

    #[test]
    fn test_sbom_component_hob_only() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        publisher()
            .entry_point(
                Some(Hob::mock(vec![SbomHob::parse(&entry(b"hob")).unwrap()])),
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![]))),
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b"hob"]);
    }

    #[test]
    fn test_sbom_component_protocol_only_with_empty_payload() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![entry(b"")]))),
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b""]);
    }

    #[test]
    fn test_sbom_component_no_entries_skips_installation() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_handles().times(1).returning(|_| Ok(Vec::new()));
        publisher().entry_point(None, Service::mock(Box::new(events())), Service::mock(Box::new(protocols))).unwrap();
        assert!(acpi.tables.borrow().is_empty());
    }

    #[test]
    fn test_sbom_component_malformed_entries_are_skipped() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let mut bad_revision = entry(b"revision");
        bad_revision[0] = 0;
        let mut bad_header = entry(b"header");
        bad_header[1] = 0;
        let mut truncated = entry(b"truncated");
        truncated.pop();
        let mut trailing = entry(b"trailing");
        trailing.push(0);
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(
                    &acpi,
                    vec![bad_revision, bad_header, truncated, trailing, entry(b"valid")],
                ))),
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b"valid"]);
    }

    #[test]
    fn test_sbom_component_installation_failure_is_not_retried() {
        let acpi = TestAcpiProtocol::new(efi::Status::OUT_OF_RESOURCES);
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![entry(b"image")]))),
            )
            .unwrap();
        assert_eq!(acpi.tables.borrow().len(), 1);
    }

    #[test]
    fn test_sbom_component_publish_propagates_installation_status() {
        let acpi = TestAcpiProtocol::new(efi::Status::OUT_OF_RESOURCES);
        let mut component = initialized_publisher(protocols(&acpi, vec![entry(b"image")]));
        assert_eq!(component.publish(), Err(EfiError::OutOfResources));
    }

    #[test]
    fn test_sbom_component_missing_acpi_protocol() {
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_handles().returning(|_| Ok(Vec::new()));
        protocols.expect_locate_interface().times(1).returning(|_| Err(ProtocolError::NotFound));
        let mut component = initialized_publisher(protocols);
        component.entries = entry(b"hob");
        assert_eq!(component.publish(), Err(EfiError::NotFound));
    }

    #[test]
    fn test_sbom_component_discovery_failures() {
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_handles().times(1).returning(|_| Err(ProtocolError::OutOfResources));
        let mut component = initialized_publisher(protocols);
        assert_eq!(component.publish(), Err(EfiError::OutOfResources));

        let mut protocols = MockProtocolServices::new();
        protocols
            .expect_locate_handles()
            .times(1)
            .returning(|_| Ok(vec![Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()]));
        protocols.expect_interface_on_handle().times(1).returning(|_, _| Err(ProtocolError::NotFound));
        component.protocols = Service::mock(Box::new(protocols));
        assert_eq!(component.publish(), Err(EfiError::NotFound));
    }

    #[test]
    fn test_sbom_component_event_registration_failure() {
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(1).returning(|_, _, _| Err(EventError::InvalidParameter));
        assert_eq!(
            publisher().entry_point(
                None,
                Service::mock(Box::new(events)),
                Service::mock(Box::new(MockProtocolServices::new()))
            ),
            Err(EfiError::InvalidParameter),
        );
    }

    #[test]
    fn test_sbom_component_table_length_limits() {
        let maximum_entries = u32::MAX as usize - size_of::<AcpiDescriptionHeader>();
        assert_eq!(table_length(maximum_entries), Ok(u32::MAX));
        assert_eq!(table_length(maximum_entries + 1), Err(EfiError::OutOfResources));
        assert_eq!(table_length(usize::MAX), Err(EfiError::OutOfResources));
    }

    #[test]
    fn test_sbom_component_event_close_reports_invalid_event() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(1).returning(|_, _, mut callback| {
            let event = Event::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap();
            callback(event);
            Ok(event)
        });
        events.expect_close_event().times(1).returning(|_| Err(EventError::InvalidParameter));
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events)),
                Service::mock(Box::new(protocols(&acpi, vec![entry(b"image")]))),
            )
            .unwrap();
        assert_eq!(acpi.tables.borrow().len(), 1);
    }
}
