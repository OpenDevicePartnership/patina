//! Firmware Policy Boundary Audit
//!
//! Reports, once when the policy is installed, whether the platform policy
//! grants Ring 3 write access to a register or port that defines the boundary
//! between Ring 0 and Ring 3.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use super::{AccessType, IoWidth, gate::PolicyGate};

/// Reports every boundary-defining MSR and I/O port the policy grants Ring 3 write access to,
/// returning how many it found in total.
///
/// Both halves always run, so the log names every problem in the policy rather than only the
/// first kind found. See [`audit_boundary_msr_grants`] and [`audit_boundary_io_grants`] for what
/// each half covers and why those registers and ports were chosen.
pub(crate) fn audit_boundary_grants(gate: &PolicyGate) -> usize {
    audit_boundary_msr_grants(gate) + audit_boundary_io_grants(gate)
}

/// MSRs whose write access defines the boundary between Ring 0 and Ring 3, as inclusive
/// `(first, last, description)` ranges.
///
/// The list is used for reporting only. The platform policy remains the one place that decides
/// what Ring 3 may access. Write access to any of these defeats Ring 3 isolation: `LSTAR` sets
/// the syscall entry address the CPU calls in Ring 0, `KERNEL_GS_BASE` sets the Ring 0 stack the
/// entry stub switches to, `IA32_PL0_SSP` relocates the supervisor shadow stack, `SMRR_PHYS*`
/// unlock MMRAM, clearing `EFER.NXE` disables NX enforcement, the MTRR and `PAT` registers allow
/// cache poisoning of MMRAM, and `IA32_DS_AREA` and `IA32_RTIT_CTL` direct trace stores to an
/// arbitrary address.
///
/// Registers a platform has reason to write are left out even when they are security relevant,
/// `IA32_APIC_BASE` and `IA32_FEATURE_CONTROL` among them, so that a normal policy reports
/// nothing.
const BOUNDARY_MSRS: &[(u32, u32, &str)] = &[
    (0x0000_009E, 0x0000_009E, "IA32_SMBASE"),
    (0x0000_0174, 0x0000_0176, "IA32_SYSENTER_CS/ESP/EIP"),
    (0x0000_01D9, 0x0000_01D9, "IA32_DEBUGCTL"),
    (0x0000_01F2, 0x0000_01F3, "SMRR_PHYSBASE/SMRR_PHYSMASK"),
    (0x0000_0200, 0x0000_020F, "variable-range MTRRs"),
    (0x0000_0250, 0x0000_0250, "MTRR_FIX64K_00000"),
    (0x0000_0258, 0x0000_0259, "MTRR_FIX16K_80000/A0000"),
    (0x0000_0268, 0x0000_026F, "fixed-range MTRRs"),
    (0x0000_0277, 0x0000_0277, "IA32_PAT"),
    (0x0000_02FF, 0x0000_02FF, "IA32_MTRR_DEF_TYPE"),
    (0x0000_04E0, 0x0000_04E0, "MSR_SMM_FEATURE_CONTROL"),
    (0x0000_0570, 0x0000_0570, "IA32_RTIT_CTL"),
    (0x0000_0600, 0x0000_0600, "IA32_DS_AREA"),
    (0x0000_06A0, 0x0000_06A0, "IA32_U_CET"),
    (0x0000_06A2, 0x0000_06A2, "IA32_S_CET"),
    (0x0000_06A4, 0x0000_06A8, "IA32_PL0_SSP..IA32_PL3_SSP/IA32_INTERRUPT_SSP_TABLE_ADDR"),
    (0x0000_0DA0, 0x0000_0DA0, "IA32_XSS"),
    (0xC000_0080, 0xC000_0084, "IA32_EFER/STAR/LSTAR/CSTAR/FMASK"),
    (0xC000_0101, 0xC000_0102, "IA32_GS_BASE/KERNEL_GS_BASE"),
];

/// Reports every [`BOUNDARY_MSRS`] register the policy grants Ring 3 write access to, returning
/// how many it found.
///
/// This is a diagnostic and denies nothing. It runs once, when the policy is installed, and
/// queries the same gate the syscall path uses, so the policy stays the only place that decides
/// what Ring 3 may access. Running it at initialization reports the result on every boot, rather
/// than when some driver issues the `WRMSR`.
///
/// A non-zero result means the platform policy does not maintain the Ring 0 / Ring 3 boundary and
/// needs to be corrected. The supervisor continues, because the platform policy, not this list,
/// is the authority on what a platform needs.
fn audit_boundary_msr_grants(gate: &PolicyGate) -> usize {
    let mut granted = 0;

    for &(first, last, description) in BOUNDARY_MSRS {
        for msr in first..=last {
            if gate.is_msr_allowed(msr, AccessType::Write).is_ok() {
                log::error!(
                    "MM policy grants Ring 3 write access to MSR 0x{msr:08x} ({description}). This register defines \
                     the boundary between Ring 0 and Ring 3; granting it defeats Ring 3 isolation. Remove the entry \
                     from the platform MM policy."
                );
                granted += 1;
            }
        }
    }

    if granted == 0 {
        log::info!("MM policy audit: no boundary-defining MSR is writable from Ring 3");
    } else {
        log::error!("MM policy audit: {granted} boundary-defining MSR(s) are writable from Ring 3");
    }

    granted
}

/// I/O ports whose write access defines the boundary between Ring 0 and Ring 3, as inclusive
/// `(first, last, description)` ranges.
///
/// Used for reporting only, as [`BOUNDARY_MSRS`] is. Write access to PCI configuration space
/// reaches the chipset registers that lock MMRAM and program the DRAM remap windows, so it can
/// unlock MMRAM or alias normal memory over it without any `WRMSR`. Write access to the ACPI
/// software MMI command port lets a demoted driver supply the command value the MMI dispatchers
/// match on, so handlers run as though the request came from outside MM.
///
/// Ports a platform has reason to drive from MM are left out, the embedded controller, CMOS,
/// GPIO and the ACPI PM block among them, so that a normal policy reports nothing.
const BOUNDARY_IO_PORTS: &[(u16, u16, &str)] =
    &[(0x00B2, 0x00B3, "ACPI software MMI command/data port"), (0x0CF8, 0x0CFF, "PCI configuration address/data")];

/// Reports every [`BOUNDARY_IO_PORTS`] port the policy grants Ring 3 write access to, returning
/// how many it found.
///
/// The I/O counterpart to [`audit_boundary_msr_grants`], with the same behavior: it runs once
/// when the policy is installed, queries the gate, and changes nothing. Ports are checked one
/// byte at a time, which is the granularity at which the policy describes them.
fn audit_boundary_io_grants(gate: &PolicyGate) -> usize {
    let mut granted = 0;

    for &(first, last, description) in BOUNDARY_IO_PORTS {
        for port in first..=last {
            if gate.is_io_allowed(u32::from(port), IoWidth::Byte, AccessType::Write).is_ok() {
                log::error!(
                    "MM policy grants Ring 3 write access to I/O port 0x{port:04x} ({description}). This port \
                     reaches the configuration that defines the MM boundary; granting it defeats Ring 3 isolation. \
                     Remove the entry from the platform MM policy."
                );
                granted += 1;
            }
        }
    }

    if granted == 0 {
        log::info!("MM policy audit: no boundary-defining I/O port is writable from Ring 3");
    } else {
        log::error!("MM policy audit: {granted} boundary-defining I/O port(s) are writable from Ring 3");
    }

    granted
}
