//! Component publishing an SBOM ACPI table at Ready to Boot.
//!
//! Copyright (C) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use crate::hob::SbomHob;
use alloc::vec::Vec;
use core::mem::size_of;
use patina::{
    component::{
        component,
        hob::Hob,
        service::{
            Service,
            performance::PerformanceManager,
            uefi_services::{
                event::{EventServices, EventServicesExt, Tpl},
                protocol::{ProtocolServices, ProtocolServicesExt},
            },
        },
    },
    error::EfiError,
    guid::CALLER_ID,
    uefi::acpi_table::{
        ACPI_CHECKSUM_OFFSET, AcpiDescriptionHeader,
        sbom::{EFI_ACPI_SBOM_TABLE_REVISION, EFI_ACPI_SBOM_TABLE_SIGNATURE},
    },
    uefi::{acpi_table::AcpiTableProtocol, event::READY_TO_BOOT_EVENT_GROUP_GUID},
    vendor_protocols::sbom::{PROTOCOL_GUID as SBOM_PROTOCOL_GUID, SbomProtocol},
};
use zerocopy::IntoBytes;

const DEDUPLICATION_MEASUREMENT: &str = "SbomDeduplication";

/// Publishes a combined ACPI SBOM table once, at Ready to Boot.
pub struct SbomComponent {
    header: AcpiDescriptionHeader,
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
        }
    }

    fn entry_point(
        self,
        hobs: Option<Hob<SbomHob>>,
        events: Service<dyn EventServices>,
        protocols: Service<dyn ProtocolServices>,
        performance: Option<Service<dyn PerformanceManager>>,
    ) -> Result<(), EfiError> {
        let mut entries = Vec::new();
        if let Some(hobs) = hobs {
            for hob in &hobs {
                append_entry(&mut entries, hob.entry().as_bytes())?;
            }
        }
        events.on_event_group_self_managed(READY_TO_BOOT_EVENT_GROUP_GUID, Tpl::Callback, move |event| {
            if let Err(error) = publish(self.header, &mut entries, &protocols, performance.as_ref()) {
                log::error!("SBOM: failed to publish ACPI table: {error}");
            }
            if let Err(error) = events.close_event(event) {
                log::error!("SBOM: failed to close Ready to Boot event: {error:?}");
            }
        })?;
        Ok(())
    }
}

fn publish(
    header: AcpiDescriptionHeader,
    entries: &mut Vec<Vec<u8>>,
    protocols: &Service<dyn ProtocolServices>,
    performance: Option<&Service<dyn PerformanceManager>>,
) -> Result<(), EfiError> {
    for handle in protocols.locate_handles(SBOM_PROTOCOL_GUID)? {
        let interface = protocols.interface_on_handle(handle, SBOM_PROTOCOL_GUID)?;
        // SAFETY: The SBOM provider guarantees a readable header and complete declared payload. At TPL Callback,
        // image-unload application code cannot run while this immediate copy is performed.
        // The pointer is used only here and is never retained after the notification.
        match unsafe { SbomProtocol::from_raw(interface.as_raw().cast()) } {
            Ok(entry) => append_entry(entries, entry.as_bytes())?,
            Err(error) => log::warn!("SBOM: ignoring invalid entry on handle {handle:?}: {error}"),
        }
    }
    if entries.is_empty() {
        return Ok(());
    }
    if let Some(performance) = performance {
        performance.perf_in_module_begin(DEDUPLICATION_MEASUREMENT, CALLER_ID.as_efi_guid());
    }
    entries.sort_unstable();
    entries.dedup();
    if let Some(performance) = performance {
        performance.perf_in_module_end(DEDUPLICATION_MEASUREMENT, CALLER_ID.as_efi_guid());
    }
    let table = build_table(header, entries)?;
    protocols.with_protocol::<AcpiTableProtocol, _>(|protocol| protocol.install(&table).map(|_| ()))?
}

fn build_table(mut header: AcpiDescriptionHeader, entries: &[Vec<u8>]) -> Result<Vec<u8>, EfiError> {
    let entries_length = entries
        .iter()
        .try_fold(0usize, |length, entry| length.checked_add(entry.len()).ok_or(EfiError::OutOfResources))?;
    let length = table_length(entries_length)?;
    header.length = length.to_le();
    header.checksum = 0;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length as usize).map_err(|_| EfiError::OutOfResources)?;
    bytes.extend_from_slice(header.as_bytes());
    for entry in entries {
        bytes.extend_from_slice(entry);
    }
    let checksum = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    *bytes.get_mut(ACPI_CHECKSUM_OFFSET).ok_or(EfiError::LoadError)? = checksum;
    Ok(bytes)
}

