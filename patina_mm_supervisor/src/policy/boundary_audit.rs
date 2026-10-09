//! Firmware Policy Boundary Audit
//!
//! Reports, once when the policy is installed, whether the platform policy grants Ring 3 write
//! access to a register or port that defines the privilege boundary the supervisor rests on.
//!
//! The audit decides nothing. It asks [`crate::policy::gate`] the same questions a
//! syscall would ask and logs what it finds, so the supervisor keeps one place that decides what
//! Ring 3 may do. A finding means the platform policy needs correcting, and initialization
//! continues either way. That is what separates this from
//! [`crate::policy::validation`], which rejects a policy outright and stops the
//! supervisor.
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

/// MSRs whose write access defines the privilege boundary the supervisor rests on, as inclusive
/// `(first, last, description)` ranges.
///
/// The platform policy decides what Ring 3 may touch; this list decides nothing. It exists so the
/// supervisor can say, once, whether the policy it was handed still leaves a boundary to enforce.
/// Granting Ring 3 a write here does not weaken isolation, it removes it: `LSTAR` repoints the
/// syscall entry at attacker-chosen code that the CPU then runs in Ring 0, `KERNEL_GS_BASE`
/// chooses the Ring 0 stack the entry stub switches to, `IA32_PL0_SSP` relocates the supervisor
/// shadow stack, `SMRR_PHYS*` unlock MMRAM, clearing `EFER.NXE` disables NX enforcement, the MTRR
/// and `PAT` registers enable cache-poisoning attacks on MMRAM, and `IA32_DS_AREA` and
/// `IA32_RTIT_CTL` aim trace stores at an arbitrary address.
///
/// Registers a real platform has cause to write are deliberately absent even when they are
/// security-relevant - `IA32_APIC_BASE` and `IA32_FEATURE_CONTROL` among them - so that a normal
/// policy does not produce noise here.
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
/// This is a diagnostic, not a gate. It runs once, when the policy is installed, and asks the
/// policy its own question rather than second-guessing it, so the supervisor keeps exactly one
/// place that decides what Ring 3 may do. Reporting at initialization also puts the finding in
/// front of the platform author deterministically on every boot, instead of waiting for whichever
/// driver happens to issue the offending `WRMSR`.
///
/// A non-zero result means the platform policy has voided the Ring 0 / Ring 3 boundary and needs
/// to be corrected; the supervisor continues, because a list of registers compiled into the
/// supervisor is not a better authority on a platform's needs than its reviewed policy.
fn audit_boundary_msr_grants(gate: &PolicyGate) -> usize {
    let mut granted = 0;

    for &(first, last, description) in BOUNDARY_MSRS {
        for msr in first..=last {
            if gate.is_msr_allowed(msr, AccessType::Write).is_ok() {
                log::error!(
                    "MM policy grants Ring 3 write access to MSR 0x{msr:08x} ({description}). This register defines \
                     the supervisor's privilege boundary; granting it makes Ring 3 isolation unenforceable. Remove \
                     the entry from the platform MM policy."
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

/// I/O ports whose write access defines the privilege boundary the supervisor rests on, as
/// inclusive `(first, last, description)` ranges.
///
/// Same contract as [`BOUNDARY_MSRS`]: this list decides nothing, it reports. Granting Ring 3
/// write access to PCI configuration space reaches the chipset registers that lock MMRAM and
/// program the DRAM remap windows, so it can unlock MMRAM or alias normal memory over it without
/// ever issuing a `WRMSR`; granting the ACPI software MMI command port lets a demoted driver
/// forge the command value the MMI dispatchers key on, invoking handlers as though the request
/// came from outside MM.
///
/// Ports a real platform has cause to drive from MM are deliberately absent - the embedded
/// controller, CMOS, GPIO and the ACPI PM block among them - so that a normal policy does not
/// produce noise here.
const BOUNDARY_IO_PORTS: &[(u16, u16, &str)] =
    &[(0x00B2, 0x00B3, "ACPI software MMI command/data port"), (0x0CF8, 0x0CFF, "PCI configuration address/data")];

/// Reports every [`BOUNDARY_IO_PORTS`] port the policy grants Ring 3 write access to, returning
/// how many it found.
///
/// The companion to [`audit_boundary_msr_grants`], and a diagnostic on the same terms: it runs
/// once when the policy is installed, asks the policy its own question, and changes nothing.
/// Ports are probed one byte at a time, which is the granularity at which the policy describes
/// them.
fn audit_boundary_io_grants(gate: &PolicyGate) -> usize {
    let mut granted = 0;

    for &(first, last, description) in BOUNDARY_IO_PORTS {
        for port in first..=last {
            if gate.is_io_allowed(u32::from(port), IoWidth::Byte, AccessType::Write).is_ok() {
                log::error!(
                    "MM policy grants Ring 3 write access to I/O port 0x{port:04x} ({description}). This port \
                     reaches the configuration that defines the MM boundary; granting it makes Ring 3 isolation \
                     unenforceable. Remove the entry from the platform MM policy."
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::policy::{
        ACCESS_ATTR_ALLOW, RESOURCE_ATTR_READ, RESOURCE_ATTR_WRITE,
        test_support::{Descriptors, PolicyBuilder, io, msr},
    };

    const WRITE: u16 = RESOURCE_ATTR_WRITE as u16;
    const READ_WRITE: u16 = (RESOURCE_ATTR_READ | RESOURCE_ATTR_WRITE) as u16;

    #[test]
    fn test_boundary_msr_audit_passes_a_policy_modelled_on_a_shipping_platform() {
        // The MSR grants from a real platform MM policy, including the security-relevant ones the
        // list deliberately leaves to the platform. None of these may be reported, or the audit is
        // noise that platform authors will learn to ignore.
        let granted = [
            msr(0x0000_001B, 4, READ_WRITE), // IA32_APIC_BASE
            msr(0x0000_003A, 1, READ_WRITE), // IA32_FEATURE_CONTROL
            msr(0x0000_00FE, 1, READ_WRITE), // IA32_MTRRCAP
            msr(0x0000_01A0, 1, READ_WRITE), // IA32_MISC_ENABLE
            msr(0x0000_01FE, 2, READ_WRITE), // chipset-specific, directly below the variable MTRRs
            msr(0x0000_0830, 1, READ_WRITE), // x2APIC interrupt command
        ];
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Msr(granted.to_vec())).build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 0);
    }

    #[test]
    fn test_boundary_msr_audit_reports_a_policy_that_grants_the_syscall_entry() {
        // An allow range that starts at IA32_EFER and runs over the syscall MSRs - the shape of
        // policy mistake the audit exists to surface.
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(0xC000_0080, 5, READ_WRITE)]))
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 5);
    }

    #[test]
    fn test_boundary_msr_audit_reports_each_granted_register_once() {
        let policy = PolicyBuilder::new()
            .root(
                ACCESS_ATTR_ALLOW,
                Descriptors::Msr(vec![
                    msr(0x0000_01F2, 2, WRITE),      // both SMRR registers
                    msr(0x0000_06A4, 1, READ_WRITE), // IA32_PL0_SSP alone
                ]),
            )
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 3);
    }

    #[test]
    fn test_boundary_msr_audit_ignores_read_only_grants() {
        // Reads are the platform's call; only a write voids the boundary.
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(0xC000_0080, 5, RESOURCE_ATTR_READ as u16)]))
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 0);
    }

    #[test]
    fn test_boundary_msr_audit_covers_the_syscall_flag_mask() {
        // `IA32_FMASK` is what clears DF and AC when Ring 3 enters Ring 0; a policy that lets
        // Ring 3 write it hands back the flags the supervisor runs under.
        const IA32_FMASK: u32 = 0xC000_0084;

        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(IA32_FMASK, 1, READ_WRITE)]))
            .build();

        assert_eq!(audit_boundary_msr_grants(&policy.gate()), 1);
    }

    #[test]
    fn test_boundary_io_audit_stays_quiet_on_ports_a_platform_legitimately_drives() {
        let granted = [
            io(0x0062, 2, READ_WRITE),    // embedded controller
            io(0x0070, 2, READ_WRITE),    // CMOS index/data
            io(0x0400, 0x40, READ_WRITE), // an ACPI PM block
            io(0x0CF9, 1, READ_WRITE),    // reset control, inside the PCI config window but not it
        ];
        let policy = PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(granted.to_vec())).build();

        // 0xCF9 sits within the audited 0xCF8-0xCFF span, so it is reported; the rest are not.
        assert_eq!(audit_boundary_io_grants(&policy.gate()), 1);
    }

    #[test]
    fn test_boundary_io_audit_reports_a_policy_that_grants_pci_config_space() {
        let policy =
            PolicyBuilder::new().root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x0CF8, 8, READ_WRITE)])).build();

        assert_eq!(audit_boundary_io_grants(&policy.gate()), 8);
    }

    #[test]
    fn test_boundary_io_audit_ignores_read_only_grants() {
        // Reading PCI config space does not move the boundary; writing it does.
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x0CF8, 8, RESOURCE_ATTR_READ as u16)]))
            .build();

        assert_eq!(audit_boundary_io_grants(&policy.gate()), 0);
    }

    #[test]
    fn test_boundary_audit_totals_both_halves() {
        let policy = PolicyBuilder::new()
            .root(ACCESS_ATTR_ALLOW, Descriptors::Msr(vec![msr(0xC000_0080, 5, READ_WRITE)]))
            .root(ACCESS_ATTR_ALLOW, Descriptors::Io(vec![io(0x0CF8, 8, READ_WRITE)]))
            .build();

        // The five syscall MSRs plus the eight PCI configuration ports.
        assert_eq!(audit_boundary_grants(&policy.gate()), 13);
    }

    #[test]
    fn test_boundary_audit_reports_nothing_for_an_empty_policy() {
        let policy = PolicyBuilder::new().build();

        assert_eq!(audit_boundary_grants(&policy.gate()), 0);
    }
}
