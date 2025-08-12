//! UEFI Dependency Expression (DEPEX) support
//!
//! This module provides a parser and evaluator for UEFI dependency expressions.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![no_std]
#![feature(coverage_attribute)]

extern crate alloc;

use alloc::vec::Vec;
use core::mem;
use r_efi::efi;
use uuid::Uuid;

/// The size of a GUID in bytes
const GUID_SIZE: usize = mem::size_of::<r_efi::efi::Guid>();

/// The initial size of the dependency expression stack in bytes
const DEPEX_STACK_SIZE_INCREMENT: usize = 0x100;

/// A UEFI dependency expression (DEPEX) opcode
#[derive(Debug, Clone, PartialEq)]
pub enum Opcode {
    /// If present, this must be the first and only opcode,
    /// may be used by DXE and SMM drivers.
    Before(Uuid),
    /// If present, this must be the first and only opcode,
    /// may be used by DXE and SMM drivers.
    After(Uuid),
    /// A Push opcode is followed by a GUID.
    Push(Uuid, bool),
    /// A logical AND operation of the two operands on the top
    /// of the stack.
    And,
    /// A logical OR operation of the two operands on the top
    /// of the stack.
    Or,
    /// A logical NOT operation of the operand on the top of
    /// the stack.
    Not,
    /// Pushes a true value onto the stack.
    True,
    /// Pushes a false value onto the stack.
    False,
    /// The End opcode is the last opcode in the expression.
    End,
    /// If present, this must be the first opcode in the expression.
    /// Used to schedule on request.
    Sor,
    /// An unknown opcode. Indicates an unrecognized opcode
    /// that should be treated as an error during evaluation.
    Unknown,
    /// A known opcode with an unexpected payload length.
    Malformed {
        /// The unhandled opcode value.
        opcode: u8,
        /// The length of the payload sent with the opcode.
        len: usize,
    },
}

/// Converts a UUID to an EFI GUID.
fn guid_from_uuid(uuid: &Uuid) -> Option<efi::Guid> {
    let fields = uuid.as_fields();
    let node = &fields.3[2..].try_into().ok()?;
    Some(efi::Guid::from_fields(fields.0, fields.1, fields.2, fields.3[0], fields.3[1], node))
}

/// Converts a byte slice to a GUID.
fn uuid_from_slice(slice: Option<&[u8]>) -> Option<Uuid> {
    Uuid::from_slice_le(slice?).ok()
}

impl<'a> From<&'a [u8]> for Opcode {
    /// Creates an Opcode from a byte slice.
    fn from(bytes: &'a [u8]) -> Self {
        match bytes[0] {
            0x00 => match uuid_from_slice(bytes.get(1..GUID_SIZE + 1)) {
                Some(uuid) => Opcode::Before(uuid),
                None => Opcode::Malformed { opcode: 0x00, len: bytes.len() - 1 },
            },
            0x01 => match uuid_from_slice(bytes.get(1..GUID_SIZE + 1)) {
                Some(uuid) => Opcode::After(uuid),
                None => Opcode::Malformed { opcode: 0x01, len: bytes.len() - 1 },
            },
            0x02 => match uuid_from_slice(bytes.get(1..GUID_SIZE + 1)) {
                Some(uuid) => Opcode::Push(uuid, false),
                None => Opcode::Malformed { opcode: 0x02, len: bytes.len() - 1 },
            },
            0x03 => Opcode::And,
            0x04 => Opcode::Or,
            0x05 => Opcode::Not,
            0x06 => Opcode::True,
            0x07 => Opcode::False,
            0x08 => Opcode::End,
            0x09 => Opcode::Sor,
            _ => Opcode::Unknown,
        }
    }
}

impl Opcode {
    fn byte_size(&self) -> usize {
        match *self {
            Opcode::Before(_) | Opcode::After(_) | Opcode::Push(_, _) => 1 + GUID_SIZE,
            _ => 1,
        }
    }
}

