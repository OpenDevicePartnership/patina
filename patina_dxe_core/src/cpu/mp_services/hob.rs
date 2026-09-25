//! `MpInformation2` HOB parsing for MP Services.
//!
//! For information on these HOBs see the EDK2 definitions.
//! - <https://github.com/tianocore/edk2/blob/master/UefiCpuPkg/Include/Guid/MpInformation2.h>
//! - <https://github.com/tianocore/edk2/blob/master/UefiCpuPkg/Library/MpInitLib/MpHandOff.h>
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;

use patina::component::hob::{FromHob, Hob};
use patina::standard::efi::protocols::mp_services;
use patina_internal_cpu::mp::{MpHandOffInfo, ProcessorHandOff};
use zerocopy::FromBytes;

/// One `EFI_PROCESSOR_INFORMATION` record as laid out at the start of each
/// `MP_INFORMATION2_ENTRY`. Read via `zerocopy`, then converted to the SDK's
/// [`mp_services::ProcessorInformation`] (which carries a union) afterward.
#[derive(Clone, Copy, zerocopy_derive::FromBytes)]
#[repr(C)]
struct RawProcessorInfo {
    processor_id: u64,
    status_flag: u32,
    /// package, core, thread.
    location: [u32; 3],
    /// package, module, tile, die, core, thread.
    location2: [u32; 6],
}

impl From<RawProcessorInfo> for mp_services::ProcessorInformation {
    fn from(r: RawProcessorInfo) -> Self {
        Self {
            processor_id: r.processor_id,
            status_flag: r.status_flag,
            location: mp_services::CpuPhysicalLocation {
                package: r.location[0],
                core: r.location[1],
                thread: r.location[2],
            },
            extended_information: mp_services::ExtendedProcessorInformation {
                location2: mp_services::CpuPhysicalLocation2 {
                    package: r.location2[0],
                    module: r.location2[1],
                    tile: r.location2[2],
                    die: r.location2[3],
                    core: r.location2[4],
                    thread: r.location2[5],
                },
            },
        }
    }
}

#[derive(Clone, Copy, zerocopy_derive::FromBytes)]
#[repr(C)]
struct MpInfo2Header {
    number_of_processors: u16,
    entry_size: u16,
    _version: u8,
    _reserved: [u8; 3],
    processor_index: u64,
}

/// EDK2's `MP_INFORMATION2_HOB_DATA` (`gMpInformation2HobGuid`)
///
/// See: <https://github.com/tianocore/edk2/blob/master/UefiCpuPkg/Include/Guid/MpInformation2.h>
pub(super) struct MpInformation2 {
    processor_index: usize,
    processors: Vec<mp_services::ProcessorInformation>,
}

impl MpInformation2 {
    pub(super) fn processor_index(&self) -> usize {
        self.processor_index
    }

    /// Per-processor information entries carried by this HOB instance.
    pub(super) fn processors(&self) -> &[mp_services::ProcessorInformation] {
        &self.processors
    }
}

impl FromHob for MpInformation2 {
    const HOB_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("417A7F64-F4E9-4B32-846A-5CC4D8621879");

    fn parse(bytes: &[u8]) -> Self {
        let Ok((header, _)) = MpInfo2Header::read_from_prefix(bytes) else {
            return Self { processor_index: 0, processors: Vec::new() };
        };
        // `EntrySize` is the stride between entries; fall back to the size of
        // `RawProcessorInfo` if the firmware reports a smaller/zero value.
        let entry_size = (header.entry_size as usize).max(core::mem::size_of::<RawProcessorInfo>());

        let mut processors = Vec::new();
        let mut off = core::mem::size_of::<MpInfo2Header>();
        for _ in 0..header.number_of_processors {
            let Some(entry) = bytes.get(off..) else { break };
            let Ok((raw, _)) = RawProcessorInfo::read_from_prefix(entry) else { break };
            processors.push(raw.into());
            off = match off.checked_add(entry_size) {
                Some(next) => next,
                None => break,
            };
        }
        Self { processor_index: header.processor_index as usize, processors }
    }
}

#[derive(Clone, Copy, zerocopy_derive::FromBytes)]
#[repr(C)]
struct RawProcessorHandOff {
    apic_id: u32,
    health: u32,
    startup_signal_address: u64,
    startup_procedure_address: u64,
}

