//! MU Variable Policy Protocol
//!
//! Provides the protocol required to set policies on UEFI variables.
//!
//! See <https://microsoft.github.io/mu/dyn/mu_basecore/MdeModulePkg/Library/VariablePolicyLib/ReadMe/>
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use crate::uefi_protocol::mu_variable_policy::protocol::INITIAL_PROTOCOL_REVISION;
use crate::{error::EfiError, uefi_protocol::mu_variable_policy::protocol::VariablePolicyEntryHeader};

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use r_efi::efi::Guid;

use super::ProtocolInterface;

pub mod protocol {
    use core::ffi::c_void;

    use r_efi::efi::{Guid, Status};

    pub const INITIAL_PROTOCOL_REVISION: u64 = 0x10000;
    pub const VARIABLE_POLICY_ENTRY_REVISION: u32 = 0x10000;
    pub const PROTOCOL_GUID: Guid =
        Guid::from_fields(0x81d1675c, 0x86f6, 0x48df, 0xbd, 0x95, &[0x9a, 0x6e, 0x4f, 0x09, 0x25, 0xc3]);

    #[repr(u8)]
    pub enum VariablePolicyType {
        /// No variable locking is performed. However, the attribute and size constraints are still enforced. LockPolicy field is size 0.
        NoLock = 0,
        /// The variable starts being locked immediately after policy entry registration. If the variable doesn't exist at this point, being LockedNow means it cannot be created on this boot. LockPolicy field is size 0.
        LockNow = 1,
        /// The variable starts being locked after it is created. This allows for variable creation and protection after LockVariablePolicy() function has been called. The LockPolicy field is size 0.
        LockOnCreate = 2,
        /// The Variable Policy Engine will examine the state/contents of another variable to determine if the variable referenced in the policy entry is locked.
        LockOnVarState = 3,
    }

    #[repr(C, packed(1))]
    pub struct VariablePolicyEntryHeader {
        pub version: u32,
        pub size: u16,
        pub offset_to_name: u16,
        pub namespace_guid: u128,
        pub min_size: u32,
        pub max_size: u32,
        pub attributes_must_have: u32,
        pub attributes_cant_have: u32,
        pub lock_policy_type: VariablePolicyType,
        _reserved: [u8; 3],
        // Either name or LockOnVarStatePolicy comes next, depending on lock type
    }

    #[repr(C, packed(1))]
    pub struct LockOnVarStatePolicy {
        pub namespace_guid: u128,
        pub value: u8,
        _reserved: u8,
        // Name comes next
    }

    pub type DisableVariablePolicy = extern "efiapi" fn() -> Status;
    pub type IsVariablePolicyEnabled = extern "efiapi" fn(state: *mut bool) -> Status;
    pub type RegisterVariablePolicy = extern "efiapi" fn(policy_entry: *const VariablePolicyEntryHeader) -> Status;
    pub type DumpVariablePolicy = extern "efiapi" fn(policy: *mut u8, size: *mut u32) -> Status;
    pub type LockVariablePolicy = extern "efiapi" fn() -> Status;

    pub type GetVariablePolicyInfo = extern "efiapi" fn(
        variable_name: *const u16,
        vendor_guid: *const Guid,
        variable_policy_variable_name_buffer_size: *mut usize,
        variable_policy: *mut c_void,
        variable_policy_variable_name: *mut u16,
    ) -> Status;

    pub type GetLockOnVariableStateVariablePolicyInfo = extern "efiapi" fn(
        variable_name: *const u16,
        vendor_guid: *const Guid,
        variable_lock_policy_variable_name_buffer_size: *mut usize,
        variable_policy: *mut c_void,
        variable_lock_policy_variable_name: *mut u16,
    ) -> Status;

    #[repr(C)]
    pub struct Protocol {
        pub revision: u64,
        pub disable_variable_policy: DisableVariablePolicy,
        pub is_variable_policy_enabled: IsVariablePolicyEnabled,
        pub register_variable_policy: RegisterVariablePolicy,
        pub dump_variable_policy: DumpVariablePolicy,
        pub lock_variable_policy: LockVariablePolicy,
        pub get_variable_policy_info: GetVariablePolicyInfo,
        pub get_lock_on_variable_state_variable_policy_info: GetLockOnVariableStateVariablePolicyInfo,
    }
}

#[derive(Debug, Clone)]
pub struct BasicVariablePolicy<'a> {
    pub name: &'a [u16],
    pub namespace: &'a Guid,
    pub min_size: Option<u32>,
    pub max_size: Option<u32>,
    pub attributes_must_have: Option<u32>,
    pub attributes_cant_have: Option<u32>,
}