/// Represents an associated dependency, where one guid must execute before or after another guid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociatedDependency {
    /// Indicates that the associated guid must be executed before the guid in the enum.
    Before(efi::Guid),
    /// Indicates that the associated guid must be executed after the guid in the enum.
    After(efi::Guid),
}

#[derive(Debug)]
/// A UEFI dependency expression (DEPEX)
pub struct Depex {
    expression: Vec<Opcode>,
}

impl From<&[u8]> for Depex {
    fn from(value: &[u8]) -> Self {
        let depex_parser = DepexParser::new(value);
        Self { expression: depex_parser.into_iter().collect() }
    }
}

impl From<Vec<u8>> for Depex {
    fn from(value: Vec<u8>) -> Self {
        Self::from(value.as_slice())
    }
}

impl From<&[Opcode]> for Depex {
    fn from(value: &[Opcode]) -> Self {
        Self { expression: value.to_vec() }
    }
}

impl Depex {
    /// Evaluates a DEPEX expression.
    pub fn eval(&mut self, protocols: &[efi::Guid]) -> bool {
        let mut stack = Vec::with_capacity(DEPEX_STACK_SIZE_INCREMENT);
        log::trace!("Depex:");
        for (index, opcode) in self.expression.iter_mut().enumerate() {
            match opcode {
                Opcode::Before(_) | Opcode::After(_) => {
                    log::trace!("  {:#x?}", opcode);
                    if index != 0 {
                        debug_assert!(false, "Invalid BEFORE or AFTER not at start of depex {:#x?}", self.expression);
                        return false;
                    }

                    if self.expression.len() > 2 {
                        debug_assert!(
                            false,
                            "Invalid BEFORE or AFTER with additional opcodes {:#x?}.",
                            self.expression
                        );
                        return false;
                    }

                    if self.expression.len() == 2 && self.expression[1] != Opcode::End {
                        debug_assert!(
                            false,
                            "Invalid BEFORE or AFTER with additional opcodes {:#x?}.",
                            self.expression
                        );
                        return false;
                    }
                    return false;
                }
                Opcode::Sor => {
                    log::trace!("  {:#x?}", opcode);
                    if index != 0 {
                        debug_assert!(false, "Invalid SOR not at start of depex.");
                        return false;
                    }
                    return false;
                }
                Opcode::Push(guid, present) => {
                    if *present {
                        stack.push(true)
                    } else {
                        if let Some(guid) = guid_from_uuid(guid) {
                            if protocols.contains(&guid) {
                                *present = true;
                                stack.push(true);
                                continue;
                            }
                        }
                        stack.push(false);
                    }
                    log::trace!(
                        "  {opcode:x?} => {:?}, stack ->{:?}",
                        stack.last(),
                        stack.iter().rev().collect::<Vec<_>>()
                    );
                }
                Opcode::And => {
                    let operator1 = stack.pop().unwrap_or(false);
                    let operator2 = stack.pop().unwrap_or(false);
                    stack.push(operator1 && operator2);
                    log::trace!(
                        "  {opcode:x?}({operator1:?},{operator2:?}) => {:?}, stack ->{:?}",
                        stack.last(),
                        stack.iter().rev().collect::<Vec<_>>()
                    );
                }
                Opcode::Or => {
                    let operator1 = stack.pop().unwrap_or(false);
                    let operator2 = stack.pop().unwrap_or(false);
                    stack.push(operator1 || operator2);
                    log::trace!(
                        "  {opcode:x?}({operator1:?},{operator2:?}) => {:?}, stack ->{:?}",
                        stack.last(),
                        stack.iter().rev().collect::<Vec<_>>()
                    );
                }
                Opcode::Not => {
                    let operator = stack.pop().unwrap_or(false);
                    stack.push(!operator);
                    log::trace!(
                        "  {opcode:x?}({operator:?}) => {:?}, stack ->{:?}",
                        stack.last(),
                        stack.iter().rev().collect::<Vec<_>>()
                    );
                }
                Opcode::True => {
                    stack.push(true);
                    log::trace!(
                        "  {opcode:x?} => {:?}, stack ->{:?}",
                        stack.last(),
                        stack.iter().rev().collect::<Vec<_>>()
                    );
                }
                Opcode::False => {
                    stack.push(false);
                    log::trace!(
                        "  {opcode:x?} => {:?}, stack ->{:?}",
                        stack.last(),
                        stack.iter().rev().collect::<Vec<_>>()
                    );
                }
                Opcode::End => {
                    let operator = stack.pop().unwrap_or(false);
                    log::trace!(
                        "  {opcode:x?} => final result: {:?}, final stack ->{:?}",
                        operator,
                        stack.iter().rev().collect::<Vec<_>>()
                    );
                    return operator;
                }
                Opcode::Unknown => {
                    debug_assert!(false, "Exiting early due to an unknown opcode.");
                    return false;
                }
                Opcode::Malformed { opcode, len } => {
                    log::error!("Opcode [0x{opcode:x?}] expects a guid, only has a length of: {len}");
                    debug_assert!(
                        false,
                        "Exiting early because opcode [0x{opcode:x?}] expects a guid, only has a length of: {len}"
                    );
                    return false;
                }
            }
        }
        false
    }