/// EDK2's `MP_HAND_OFF` header (`gMpHandOffGuid`)
///
/// See: <https://github.com/tianocore/edk2/blob/master/UefiCpuPkg/Library/MpInitLib/MpHandOff.h>
#[derive(Clone, Copy, zerocopy_derive::FromBytes)]
#[repr(C)]
struct MpHandOffHeader {
    processor_index: u32,
    cpu_count: u32,
}

/// Parsed EDK2 `MP_HAND_OFF` (`gMpHandOffGuid`), the per-processor PEI-to-DXE
/// handoff records for the processor range this HOB instance describes.
pub(super) struct MpHandOff {
    processor_index: usize,
    processors: Vec<ProcessorHandOff>,
}

impl MpHandOff {
    pub(super) fn processor_index(&self) -> usize {
        self.processor_index
    }

    /// Per-processor handoff records carried by this HOB instance.
    pub(super) fn processors(&self) -> &[ProcessorHandOff] {
        &self.processors
    }
}

impl FromHob for MpHandOff {
    const HOB_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("11E2BD88-ED38-4ABD-A399-21F25FD07A60");

    fn parse(bytes: &[u8]) -> Self {
        let Ok((header, _)) = MpHandOffHeader::read_from_prefix(bytes) else {
            return Self { processor_index: 0, processors: Vec::new() };
        };

        let mut processors = Vec::new();
        let mut off = core::mem::size_of::<MpHandOffHeader>();
        for _ in 0..header.cpu_count {
            let Some(entry) = bytes.get(off..) else { break };
            let Ok((raw, _)) = RawProcessorHandOff::read_from_prefix(entry) else { break };
            processors.push(ProcessorHandOff {
                processor_id: raw.apic_id,
                healthy: raw.health == 0,
                startup_signal_address: raw.startup_signal_address,
                startup_procedure_address: raw.startup_procedure_address,
            });
            off = match off.checked_add(core::mem::size_of::<RawProcessorHandOff>()) {
                Some(next) => next,
                None => break,
            };
        }
        Self { processor_index: header.processor_index as usize, processors }
    }
}

/// EDK2's `MP_HAND_OFF_CONFIG` (`gMpHandOffConfigGuid`)
///
/// See: <https://github.com/tianocore/edk2/blob/master/UefiCpuPkg/Library/MpInitLib/MpHandOff.h>
#[derive(Clone, Copy, zerocopy_derive::FromBytes)]
#[repr(C)]
pub(super) struct MpHandOffConfig {
    pub wait_loop_execution_mode: u32,
    pub startup_signal_value: u32,
}

impl FromHob for MpHandOffConfig {
    const HOB_GUID: patina::BinaryGuid = patina::BinaryGuid::from_string("DABBD793-7B46-4144-8AD4-101C7C08EBFA");

    fn parse(bytes: &[u8]) -> Self {
        Self::read_from_prefix(bytes)
            .map_or(Self { wait_loop_execution_mode: 0, startup_signal_value: 0 }, |(config, _)| config)
    }
}

/// A wrapper structure for the HOBs parsed by the MP Services component.
///
/// Because these hobs need to be cross referenced for validation and handoff info,
/// it's convenient to parse them all at once and keep them together.
pub(super) struct MpHobs {
    pub processors: Vec<mp_services::ProcessorInformation>,
    processor_handoffs: Vec<ProcessorHandOff>,
    handoff_config: Option<MpHandOffConfig>,
}

impl MpHobs {
    pub fn parse_hobs(
        mp_info: Option<Hob<MpInformation2>>,
        mp_handoff: Option<Hob<MpHandOff>>,
        mp_handoff_config: Option<Hob<MpHandOffConfig>>,
    ) -> Self {
        let processor_handoffs: Vec<ProcessorHandOff> = mp_handoff
            .as_ref()
            .map(|hob| {
                Self::flatten_indexed(
                    hob.iter().map(|inst| (inst.processor_index(), inst.processors())).collect(),
                    "MP_HAND_OFF",
                )
            })
            .unwrap_or_default();

        let processors: Vec<mp_services::ProcessorInformation> = mp_info
            .as_ref()
            .map(|hob| {
                Self::flatten_indexed(
                    hob.iter().map(|inst| (inst.processor_index(), inst.processors())).collect(),
                    "MP_INFORMATION2",
                )
            })
            .unwrap_or_default();

        let handoff_config = mp_handoff_config.as_ref().and_then(|cfg| cfg.iter().next()).copied();

        Self { processors, processor_handoffs, handoff_config }
    }