fn append_entry(entries: &mut Vec<Vec<u8>>, entry: &[u8]) -> Result<(), EfiError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(entry.len()).map_err(|_| EfiError::OutOfResources)?;
    bytes.extend_from_slice(entry);
    entries.try_reserve(1).map_err(|_| EfiError::OutOfResources)?;
    entries.push(bytes);
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
    use mockall::Sequence;
    use patina::{
        component::{
            hob::FromHob,
            service::{
                performance::MockPerformanceManager,
                uefi_services::{
                    event::{Event, EventError, MockEventServices},
                    protocol::{Handle, MockProtocolServices, ProtocolError, ProtocolPtr},
                },
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
            protocols.expect_interface_on_handle().times(count).returning_st(move |handle, guid| {
                assert_eq!(guid, SBOM_PROTOCOL_GUID);
                let index = handle.as_raw() as usize - 1;
                Ok(ProtocolPtr::from_raw(entries[index].as_ptr().cast_mut().cast()).unwrap())
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
                None,
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
                None,
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b"hob"]);
    }

    #[test]
    fn test_sbom_component_deduplicates_hobs_and_protocols() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        publisher()
            .entry_point(
                Some(Hob::mock(vec![
                    SbomHob::parse(&entry(b"bbb")).unwrap(),
                    SbomHob::parse(&entry(b"aaa")).unwrap(),
                    SbomHob::parse(&entry(b"bbb")).unwrap(),
                ])),
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(
                    &acpi,
                    vec![entry(b"ccc"), entry(b"bbb"), entry(b"ccc"), entry(b"aaa")],
                ))),
                None,
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b"aaa", b"bbb", b"ccc"]);
    }

    #[test]
    fn test_sbom_component_deduplicates_empty_payloads() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        publisher()
            .entry_point(
                Some(Hob::mock(vec![SbomHob::parse(&entry(b"")).unwrap()])),
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![entry(b""), entry(b"")]))),
                None,
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b""]);
    }

    #[test]
    fn test_sbom_component_deduplication_preserves_distinct_metadata() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let original = entry(b"same");
        let mut flags = original.clone();
        flags[7] = 1;
        let mut format = original.clone();
        format[8] = 2;
        let mut compression = original.clone();
        compression[9] = 2;
        let mut expected = vec![original, flags, format, compression];
        let mut inputs = expected.clone();
        inputs.extend(expected.clone());
        inputs.reverse();
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, inputs))),
                None,
            )
            .unwrap();
        expected.sort_unstable();
        let tables = acpi.tables.borrow();
        assert_table(&tables[0], &[b"same", b"same", b"same", b"same"]);
        let (table, _) = SbomTable::parse_prefix(&tables[0]).unwrap();
        let actual: Vec<_> = table.entries().map(|entry| entry.unwrap().as_bytes().to_vec()).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_sbom_component_protocol_only_with_empty_payload() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![entry(b"")]))),
                None,
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b""]);
    }

    #[test]
    fn test_sbom_component_measures_deduplication() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let mut performance = MockPerformanceManager::new();
        let mut sequence = Sequence::new();
        performance
            .expect_perf_in_module_begin()
            .withf(|name, caller| name == DEDUPLICATION_MEASUREMENT && caller == CALLER_ID.as_efi_guid())
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(());
        performance
            .expect_perf_in_module_end()
            .withf(|name, caller| name == DEDUPLICATION_MEASUREMENT && caller == CALLER_ID.as_efi_guid())
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(());
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![entry(b"image"), entry(b"image")]))),
                Some(Service::mock(Box::new(performance))),
            )
            .unwrap();
        assert_table(&acpi.tables.borrow()[0], &[b"image"]);
    }

    #[test]
    fn test_sbom_component_no_entries_skips_installation() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_handles().times(1).returning(|_| Ok(Vec::new()));
        publisher()
            .entry_point(None, Service::mock(Box::new(events())), Service::mock(Box::new(protocols)), None)
            .unwrap();
        assert!(acpi.tables.borrow().is_empty());
    }

    #[test]
    fn test_sbom_component_malformed_entries_are_skipped() {
        let acpi = TestAcpiProtocol::new(efi::Status::SUCCESS);
        let mut bad_revision = entry(b"revision");
        bad_revision[0] = 0;
        let mut bad_header = entry(b"header");
        bad_header[1] = 0;
        publisher()
            .entry_point(
                None,
                Service::mock(Box::new(events())),
                Service::mock(Box::new(protocols(&acpi, vec![bad_revision, bad_header, entry(b"valid")]))),
                None,
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
                None,
            )
            .unwrap();
        assert_eq!(acpi.tables.borrow().len(), 1);
    }

    #[test]
    fn test_sbom_component_publish_propagates_installation_status() {
        let acpi = TestAcpiProtocol::new(efi::Status::OUT_OF_RESOURCES);
        let protocols = Service::mock(Box::new(protocols(&acpi, vec![entry(b"image")])) as Box<dyn ProtocolServices>);
        assert_eq!(publish(publisher().header, &mut Vec::new(), &protocols, None), Err(EfiError::OutOfResources));
    }

    #[test]
    fn test_sbom_component_missing_acpi_protocol() {
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_handles().returning(|_| Ok(Vec::new()));
        protocols.expect_locate_interface().times(1).returning(|_| Err(ProtocolError::NotFound));
        let protocols = Service::mock(Box::new(protocols) as Box<dyn ProtocolServices>);
        assert_eq!(publish(publisher().header, &mut vec![entry(b"hob")], &protocols, None), Err(EfiError::NotFound));
    }

    #[test]
    fn test_sbom_component_discovery_failures() {
        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_handles().times(1).returning(|_| Err(ProtocolError::OutOfResources));
        let protocols = Service::mock(Box::new(protocols) as Box<dyn ProtocolServices>);
        assert_eq!(publish(publisher().header, &mut Vec::new(), &protocols, None), Err(EfiError::OutOfResources));

        let mut protocols = MockProtocolServices::new();
        protocols
            .expect_locate_handles()
            .times(1)
            .returning(|_| Ok(vec![Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()]));
        protocols.expect_interface_on_handle().times(1).returning(|_, _| Err(ProtocolError::NotFound));
        let protocols = Service::mock(Box::new(protocols) as Box<dyn ProtocolServices>);
        assert_eq!(publish(publisher().header, &mut Vec::new(), &protocols, None), Err(EfiError::NotFound));
    }

    #[test]
    fn test_sbom_component_event_registration_failure() {
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().times(1).returning(|_, _, _| Err(EventError::InvalidParameter));
        assert_eq!(
            publisher().entry_point(
                None,
                Service::mock(Box::new(events)),
                Service::mock(Box::new(MockProtocolServices::new())),
                None
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
                None,
            )
            .unwrap();
        assert_eq!(acpi.tables.borrow().len(), 1);
    }
}