    /// If the depex expression is an associated dependency, it returns the associated dependency.
    pub fn is_associated(&self) -> Option<AssociatedDependency> {
        match self.expression.first() {
            Some(Opcode::Before(uid)) => Some(AssociatedDependency::Before(guid_from_uuid(uid)?)),
            Some(Opcode::After(uid)) => Some(AssociatedDependency::After(guid_from_uuid(uid)?)),
            _ => None,
        }
    }

    /// indicates that this is a "schedule on request" depex.
    pub fn is_sor(&self) -> bool {
        self.expression.first() == Some(&Opcode::Sor)
    }

    /// Marks a SOR depex as "scheduled". Does nothing for non SOR DEPEX expressions.
    pub fn schedule(&mut self) {
        if self.is_sor() {
            self.expression.remove(0);
        }
    }
}

struct DepexParser {
    expression: Vec<u8>,
    index: usize,
}

impl DepexParser {
    fn new(expression: &[u8]) -> Self {
        Self { expression: expression.to_vec(), index: 0 }
    }
}

impl Iterator for DepexParser {
    type Item = Opcode;

    /// Iterates over the DEPEX expression, returning the next Opcode.
    fn next(&mut self) -> Option<Opcode> {
        if self.index >= self.expression.len() {
            return None;
        }

        let opcode = Opcode::from(&self.expression[self.index..]);
        self.index += opcode.byte_size();
        Some(opcode)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;
    use alloc::vec;
    use core::str::FromStr;
    use r_efi::efi;
    use std::println;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn malformed_opcodes_should_generate_correct_malformed_opcode_enum_variant() {
        // Verify "Before" opcode with no GUID
        assert_eq!(Opcode::from([0x00u8].as_slice()), Opcode::Malformed { opcode: 0x00, len: 0 });
        assert_eq!(
            Opcode::from([0x00u8, 0x01u8, 0x02u8, 0x03u8].as_slice()),
            Opcode::Malformed { opcode: 0x00, len: 3 }
        );

        // Verify "After" opcode with no GUID
        assert_eq!(Opcode::from([0x01u8].as_slice()), Opcode::Malformed { opcode: 0x01, len: 0 });
        assert_eq!(
            Opcode::from([0x01u8, 0x01u8, 0x02u8, 0x03u8].as_slice()),
            Opcode::Malformed { opcode: 0x01, len: 3 }
        );

        // Verify "Push" opcode with no GUID
        assert_eq!(Opcode::from([0x02u8].as_slice()), Opcode::Malformed { opcode: 0x02, len: 0 });
        assert_eq!(
            Opcode::from([0x02u8, 0x01u8, 0x02u8, 0x03u8].as_slice()),
            Opcode::Malformed { opcode: 0x02, len: 3 }
        );
    }

    #[test]
    fn true_should_eval_true() {
        let mut depex = Depex::from(vec![0x06, 0x08]);
        assert!(depex.eval(&[]));
    }

    #[test]
    fn false_should_eval_false() {
        let mut depex = Depex::from(vec![0x07, 0x08]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    fn before_should_eval_false() {
        let mut depex = Depex::from(vec![
            0x00, 0xFA, 0xBD, 0xB6, 0x76, 0xCD, 0x2A, 0x62, 0x44, 0x9E, 0x3F, 0xCB, 0x58, 0xC9, 0x69, 0xD9, 0x37, 0x08,
        ]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    fn after_should_eval_false() {
        let mut depex = Depex::from(vec![
            0x01, 0xFA, 0xBD, 0xB6, 0x76, 0xCD, 0x2A, 0x62, 0x44, 0x9E, 0x3F, 0xCB, 0x58, 0xC9, 0x69, 0xD9, 0x37, 0x08,
        ]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    fn before_should_return_is_associated() {
        let depex = Depex::from(vec![
            0x00, 0xFA, 0xBD, 0xB6, 0x76, 0xCD, 0x2A, 0x62, 0x44, 0x9E, 0x3F, 0xCB, 0x58, 0xC9, 0x69, 0xD9, 0x37, 0x08,
        ]);

        assert_eq!(
            depex.is_associated(),
            Some(AssociatedDependency::Before(
                guid_from_uuid(&Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap()).unwrap()
            ))
        );
    }

    #[test]
    fn after_should_return_is_associated() {
        let depex = Depex::from(vec![
            0x01, 0xFA, 0xBD, 0xB6, 0x76, 0xCD, 0x2A, 0x62, 0x44, 0x9E, 0x3F, 0xCB, 0x58, 0xC9, 0x69, 0xD9, 0x37, 0x08,
        ]);

        assert_eq!(
            depex.is_associated(),
            Some(AssociatedDependency::After(
                guid_from_uuid(&Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap()).unwrap()
            ))
        );
    }

    #[test]
    fn sor_first_opcode_should_eval_false() {
        // Treated as a no-op, with no other operands, false should be returned
        let mut depex = Depex::from(vec![0x09, 0x08]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    fn sor_first_opcode_followed_by_true_should_eval_false() {
        let mut depex = Depex::from(vec![0x09, 0x06, 0x08]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    fn sor_first_opcode_followed_by_true_should_eval_true_after_schedule() {
        let mut depex = Depex::from(vec![0x09, 0x06, 0x08]);
        assert!(!depex.eval(&[]));

        depex.schedule();
        assert!(depex.eval(&[]));
    }

    #[test]
    #[should_panic(expected = "Invalid SOR not at start of depex")]
    fn sor_not_first_opcode_should_eval_false() {
        let mut depex = Depex::from(vec![0x06, 0x09, 0x08]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    #[should_panic(expected = "Exiting early due to an unknown opcode.")]
    fn replacetrue_should_eval_false() {
        let mut depex = Depex::from(vec![0xFF, 0x08]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    #[should_panic(expected = "Exiting early due to an unknown opcode.")]
    fn unknown_opcode_should_return_false() {
        let mut depex = Depex::from(vec![0xE0, 0x08]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    fn not_true_should_eval_false() {
        let mut depex = Depex::from(vec![0x07, 0x06, 0x08]);
        assert!(depex.eval(&[]));
    }

    #[test]
    fn not_false_should_eval_true() {
        let mut depex = Depex::from(vec![0x07, 0x05, 0x08]);
        assert!(depex.eval(&[]));
    }

    #[test]
    /// Tests a DEPEX expression with all AND operations that should evaluate to true when all protocols are installed.
    ///
    /// This test is based on the following dependency expression:
    ///   PUSH EfiPcdProtocolGuid
    ///   PUSH EfiDevicePathUtilitiesProtocolGuid
    ///   PUSH EfiHiiStringProtocolGuid
    ///   PUSH EfiHiiDatabaseProtocolGuid
    ///   PUSH EfiHiiConfigRoutingProtocolGuid
    ///   PUSH EfiResetArchProtocolGuid
    ///   PUSH EfiVariableWriteArchProtocolGuid
    ///   PUSH EfiVariableArchProtocolGuid
    ///   AND
    ///   AND
    ///   AND
    ///   AND
    ///   AND
    ///   AND
    ///   AND
    ///   END
    fn all_protocols_installed_and_should_eval_true() {
        let efi_pcd_prot_uuid = Uuid::from_str("13a3f0f6-264a-3ef0-f2e0-dec512342f34").unwrap();
        let efi_pcd_prot_guid: efi::Guid = guid_from_uuid(&efi_pcd_prot_uuid).unwrap();
        let efi_device_path_utilities_prot_uuid = Uuid::from_str("0379be4e-d706-437d-b037-edb82fb772a4").unwrap();
        let efi_device_path_utilities_prot_guid: efi::Guid =
            guid_from_uuid(&efi_device_path_utilities_prot_uuid).unwrap();
        let efi_hii_string_prot_uuid = Uuid::from_str("0fd96974-23aa-4cdc-b9cb-98d17750322a").unwrap();
        let efi_hii_string_prot_guid: efi::Guid = guid_from_uuid(&efi_hii_string_prot_uuid).unwrap();
        let efi_hii_db_prot_uuid = Uuid::from_str("ef9fc172-a1b2-4693-b327-6d32fc416042").unwrap();
        let efi_hii_db_prot_guid: efi::Guid = guid_from_uuid(&efi_hii_db_prot_uuid).unwrap();
        let efi_hii_config_routing_prot_uuid = Uuid::from_str("587e72d7-cc50-4f79-8209-ca291fc1a10f").unwrap();
        let efi_hii_config_routing_prot_guid: efi::Guid = guid_from_uuid(&efi_hii_config_routing_prot_uuid).unwrap();
        let efi_reset_arch_prot_uuid = Uuid::from_str("27cfac88-46cc-11d4-9a38-0090273fc14d").unwrap();
        let efi_reset_arch_prot_guid: efi::Guid = guid_from_uuid(&efi_reset_arch_prot_uuid).unwrap();
        let efi_var_write_arch_prot_uuid = Uuid::from_str("6441f818-6362-eb44-5700-7dba31dd2453").unwrap();
        let efi_var_write_arch_prot_guid: efi::Guid = guid_from_uuid(&efi_var_write_arch_prot_uuid).unwrap();
        let efi_var_arch_prot_uuid = Uuid::from_str("1e5668e2-8481-11d4-bcf1-0080c73c8881").unwrap();
        let efi_var_arch_prot_guid: efi::Guid = guid_from_uuid(&efi_var_arch_prot_uuid).unwrap();

        let protocols = [
            efi_pcd_prot_guid,
            efi_device_path_utilities_prot_guid,
            efi_hii_string_prot_guid,
            efi_hii_db_prot_guid,
            efi_hii_config_routing_prot_guid,
            efi_reset_arch_prot_guid,
            efi_var_write_arch_prot_guid,
            efi_var_arch_prot_guid,
        ];

        println!("Testing DEPEX for BdsDxe DXE driver...\n");

        let expression: &[u8] = &[
            0x02, 0xF6, 0xF0, 0xA3, 0x13, 0x4A, 0x26, 0xF0, 0x3E, 0xF2, 0xE0, 0xDE, 0xC5, 0x12, 0x34, 0x2F, 0x34, 0x02,
            0x4E, 0xBE, 0x79, 0x03, 0x06, 0xD7, 0x7D, 0x43, 0xB0, 0x37, 0xED, 0xB8, 0x2F, 0xB7, 0x72, 0xA4, 0x02, 0x74,
            0x69, 0xD9, 0x0F, 0xAA, 0x23, 0xDC, 0x4C, 0xB9, 0xCB, 0x98, 0xD1, 0x77, 0x50, 0x32, 0x2A, 0x02, 0x72, 0xC1,
            0x9F, 0xEF, 0xB2, 0xA1, 0x93, 0x46, 0xB3, 0x27, 0x6D, 0x32, 0xFC, 0x41, 0x60, 0x42, 0x02, 0xD7, 0x72, 0x7E,
            0x58, 0x50, 0xCC, 0x79, 0x4F, 0x82, 0x09, 0xCA, 0x29, 0x1F, 0xC1, 0xA1, 0x0F, 0x02, 0x88, 0xAC, 0xCF, 0x27,
            0xCC, 0x46, 0xD4, 0x11, 0x9A, 0x38, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D, 0x02, 0x18, 0xF8, 0x41, 0x64, 0x62,
            0x63, 0x44, 0xEB, 0x57, 0x00, 0x7D, 0xBA, 0x31, 0xDD, 0x24, 0x53, 0x02, 0xE2, 0x68, 0x56, 0x1E, 0x81, 0x84,
            0xD4, 0x11, 0xBC, 0xF1, 0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x03, 0x08,
        ];
        let mut depex = Depex::from(expression.to_vec());

        assert!(depex.eval(&protocols));
    }

    #[test]
    /// Tests a DEPEX expression with AND and OR operations that should evaluate to true when all protocols are installed.
    ///
    /// This test is based on the following dependency expression:
    ///   PUSH EfiVariableArchProtocolGuid
    ///   PUSH EfiVariableWriteArchProtocolGuid
    ///   PUSH EfiTcgProtocolGuid
    ///   PUSH EfiTrEEProtocolGuid
    ///   OR
    ///   AND
    ///   AND
    ///   PUSH EfiPcdProtocolGuid
    ///   PUSH EfiDevicePathUtilitiesProtocolGuid
    ///   AND
    ///   AND
    ///   END
    fn all_protocols_installed_or_and_should_eval_true() {
        let efi_var_arch_prot_uuid = Uuid::from_str("1e5668e2-8481-11d4-bcf1-0080c73c8881").unwrap();
        let efi_var_arch_prot_guid: efi::Guid = guid_from_uuid(&efi_var_arch_prot_uuid).unwrap();
        let efi_var_write_arch_prot_uuid = Uuid::from_str("6441f818-6362-eb44-5700-7dba31dd2453").unwrap();
        let efi_var_write_arch_prot_guid: efi::Guid = guid_from_uuid(&efi_var_write_arch_prot_uuid).unwrap();
        let efi_tcg_prot_uuid = Uuid::from_str("f541796d-a62e-4954-a775-9584f61b9cdd").unwrap();
        let efi_tcg_prot_guid: efi::Guid = guid_from_uuid(&efi_tcg_prot_uuid).unwrap();
        let efi_tree_prot_uuid = Uuid::from_str("607f766c-7455-42be-930b-e4d76db2720f").unwrap();
        let efi_tree_prot_guid: efi::Guid = guid_from_uuid(&efi_tree_prot_uuid).unwrap();
        let efi_pcd_prot_uuid = Uuid::from_str("13a3f0f6-264a-3ef0-f2e0-dec512342f34").unwrap();
        let efi_pcd_prot_guid: efi::Guid = guid_from_uuid(&efi_pcd_prot_uuid).unwrap();
        let efi_device_path_utilities_prot_uuid = Uuid::from_str("0379be4e-d706-437d-b037-edb82fb772a4").unwrap();
        let efi_device_path_utilities_prot_guid: efi::Guid =
            guid_from_uuid(&efi_device_path_utilities_prot_uuid).unwrap();

        let protocols = [
            efi_var_arch_prot_guid,
            efi_var_write_arch_prot_guid,
            efi_tcg_prot_guid,
            efi_tree_prot_guid,
            efi_pcd_prot_guid,
            efi_device_path_utilities_prot_guid,
        ];

        println!("Testing DEPEX for TcgMor DXE driver...\n");

        let expression: &[u8] = &[
            0x02, 0xE2, 0x68, 0x56, 0x1E, 0x81, 0x84, 0xD4, 0x11, 0xBC, 0xF1, 0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81, 0x02,
            0x18, 0xF8, 0x41, 0x64, 0x62, 0x63, 0x44, 0xEB, 0x57, 0x0, 0x7D, 0xBA, 0x31, 0xDD, 0x24, 0x53, 0x02, 0x6D,
            0x79, 0x41, 0xF5, 0x2E, 0xA6, 0x54, 0x49, 0xA7, 0x75, 0x95, 0x84, 0xF6, 0x1B, 0x9C, 0xDD, 0x02, 0x6C, 0x76,
            0x7F, 0x60, 0x55, 0x74, 0xBE, 0x42, 0x93, 0x0B, 0xE4, 0xD7, 0x6D, 0xB2, 0x72, 0x0F, 0x04, 0x03, 0x03, 0x02,
            0xF6, 0xF0, 0xA3, 0x13, 0x4A, 0x26, 0xF0, 0x3E, 0xF2, 0xE0, 0xDE, 0xC5, 0x12, 0x34, 0x2F, 0x34, 0x02, 0x4E,
            0xBE, 0x79, 0x03, 0x06, 0xD7, 0x7D, 0x43, 0xB0, 0x37, 0xED, 0xB8, 0x2F, 0xB7, 0x72, 0xA4, 0x03, 0x03, 0x08,
        ];
        let mut depex = Depex::from(expression.to_vec());

        assert!(depex.eval(&protocols));
    }

    #[test]
    /// This test is based on the following dependency expression:
    ///   PUSH EfiVariableArchProtocolGuid
    ///   PUSH EfiVariableWriteArchProtocolGuid
    ///   PUSH EfiTcgProtocolGuid
    ///   PUSH EfiTrEEProtocolGuid
    ///   OR
    ///   AND
    ///   AND
    ///   PUSH EfiPcdProtocolGuid
    ///   PUSH EfiDevicePathUtilitiesProtocolGuid
    ///   AND
    ///   AND
    ///   END
    fn opcode_list_to_depex_should_work() {
        let efi_var_arch_prot_uuid = Uuid::from_str("1e5668e2-8481-11d4-bcf1-0080c73c8881").unwrap();
        let efi_var_arch_prot_guid: efi::Guid = guid_from_uuid(&efi_var_arch_prot_uuid).unwrap();
        let efi_var_write_arch_prot_uuid = Uuid::from_str("6441f818-6362-eb44-5700-7dba31dd2453").unwrap();
        let efi_var_write_arch_prot_guid: efi::Guid = guid_from_uuid(&efi_var_write_arch_prot_uuid).unwrap();
        let efi_tcg_prot_uuid = Uuid::from_str("f541796d-a62e-4954-a775-9584f61b9cdd").unwrap();
        let efi_tcg_prot_guid: efi::Guid = guid_from_uuid(&efi_tcg_prot_uuid).unwrap();
        let efi_tree_prot_uuid = Uuid::from_str("607f766c-7455-42be-930b-e4d76db2720f").unwrap();
        let efi_tree_prot_guid: efi::Guid = guid_from_uuid(&efi_tree_prot_uuid).unwrap();
        let efi_pcd_prot_uuid = Uuid::from_str("13a3f0f6-264a-3ef0-f2e0-dec512342f34").unwrap();
        let efi_pcd_prot_guid: efi::Guid = guid_from_uuid(&efi_pcd_prot_uuid).unwrap();
        let efi_device_path_utilities_prot_uuid = Uuid::from_str("0379be4e-d706-437d-b037-edb82fb772a4").unwrap();
        let efi_device_path_utilities_prot_guid: efi::Guid =
            guid_from_uuid(&efi_device_path_utilities_prot_uuid).unwrap();

        let protocols = [
            efi_var_arch_prot_guid,
            efi_var_write_arch_prot_guid,
            efi_tcg_prot_guid,
            efi_tree_prot_guid,
            efi_pcd_prot_guid,
            efi_device_path_utilities_prot_guid,
        ];

        let expression: &[Opcode] = &[
            Opcode::Push(efi_var_arch_prot_uuid, true),
            Opcode::Push(efi_var_write_arch_prot_uuid, false),
            Opcode::Push(efi_tcg_prot_uuid, false),
            Opcode::Push(efi_tree_prot_uuid, false),
            Opcode::Or,
            Opcode::And,
            Opcode::And,
            Opcode::Push(efi_pcd_prot_uuid, false),
            Opcode::Push(efi_device_path_utilities_prot_uuid, false),
            Opcode::And,
            Opcode::And,
            Opcode::End,
        ];

        let mut depex = Depex::from(expression);

        assert!(depex.eval(&protocols));
    }

    #[test]
    fn guid_to_uuid_conversion_should_produce_correct_bytes() {
        let device_path_protocol_guid_bytes: &[u8] =
            &[0x4E, 0xBE, 0x79, 0x03, 0x06, 0xD7, 0x7D, 0x43, 0xB0, 0x37, 0xED, 0xB8, 0x2F, 0xB7, 0x72, 0xA4];

        let uuid = uuid_from_slice(Some(device_path_protocol_guid_bytes)).unwrap();
        assert_eq!(uuid, uuid::Uuid::from_str("0379be4e-d706-437d-b037-edb82fb772a4").unwrap());

        let guid = guid_from_uuid(&uuid);
        assert_eq!(guid.unwrap().as_bytes(), device_path_protocol_guid_bytes);
    }

    #[test]
    fn guid_not_in_protocol_db_should_eval_false() {
        let mut depex = Depex::from(vec![
            0x02, 0xF6, 0xF0, 0xA3, 0x13, 0x4A, 0x26, 0xF0, 0x3E, 0xF2, 0xE0, 0xDE, 0xC5, 0x12, 0x34, 0x2F, 0x34, 0x08,
        ]);
        assert!(!depex.eval(&[]));
    }

    #[test]
    #[should_panic(expected = "Invalid BEFORE or AFTER not at start of depex")]
    fn opcode_before_should_panic_when_not_at_start_of_depex() {
        let opcodes = [Opcode::And, Opcode::Before(Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap())];
        let mut depex = Depex::from(opcodes.as_slice());
        depex.eval(&[]);
    }

    #[test]
    #[should_panic(expected = "Invalid BEFORE or AFTER not at start of depex")]
    fn opcode_after_should_panic_when_not_at_start_of_depex() {
        let opcodes = [Opcode::And, Opcode::After(Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap())];
        let mut depex = Depex::from(opcodes.as_slice());
        depex.eval(&[]);
    }

    #[test]
    #[should_panic(expected = "Invalid BEFORE or AFTER with additional opcodes")]
    fn opcode_before_should_panic_when_final_opcode_is_not_end() {
        let opcodes = [Opcode::Before(Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap()), Opcode::And];
        let mut depex = Depex::from(opcodes.as_slice());
        depex.eval(&[]);
    }

    #[test]
    #[should_panic(expected = "Invalid BEFORE or AFTER with additional opcodes")]
    fn opcode_after_should_panic_when_final_opcode_is_not_end() {
        let opcodes = [Opcode::After(Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap()), Opcode::And];
        let mut depex = Depex::from(opcodes.as_slice());
        depex.eval(&[]);
    }

    #[test]
    #[should_panic(expected = "Invalid BEFORE or AFTER with additional opcodes")]
    fn opcode_before_should_panic_when_additional_opcodes_after() {
        let opcodes =
            [Opcode::Before(Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap()), Opcode::And, Opcode::End];
        let mut depex = Depex::from(opcodes.as_slice());
        depex.eval(&[]);
    }

    #[test]
    #[should_panic(expected = "Invalid BEFORE or AFTER with additional opcodes")]
    fn opcode_after_should_panic_when_additional_opcodes_after() {
        let opcodes =
            [Opcode::After(Uuid::from_str("76b6bdfa-2acd-4462-9e3f-cb58c969d937").unwrap()), Opcode::And, Opcode::End];
        let mut depex = Depex::from(opcodes.as_slice());
        depex.eval(&[]);
    }

    #[test]
    #[should_panic(expected = "Exiting early because opcode [0x0] expects a guid, only has a length of: 0")]
    fn malformed_opcode_should_panic_with_well_defined_message() {
        let opcodes = [Opcode::Malformed { opcode: 0x00, len: 0 }];
        let mut depex = Depex::from(opcodes.as_slice());
        depex.eval(&[]);
    }
}