    pub fn build_handoff(&self) -> Option<MpHandOffInfo<'_>> {
        self.handoff_config
            .as_ref()
            .map(|config| MpHandOffInfo {
                wait_loop_execution_mode: config.wait_loop_execution_mode,
                startup_signal_value: config.startup_signal_value,
                processors: &self.processor_handoffs,
            })
            .filter(|handoff| {
                if !self.processors.is_empty() && self.processors.len() != handoff.processors.len() {
                    log::error!(
                        "MP handoff describes {} processors but MP_INFORMATION2 describes {}; continuing with the BSP only",
                        handoff.processors.len(),
                        self.processors.len()
                    );
                    return false;
                }
                true
            })
    }

    /// Flattens a set of HOB instances into a single vector of values, sorted by processor index.
    fn flatten_indexed<T: Copy>(mut chunks: Vec<(usize, &[T])>, name: &str) -> Vec<T> {
        chunks.sort_unstable_by_key(|(index, _)| *index);
        let mut values = Vec::new();
        for (index, chunk) in chunks {
            if index != values.len() {
                log::error!("Invalid {name} HOB processor index {index}; expected {}", values.len());
                return Vec::new();
            }
            values.extend_from_slice(chunk);
        }
        values
    }
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn processor_info_bytes(processor_id: u64, status_flag: u32, location: [u32; 3], location2: [u32; 6]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&processor_id.to_ne_bytes());
        bytes.extend_from_slice(&status_flag.to_ne_bytes());
        for value in location.into_iter().chain(location2) {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        bytes
    }

    fn processor_info(processor_id: u64) -> mp_services::ProcessorInformation {
        RawProcessorInfo { processor_id, status_flag: 0, location: [0; 3], location2: [0; 6] }.into()
    }

    fn processor_handoff(processor_id: u32) -> ProcessorHandOff {
        ProcessorHandOff {
            processor_id,
            healthy: true,
            startup_signal_address: 0x1000,
            startup_procedure_address: 0x2000,
        }
    }

    #[test]
    fn test_mp_hob_rejects_noncontiguous_chunks() {
        let first = [10u32];
        let second = [12u32];

        assert!(MpHobs::flatten_indexed(vec![(0, &first), (2, &second)], "test").is_empty());
    }

    #[test]
    fn test_mp_handoff_converts_bist_health() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(&2u32.to_ne_bytes());
        for (apic_id, health) in [(1u32, 0u32), (2, 1)] {
            bytes.extend_from_slice(&apic_id.to_ne_bytes());
            bytes.extend_from_slice(&health.to_ne_bytes());
            bytes.extend_from_slice(&0x1000u64.to_ne_bytes());
            bytes.extend_from_slice(&0x2000u64.to_ne_bytes());
        }

        let handoff = MpHandOff::parse(&bytes);

        assert!(handoff.processors[0].healthy);
        assert!(!handoff.processors[1].healthy);
    }

    #[test]
    fn test_mp_information2_parses_topology_with_entry_stride() {
        let entry = processor_info_bytes(0x2A, 0x55, [1, 2, 3], [4, 5, 6, 7, 8, 9]);
        let entry_size = entry.len() + 8;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u16.to_ne_bytes());
        bytes.extend_from_slice(&(entry_size as u16).to_ne_bytes());
        bytes.extend_from_slice(&[1, 0, 0, 0]);
        bytes.extend_from_slice(&3u64.to_ne_bytes());
        bytes.extend_from_slice(&entry);
        bytes.extend_from_slice(&[0xA5; 8]);

        let info = MpInformation2::parse(&bytes);
        let processor = info.processors()[0];

        assert_eq!(info.processor_index(), 3);
        assert_eq!(processor.processor_id, 0x2A);
        assert_eq!(processor.status_flag, 0x55);
        assert_eq!(processor.location.package, 1);
        assert_eq!(processor.location.core, 2);
        assert_eq!(processor.location.thread, 3);
        // SAFETY: `RawProcessorInfo::from` initializes the `location2` union member.
        let location2 = unsafe { processor.extended_information.location2 };
        assert_eq!(location2.package, 4);
        assert_eq!(location2.module, 5);
        assert_eq!(location2.tile, 6);
        assert_eq!(location2.die, 7);
        assert_eq!(location2.core, 8);
        assert_eq!(location2.thread, 9);
    }

    #[test]
    fn test_mp_information2_rejects_truncated_header() {
        let info = MpInformation2::parse(&[0; core::mem::size_of::<MpInfo2Header>() - 1]);

        assert_eq!(info.processor_index(), 0);
        assert!(info.processors().is_empty());
    }

    #[test]
    fn test_mp_handoff_ignores_truncated_entry() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4u32.to_ne_bytes());
        bytes.extend_from_slice(&2u32.to_ne_bytes());
        bytes.extend_from_slice(&7u32.to_ne_bytes());
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(&0x1000u64.to_ne_bytes());
        bytes.extend_from_slice(&0x2000u64.to_ne_bytes());
        bytes.extend_from_slice(&[0; core::mem::size_of::<RawProcessorHandOff>() - 1]);

        let handoff = MpHandOff::parse(&bytes);

        assert_eq!(handoff.processor_index(), 4);
        assert_eq!(handoff.processors().len(), 1);
        assert_eq!(handoff.processors()[0].processor_id, 7);
    }

    #[test]
    fn test_mp_handoff_rejects_truncated_header() {
        let handoff = MpHandOff::parse(&[0; core::mem::size_of::<MpHandOffHeader>() - 1]);

        assert_eq!(handoff.processor_index(), 0);
        assert!(handoff.processors().is_empty());
    }

    #[test]
    fn test_mp_handoff_config_parses_and_defaults() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u32.to_ne_bytes());
        bytes.extend_from_slice(&0xA5A5u32.to_ne_bytes());

        let config = MpHandOffConfig::parse(&bytes);
        assert_eq!(config.wait_loop_execution_mode, 8);
        assert_eq!(config.startup_signal_value, 0xA5A5);

        let default = MpHandOffConfig::parse(&bytes[..bytes.len() - 1]);
        assert_eq!(default.wait_loop_execution_mode, 0);
        assert_eq!(default.startup_signal_value, 0);
    }

    #[test]
    fn test_mp_hobs_rejects_mismatched_processor_counts() {
        let hobs = MpHobs {
            processors: vec![processor_info(0)],
            processor_handoffs: vec![processor_handoff(0), processor_handoff(1)],
            handoff_config: Some(MpHandOffConfig { wait_loop_execution_mode: 8, startup_signal_value: 1 }),
        };

        assert!(hobs.build_handoff().is_none());
    }

    #[test]
    fn test_mp_hobs_parse_and_build_handoff() {
        let info = Hob::mock(vec![
            MpInformation2 { processor_index: 1, processors: vec![processor_info(1)] },
            MpInformation2 { processor_index: 0, processors: vec![processor_info(0)] },
        ]);
        let handoff = Hob::mock(vec![
            MpHandOff { processor_index: 1, processors: vec![processor_handoff(1)] },
            MpHandOff { processor_index: 0, processors: vec![processor_handoff(0)] },
        ]);
        let config = Hob::mock(vec![MpHandOffConfig { wait_loop_execution_mode: 8, startup_signal_value: 0xA5 }]);

        let hobs = MpHobs::parse_hobs(Some(info), Some(handoff), Some(config));
        let built = hobs.build_handoff().expect("matching HOBs should produce a handoff");

        assert_eq!(hobs.processors.len(), 2);
        assert_eq!(hobs.processors[0].processor_id, 0);
        assert_eq!(hobs.processors[1].processor_id, 1);
        assert_eq!(built.wait_loop_execution_mode, 8);
        assert_eq!(built.startup_signal_value, 0xA5);
        assert_eq!(built.processors.len(), 2);
        assert_eq!(built.processors[0].processor_id, 0);
        assert_eq!(built.processors[1].processor_id, 1);
    }

    #[test]
    fn test_mp_hobs_without_inputs_has_no_handoff() {
        let hobs = MpHobs::parse_hobs(None, None, None);

        assert!(hobs.processors.is_empty());
        assert!(hobs.processor_handoffs.is_empty());
        assert!(hobs.build_handoff().is_none());
    }
}