#[derive(Debug)]
#[repr(u8)]
pub enum VariablePolicy<'a> {
    NoLock(BasicVariablePolicy<'a>),
    LockNow(BasicVariablePolicy<'a>),
    LockOnCreate(BasicVariablePolicy<'a>),
    LockOnVarState {
        basic_policy: BasicVariablePolicy<'a>,
        target_var_name: &'a [u16],
        target_var_namespace: &'a Guid,
        target_var_value: u8,
    },
}

impl VariablePolicy<'_> {
    fn get_type(&self) -> protocol::VariablePolicyType {
        match self {
            Self::NoLock(_) => protocol::VariablePolicyType::NoLock,
            Self::LockNow(_) => protocol::VariablePolicyType::LockNow,
            Self::LockOnCreate(_) => protocol::VariablePolicyType::LockOnCreate,
            Self::LockOnVarState { .. } => protocol::VariablePolicyType::LockOnVarState,
        }
    }

    fn encode(&self) -> Result<Box<[u8]>, EfiError> {
        let basic_policy: &BasicVariablePolicy = match &self {
            Self::NoLock(basic_policy) | Self::LockNow(basic_policy) | Self::LockOnCreate(basic_policy) => basic_policy,
            Self::LockOnVarState { basic_policy, .. } => basic_policy,
        };

        // Check to make sure the variable name is null-terminated
        if !basic_policy.name.ends_with(&[0]) {
            return Err(EfiError::InvalidParameter);
        }

        // Check to make sure the target variable name is null-terminated if applicable
        if let Self::LockOnVarState { target_var_name, .. } = self {
            if !target_var_name.ends_with(&[0]) {
                return Err(EfiError::InvalidParameter);
            }
        }

        // Calculate the size of the required buffer
        let size = size_of::<protocol::VariablePolicyEntryHeader>()
            + match &self {
                Self::NoLock(basic_policy) | Self::LockNow(basic_policy) | Self::LockOnCreate(basic_policy) => {
                    core::mem::size_of_val(basic_policy.name)
                }
                Self::LockOnVarState { basic_policy, target_var_name, .. } => {
                    size_of::<protocol::LockOnVarStatePolicy>()
                        + core::mem::size_of_val(*target_var_name)
                        + core::mem::size_of_val(basic_policy.name)
                }
            };

        let mut buffer: Box<[u8]> = vec![0u8; size].into_boxed_slice();

        // The first part of the buffer is the VariablePolicyEntryHeader
        let header = unsafe { &mut *(buffer.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) };

        header.version = protocol::VARIABLE_POLICY_ENTRY_REVISION;
        header.size = size as u16;
        header.offset_to_name = (size - core::mem::size_of_val(basic_policy.name)) as u16;
        header.namespace_guid = u128::from_le_bytes(*basic_policy.namespace.as_bytes());
        header.min_size = basic_policy.min_size.unwrap_or(0);
        header.max_size = basic_policy.max_size.unwrap_or(u32::MAX);
        header.attributes_must_have = basic_policy.attributes_must_have.unwrap_or(0);
        header.attributes_cant_have = basic_policy.attributes_cant_have.unwrap_or(0);
        header.lock_policy_type = self.get_type();

        // Copy variable name into the buffer
        unsafe {
            core::slice::from_raw_parts_mut(
                buffer.as_mut_ptr().add(header.offset_to_name as usize) as *mut u16,
                basic_policy.name.len(),
            )
        }
        .copy_from_slice(basic_policy.name);

        if let Self::LockOnVarState { basic_policy: _, target_var_name, target_var_namespace, target_var_value } = self
        {
            // The rest of the buffer is the LockOnVarStatePolicy
            let lock_on_var_state_policy_ptr =
                unsafe { buffer.as_mut_ptr().add(size_of::<protocol::VariablePolicyEntryHeader>()) };

            let lock_on_var_state_policy = unsafe {
                &mut *(buffer.as_mut_ptr().add(size_of::<protocol::VariablePolicyEntryHeader>())
                    as *mut protocol::LockOnVarStatePolicy)
            };

            lock_on_var_state_policy.namespace_guid = u128::from_le_bytes(*target_var_namespace.as_bytes());
            lock_on_var_state_policy.value = *target_var_value;

            // Copy over the target variable name
            unsafe {
                core::slice::from_raw_parts_mut(
                    lock_on_var_state_policy_ptr.add(size_of::<protocol::LockOnVarStatePolicy>()) as *mut u16,
                    target_var_name.len(),
                )
            }
            .copy_from_slice(target_var_name);
        }

        Ok(buffer)
    }
}

pub struct MuVariablePolicyProtocol {
    protocol: protocol::Protocol,
}

unsafe impl ProtocolInterface for MuVariablePolicyProtocol {
    const PROTOCOL_GUID: Guid = protocol::PROTOCOL_GUID;
}

impl MuVariablePolicyProtocol {
    pub fn disable_variable_policy(&self) -> Result<(), EfiError> {
        if self.protocol.revision >= INITIAL_PROTOCOL_REVISION {
            EfiError::status_to_result((self.protocol.disable_variable_policy)())
        } else {
            Err(EfiError::Unsupported)
        }
    }

    pub fn is_variable_policy_enabled(&self) -> Result<bool, EfiError> {
        if self.protocol.revision >= INITIAL_PROTOCOL_REVISION {
            let mut policy_enabled: bool = false;
            match EfiError::status_to_result((self.protocol.is_variable_policy_enabled)(&mut policy_enabled)) {
                Ok(_) => Ok(policy_enabled),
                Err(status) => Err(status),
            }
        } else {
            Err(EfiError::Unsupported)
        }
    }

    pub fn register_variable_policy(&self, policy: &VariablePolicy) -> Result<(), EfiError> {
        if self.protocol.revision >= INITIAL_PROTOCOL_REVISION {
            let encoded_policy: Box<[u8]> = policy.encode().map_err(|_| EfiError::InvalidParameter)?;

            EfiError::status_to_result((self.protocol.register_variable_policy)(
                encoded_policy.as_ptr() as *const VariablePolicyEntryHeader
            ))
        } else {
            Err(EfiError::Unsupported)
        }
    }

    pub fn lock_variable_policy(&self) -> Result<(), EfiError> {
        if self.protocol.revision >= INITIAL_PROTOCOL_REVISION {
            EfiError::status_to_result((self.protocol.lock_variable_policy)())
        } else {
            Err(EfiError::Unsupported)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use utf16_lit::{utf16, utf16_null};

    const DUMMY_VAR_NAME_1: &[u16] = &utf16_null!("DummyVariableName1");
    const DUMMY_VAR_NAME_2: &[u16] = &utf16_null!("DummyVariable2");
    const DUMMY_GUID_1: r_efi::base::Guid = r_efi::base::Guid::from_fields(1, 2, 3, 4, 5, &[1, 2, 3, 4, 5, 6]);
    const DUMMY_GUID_2: r_efi::base::Guid =
        r_efi::base::Guid::from_fields(11, 12, 13, 14, 15, &[11, 12, 13, 14, 15, 16]);

    const DUMMY_ATTRIBUTES_MUST_HAVE: u32 = 1;
    const DUMMY_ATTRIBUTES_CANT_HAVE: u32 = 2;
    const DUMMY_VAR_VALUE: u8 = 42;

    #[test]
    pub fn test_encode_variable_policy() {
        let basic_policy = BasicVariablePolicy {
            name: DUMMY_VAR_NAME_1,
            namespace: &DUMMY_GUID_1,
            min_size: None,
            max_size: None,
            attributes_must_have: Some(DUMMY_ATTRIBUTES_MUST_HAVE),
            attributes_cant_have: Some(DUMMY_ATTRIBUTES_CANT_HAVE),
        };

        let policies = vec![
            VariablePolicy::NoLock(basic_policy.clone()),
            VariablePolicy::LockNow(basic_policy.clone()),
            VariablePolicy::LockOnCreate(basic_policy.clone()),
            VariablePolicy::LockOnVarState {
                basic_policy: basic_policy.clone(),
                target_var_name: DUMMY_VAR_NAME_2,
                target_var_namespace: &DUMMY_GUID_2,
                target_var_value: DUMMY_VAR_VALUE,
            },
        ];

        // Do the following for all policies
        for policy in policies {
            let encoded_policy: Box<[u8]> = policy.encode().unwrap();

            // Check size
            assert_eq!(
                encoded_policy.len(),
                match policy {
                    VariablePolicy::NoLock(_) | VariablePolicy::LockNow(_) | VariablePolicy::LockOnCreate(_) => {
                        size_of::<VariablePolicyEntryHeader>() + DUMMY_VAR_NAME_1.len() * size_of::<u16>()
                    }
                    VariablePolicy::LockOnVarState { .. } => {
                        // Left one not updating
                        size_of::<VariablePolicyEntryHeader>()
                            + size_of::<protocol::LockOnVarStatePolicy>()
                            + (DUMMY_VAR_NAME_2.len() * size_of::<u16>())
                            + (DUMMY_VAR_NAME_1.len() * size_of::<u16>())
                    }
                }
            );

            assert_eq!(
                u32::from_le_bytes(encoded_policy[0..4].try_into().unwrap()),
                protocol::VARIABLE_POLICY_ENTRY_REVISION
            );
            assert_eq!(u16::from_le_bytes(encoded_policy[4..6].try_into().unwrap()), encoded_policy.len() as u16);

            // Check offset to name
            assert_eq!(
                u16::from_le_bytes(encoded_policy[6..8].try_into().unwrap()),
                match &policy {
                    VariablePolicy::NoLock(_) | VariablePolicy::LockNow(_) | VariablePolicy::LockOnCreate(_) => {
                        size_of::<VariablePolicyEntryHeader>()
                    }
                    VariablePolicy::LockOnVarState { basic_policy: _, target_var_name, .. } => {
                        size_of::<VariablePolicyEntryHeader>()
                            + size_of::<protocol::LockOnVarStatePolicy>()
                            + (target_var_name.len() * size_of::<u16>())
                    }
                } as u16
            );

            assert_eq!(encoded_policy[8..24], DUMMY_GUID_1.as_bytes().to_vec());
            assert_eq!(u32::from_le_bytes(encoded_policy[24..28].try_into().unwrap()), 0_u32);
            assert_eq!(u32::from_le_bytes(encoded_policy[28..32].try_into().unwrap()), u32::MAX);
            assert_eq!(u32::from_le_bytes(encoded_policy[32..36].try_into().unwrap()), DUMMY_ATTRIBUTES_MUST_HAVE);
            assert_eq!(u32::from_le_bytes(encoded_policy[36..40].try_into().unwrap()), DUMMY_ATTRIBUTES_CANT_HAVE);
            assert_eq!(encoded_policy[40], policy.get_type() as u8);
            assert_eq!(encoded_policy[41..44], vec![0, 0, 0]); // Reserved bytes

            match policy {
                VariablePolicy::NoLock(_) | VariablePolicy::LockNow(_) | VariablePolicy::LockOnCreate(_) => {
                    assert_eq!(
                        &encoded_policy[44..],
                        DUMMY_VAR_NAME_1.iter().flat_map(|&c| c.to_le_bytes()).collect::<Vec<u8>>()
                    );
                }
                VariablePolicy::LockOnVarState { .. } => {
                    assert_eq!(&encoded_policy[44..60], DUMMY_GUID_2.as_bytes().to_vec());
                    assert_eq!(encoded_policy[60], DUMMY_VAR_VALUE);
                    assert_eq!(encoded_policy[61], 0); // Reserved byte

                    // Check the target variable name
                    assert_eq!(
                        &encoded_policy[62..(62 + size_of_val(DUMMY_VAR_NAME_2))],
                        DUMMY_VAR_NAME_2.iter().flat_map(|&c| c.to_le_bytes()).collect::<Vec<u8>>()
                    );

                    // Check the basic policy variable name
                    assert_eq!(
                        &encoded_policy[(62 + size_of_val(DUMMY_VAR_NAME_2))..],
                        DUMMY_VAR_NAME_1.iter().flat_map(|&c| c.to_le_bytes()).collect::<Vec<u8>>()
                    );
                }
            }
        }
    }

    #[test]
    fn test_encode_variable_policy_invalid_name() {
        let bad_name_policy = VariablePolicy::NoLock(BasicVariablePolicy {
            name: &utf16!("InvalidName"), // Missing null terminator
            namespace: &DUMMY_GUID_1,
            min_size: None,
            max_size: None,
            attributes_must_have: Some(DUMMY_ATTRIBUTES_MUST_HAVE),
            attributes_cant_have: Some(DUMMY_ATTRIBUTES_CANT_HAVE),
        });

        assert!(bad_name_policy.encode().unwrap_err() == EfiError::InvalidParameter);

        let bad_target_name_policy = VariablePolicy::LockOnVarState {
            basic_policy: BasicVariablePolicy {
                name: DUMMY_VAR_NAME_1,
                namespace: &DUMMY_GUID_1,
                min_size: None,
                max_size: None,
                attributes_must_have: Some(DUMMY_ATTRIBUTES_MUST_HAVE),
                attributes_cant_have: Some(DUMMY_ATTRIBUTES_CANT_HAVE),
            },
            target_var_name: &utf16!("InvalidTargetName"), // Missing null terminator
            target_var_namespace: &DUMMY_GUID_2,
            target_var_value: DUMMY_VAR_VALUE,
        };

        assert!(bad_target_name_policy.encode().unwrap_err() == EfiError::InvalidParameter);
    }
}
