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

use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::ptr::{self, null_mut};

use crate::boot_services::c_ptr::{CMutPtr, CPtr};
use crate::uefi_protocol::mu_variable_policy::protocol::LockOnVarStatePolicy;
use crate::{error::EfiError, uefi_protocol::mu_variable_policy::protocol::VariablePolicyEntryHeader};

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use efi::Guid;
use r_efi::efi;

use super::ProtocolInterface;

pub mod protocol {
    use core::ffi::c_void;

    use r_efi::efi::{Guid, Status};

    pub const PROTOCOL_REVISION_1: u64 = 0x10000;
    pub const PROTOCOL_REVISION_2: u64 = 0x20000;
    pub const VARIABLE_POLICY_ENTRY_REVISION: u32 = 0x10000;
    pub const PROTOCOL_GUID: Guid =
        Guid::from_fields(0x81d1675c, 0x86f6, 0x48df, 0xbd, 0x95, &[0x9a, 0x6e, 0x4f, 0x09, 0x25, 0xc3]);

    pub const UNRESTRICTED_MIN_SIZE: u32 = 0;
    pub const UNRESTRICTED_MAX_SIZE: u32 = u32::MAX;
    pub const UNRESTRICTED_ATTRIBUTES_MUST_HAVE: u32 = 0;
    pub const UNRESTRICTED_ATTRIBUTES_CANT_HAVE: u32 = 0;

    #[repr(u8)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    impl TryFrom<u8> for VariablePolicyType {
        type Error = ();

        fn try_from(value: u8) -> Result<Self, Self::Error> {
            match value {
                0 => Ok(Self::NoLock),
                1 => Ok(Self::LockNow),
                2 => Ok(Self::LockOnCreate),
                3 => Ok(Self::LockOnVarState),
                _ => Err(()),
            }
        }
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
        pub lock_policy_type: u8,
        pub _reserved: [u8; 3],
        // Either name or LockOnVarStatePolicy comes next, depending on lock type
    }

    #[repr(C, packed(1))]
    pub struct LockOnVarStatePolicy {
        pub namespace_guid: u128,
        pub value: u8,
        _reserved: u8,
        // Name comes next
    }

    // Functions introduced in the first revision of the protocol
    pub type DisableVariablePolicy = extern "efiapi" fn() -> Status;
    pub type IsVariablePolicyEnabled = extern "efiapi" fn(state: *mut bool) -> Status;
    pub type RegisterVariablePolicy = extern "efiapi" fn(policy_entry: *const VariablePolicyEntryHeader) -> Status;
    pub type DumpVariablePolicy = extern "efiapi" fn(policy: *mut u8, size: *mut u32) -> Status;
    pub type LockVariablePolicy = extern "efiapi" fn() -> Status;

    // Functions introduced in the second revision of the protocol
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

#[derive(Debug)]
pub enum RefOrRC<'a, T: ?Sized> {
    Ref(&'a T),
    Rc(Rc<T>),
}

impl<'a, T: ?Sized> RefOrRC<'a, T> {
    fn as_ref(&self) -> &T {
        match self {
            RefOrRC::Ref(r) => r,
            RefOrRC::Rc(o) => o.as_ref(),
        }
    }
}

impl<'a> From<&'a [u16]> for RefOrRC<'a, [u16]> {
    fn from(slice: &'a [u16]) -> Self {
        RefOrRC::Ref(slice)
    }
}

impl From<Rc<[u16]>> for RefOrRC<'_, [u16]> {
    fn from(rc: Rc<[u16]>) -> Self {
        RefOrRC::Rc(rc)
    }
}

impl Clone for RefOrRC<'_, [u16]> {
    fn clone(&self) -> Self {
        match self {
            RefOrRC::Ref(slice) => RefOrRC::Ref(slice),
            RefOrRC::Rc(rc) => RefOrRC::Rc(rc.clone()),
        }
    }
}

impl PartialEq for RefOrRC<'_, [u16]> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref().len() == other.as_ref().len()
            && self.as_ref().iter().zip(other.as_ref().iter()).all(|(a, b)| a == b)
    }
}

impl Eq for RefOrRC<'_, [u16]> {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicVariablePolicy<'a> {
    name: Option<RefOrRC<'a, [u16]>>,
    pub namespace: Guid,
    pub min_size: Option<u32>,
    pub max_size: Option<u32>,
    pub attributes_must_have: Option<u32>,
    pub attributes_cant_have: Option<u32>,
}

impl<'a> BasicVariablePolicy<'a> {
    pub fn new(
        name: Option<&'a [u16]>,
        namespace: Guid,
        min_size: Option<u32>,
        max_size: Option<u32>,
        attributes_must_have: Option<u32>,
        attributes_cant_have: Option<u32>,
    ) -> Result<Self, EfiError> {
        // The name should be null-terminated if it exists
        if let Some(name) = name {
            if !name.ends_with(&[0]) {
                return Err(EfiError::InvalidParameter);
            }
        }

        // The minimum size shouldn't be larger than the maximum size
        if min_size.is_some() && max_size.is_some() && min_size.unwrap() > max_size.unwrap() {
            return Err(EfiError::InvalidParameter);
        }

        // The attributes must have and can't have should not overlap
        if attributes_must_have.is_some() && attributes_cant_have.is_some() {
            if attributes_must_have.unwrap() & attributes_cant_have.unwrap() != 0 {
                return Err(EfiError::InvalidParameter);
            }
        }

        Ok(Self {
            name: name.map(|n| RefOrRC::Ref(n)),
            namespace,
            min_size,
            max_size,
            attributes_must_have,
            attributes_cant_have,
        })
    }

    pub fn new_exact_match(
        name: Option<&'a [u16]>,
        namespace: Guid,
        exact_size: Option<u32>,
        exact_attributes: Option<u32>,
    ) -> Result<Self, EfiError> {
        // The name should be null-terminated if it exists
        if let Some(name) = name {
            if !name.ends_with(&[0]) {
                return Err(EfiError::InvalidParameter);
            }
        }

        Ok(Self {
            name: name.map(|n| RefOrRC::Ref(n)),
            namespace,
            min_size: exact_size,
            max_size: exact_size,
            attributes_must_have: exact_attributes,
            attributes_cant_have: exact_attributes.map(|attributes| !attributes),
        })
    }

    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_ref().map(|name| name.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetVarState<'a> {
    target_var_name: Option<RefOrRC<'a, [u16]>>,
    pub target_var_namespace: Guid,
    pub target_var_value: u8,
}

impl<'a> TargetVarState<'a> {
    pub fn new(
        target_var_name: Option<&'a [u16]>,
        target_var_namespace: Guid,
        target_var_value: u8,
    ) -> Result<Self, EfiError> {
        // The target name should be null-terminated if it exists
        if let Some(target_var_name) = target_var_name {
            if !target_var_name.ends_with(&[0]) {
                return Err(EfiError::InvalidParameter);
            }
        }

        Ok(Self { target_var_name: target_var_name.map(|n| RefOrRC::Ref(n)), target_var_namespace, target_var_value })
    }

    pub fn target_var_name(&self) -> Option<&[u16]> {
        self.target_var_name.as_ref().map(|name| name.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum VariablePolicy<'a> {
    NoLock(BasicVariablePolicy<'a>),
    LockNow(BasicVariablePolicy<'a>),
    LockOnCreate(BasicVariablePolicy<'a>),
    LockOnVarState(BasicVariablePolicy<'a>, TargetVarState<'a>),
}

impl VariablePolicy<'_> {
    pub fn get_type(&self) -> protocol::VariablePolicyType {
        match self {
            Self::NoLock(_) => protocol::VariablePolicyType::NoLock,
            Self::LockNow(_) => protocol::VariablePolicyType::LockNow,
            Self::LockOnCreate(_) => protocol::VariablePolicyType::LockOnCreate,
            Self::LockOnVarState { .. } => protocol::VariablePolicyType::LockOnVarState,
        }
    }

    pub fn get_basic_policy(&self) -> &BasicVariablePolicy {
        match self {
            Self::NoLock(basic_policy) => basic_policy,
            Self::LockNow(basic_policy) => basic_policy,
            Self::LockOnCreate(basic_policy) => basic_policy,
            Self::LockOnVarState(basic_policy, _) => basic_policy,
        }
    }

    pub fn get_target_var_state(&self) -> Option<&TargetVarState> {
        match self {
            Self::LockOnVarState(_, target_var_state) => Some(target_var_state),
            _ => None,
        }
    }

    fn encode(&self) -> Result<Box<[u8]>, EfiError> {
        let basic_policy: &BasicVariablePolicy = self.get_basic_policy();

        // Check to make sure the variable name is null-terminated
        if basic_policy.name().is_some() && !basic_policy.name().unwrap().ends_with(&[0]) {
            return Err(EfiError::InvalidParameter);
        }

        // Check to make sure the target variable name is null-terminated if applicable
        if let Self::LockOnVarState(_, target_var_state) = self {
            if target_var_state.target_var_name.is_some()
                && !target_var_state.target_var_name().unwrap().ends_with(&[0])
            {
                return Err(EfiError::InvalidParameter);
            }
        }

        let name_size_in_bytes =
            if basic_policy.name().is_some() { core::mem::size_of_val(basic_policy.name().unwrap()) } else { 0 };

        // Calculate the size of the required buffer
        let size = size_of::<protocol::VariablePolicyEntryHeader>()
            + name_size_in_bytes
            + match &self {
                Self::NoLock(_) | Self::LockNow(_) | Self::LockOnCreate(_) => 0,
                Self::LockOnVarState(_, target_var_state) => {
                    size_of::<protocol::LockOnVarStatePolicy>()
                        + if target_var_state.target_var_name.is_some() {
                            core::mem::size_of_val(target_var_state.target_var_name().unwrap())
                        } else {
                            0
                        }
                }
            };

        let mut buffer: Box<[u8]> = vec![0u8; size].into_boxed_slice();

        // The first part of the buffer is the VariablePolicyEntryHeader
        let header = unsafe { &mut *(buffer.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) };

        header.version = protocol::VARIABLE_POLICY_ENTRY_REVISION;
        header.size = size as u16;
        header.offset_to_name = (size - name_size_in_bytes) as u16;
        header.namespace_guid = u128::from_le_bytes(*basic_policy.namespace.as_bytes());
        header.min_size = basic_policy.min_size.unwrap_or(protocol::UNRESTRICTED_MIN_SIZE);
        header.max_size = basic_policy.max_size.unwrap_or(protocol::UNRESTRICTED_MAX_SIZE);
        header.attributes_must_have =
            basic_policy.attributes_must_have.unwrap_or(protocol::UNRESTRICTED_ATTRIBUTES_MUST_HAVE);
        header.attributes_cant_have =
            basic_policy.attributes_cant_have.unwrap_or(protocol::UNRESTRICTED_ATTRIBUTES_CANT_HAVE);
        header.lock_policy_type = self.get_type() as u8;

        // Copy variable name into the buffer, if applicable
        if basic_policy.name().is_some() {
            unsafe {
                core::slice::from_raw_parts_mut(
                    buffer.as_mut_ptr().add(header.offset_to_name as usize) as *mut u16,
                    basic_policy.name().unwrap().len(),
                )
            }
            .copy_from_slice(basic_policy.name().unwrap());
        }

        if let Some(target_var_state) = self.get_target_var_state() {
            // The rest of the buffer is the LockOnVarStatePolicy
            let lock_on_var_state_policy_ptr =
                unsafe { buffer.as_mut_ptr().add(size_of::<protocol::VariablePolicyEntryHeader>()) };

            let lock_on_var_state_policy = unsafe {
                &mut *(buffer.as_mut_ptr().add(size_of::<protocol::VariablePolicyEntryHeader>())
                    as *mut protocol::LockOnVarStatePolicy)
            };

            lock_on_var_state_policy.namespace_guid =
                u128::from_le_bytes(*target_var_state.target_var_namespace.as_bytes());
            lock_on_var_state_policy.value = target_var_state.target_var_value;

            // Copy over the target variable name if applicable
            if target_var_state.target_var_name.is_some() {
                unsafe {
                    core::slice::from_raw_parts_mut(
                        lock_on_var_state_policy_ptr.add(size_of::<protocol::LockOnVarStatePolicy>()) as *mut u16,
                        target_var_state.target_var_name().unwrap().len(),
                    )
                }
                .copy_from_slice(target_var_state.target_var_name().unwrap());
            }
        }

        Ok(buffer)
    }

    fn decode<'a>(encoded_policy: &[u8]) -> Result<Box<VariablePolicy<'a>>, EfiError> {
        // Santity checking the buffer is large enough to hold VariablePolicyEntryHeader
        if encoded_policy.len() < size_of::<protocol::VariablePolicyEntryHeader>() {
            return Err(EfiError::Aborted);
        }

        // Interpret the buffer as a VariablePolicyEntryHeader
        let header = unsafe { &*(encoded_policy.as_ptr() as *const protocol::VariablePolicyEntryHeader) };
        if header.version != protocol::VARIABLE_POLICY_ENTRY_REVISION {
            return Err(EfiError::AccessDenied);
        }

        // Check to make sure the buffer is the right size
        if header.size as usize != encoded_policy.len() {
            return Err(EfiError::AlreadyStarted);
        }

        // Check to make sure the name offset is within the buffer, but after the header
        if header.offset_to_name as usize > encoded_policy.len()
            || (header.offset_to_name as usize) < size_of::<protocol::VariablePolicyEntryHeader>()
        {
            return Err(EfiError::BufferTooSmall);
        }

        let name_length_in_bytes = encoded_policy.len() - header.offset_to_name as usize;
        if name_length_in_bytes % size_of::<u16>() != 0 {
            return Err(EfiError::BadBufferSize);
        }

        let mut name: MaybeUninit<Option<Vec<u16>>> = MaybeUninit::uninit();

        if name_length_in_bytes > 0 {
            name.write(Some(vec![0; name_length_in_bytes / size_of::<u16>()]));

            let name_ref = unsafe { name.assume_init_mut() }.as_mut().unwrap();

            // Copy the name from the buffer into the name vector
            // Note that copy_overlapping is required here instead of interpreting the appropriate slice as a &[u16] because the slice may not be aligned correctly
            unsafe {
                ptr::copy_nonoverlapping::<u8>(
                    encoded_policy.as_ptr().add(header.offset_to_name as usize),
                    name_ref.as_mut_ptr() as *mut u8,
                    name_length_in_bytes,
                );
            }

            // Ensure the end (and only the end) of the name is null-terminated
            if name_ref.last() != Some(&0) || name_ref[..name_ref.len() - 1].iter().any(|&c| c == 0) {
                return Err(EfiError::CompromisedData);
            }
        } else {
            name.write(None);
        }

        let name = unsafe { name.assume_init() };

        let basic_policy = BasicVariablePolicy {
            name: if name.is_some() { Some(RefOrRC::Rc(Rc::from(name.unwrap()))) } else { None },
            namespace: Guid::from_bytes(&header.namespace_guid.to_le_bytes()),
            min_size: if header.min_size == protocol::UNRESTRICTED_MIN_SIZE { None } else { Some(header.min_size) },
            max_size: if header.max_size == protocol::UNRESTRICTED_MAX_SIZE { None } else { Some(header.max_size) },
            attributes_must_have: if header.attributes_must_have == protocol::UNRESTRICTED_ATTRIBUTES_MUST_HAVE {
                None
            } else {
                Some(header.attributes_must_have)
            },
            attributes_cant_have: if header.attributes_cant_have == protocol::UNRESTRICTED_ATTRIBUTES_CANT_HAVE {
                None
            } else {
                Some(header.attributes_cant_have)
            },
        };

        if let Ok(lock_policy_type) = protocol::VariablePolicyType::try_from(header.lock_policy_type) {
            match lock_policy_type {
                protocol::VariablePolicyType::NoLock => {
                    return Ok(Box::new(VariablePolicy::NoLock(basic_policy)));
                }
                protocol::VariablePolicyType::LockNow => {
                    return Ok(Box::new(VariablePolicy::LockNow(basic_policy)));
                }
                protocol::VariablePolicyType::LockOnCreate => {
                    return Ok(Box::new(VariablePolicy::LockOnCreate(basic_policy)));
                }
                protocol::VariablePolicyType::LockOnVarState => {
                    // Check if the buffer is large enough for the VariablePolicyEntryHeader, LockOnVarStatePolicy, and the variable name, if defined
                    if encoded_policy.len()
                        < size_of::<protocol::VariablePolicyEntryHeader>()
                            + size_of::<protocol::LockOnVarStatePolicy>()
                            + name_length_in_bytes
                    {
                        return Err(EfiError::CrcError);
                    }

                    // Ensure that both the VariablePolicyEntryHeader and LockOnVarStatePolicy fit before the offset_to_name
                    if (header.offset_to_name as usize)
                        < (size_of::<protocol::VariablePolicyEntryHeader>()
                            + size_of::<protocol::LockOnVarStatePolicy>())
                    {
                        return Err(EfiError::EndOfFile);
                    }

                    let target_name_length_in_bytes = header.offset_to_name as usize
                        - size_of::<protocol::VariablePolicyEntryHeader>()
                        - size_of::<protocol::LockOnVarStatePolicy>();
                    if target_name_length_in_bytes % size_of::<u16>() != 0 {
                        return Err(EfiError::EndOfMedia);
                    }

                    let mut target_name: MaybeUninit<Option<Vec<u16>>> = MaybeUninit::uninit();

                    if target_name_length_in_bytes > 0 {
                        target_name.write(Some(vec![0; target_name_length_in_bytes / size_of::<u16>()]));

                        let target_name_ref = unsafe { target_name.assume_init_mut() }.as_mut().unwrap();

                        // Copy the name from the buffer into the name vector
                        // Note that copy_overlapping is required here instead of interpreting the appropriate slice as a &[u16] because the slice may not be aligned correctly
                        unsafe {
                            ptr::copy_nonoverlapping::<u8>(
                                encoded_policy.as_ptr().add(
                                    size_of::<protocol::VariablePolicyEntryHeader>()
                                        + size_of::<protocol::LockOnVarStatePolicy>(),
                                ),
                                target_name_ref.as_mut_ptr() as *mut u8,
                                target_name_length_in_bytes,
                            );
                        }

                        // Ensure the end (and only the end) of the target variable name is null-terminated
                        if target_name_ref.last() != Some(&0)
                            || target_name_ref[..target_name_ref.len() - 1].iter().any(|&c| c == 0)
                        {
                            return Err(EfiError::VolumeCorrupted);
                        }
                    } else {
                        target_name.write(None);
                    }

                    let target_name = unsafe { target_name.assume_init() };

                    // Get the LockOnVarStatePolicy part of the buffer
                    let lock_on_var_state_policy = unsafe {
                        &*(encoded_policy.as_ptr().add(size_of::<protocol::VariablePolicyEntryHeader>())
                            as *const protocol::LockOnVarStatePolicy)
                    };

                    return Ok(Box::new(VariablePolicy::LockOnVarState(
                        basic_policy,
                        TargetVarState {
                            target_var_name: target_name.map(|name| RefOrRC::Rc(Rc::from(name))),
                            target_var_namespace: Guid::from_bytes(
                                &lock_on_var_state_policy.namespace_guid.to_le_bytes(),
                            ),
                            target_var_value: lock_on_var_state_policy.value,
                        },
                    )));
                }
            }
        } else {
            // There was no valid variable policy type
            return Err(EfiError::VolumeFull);
        }
    }
}

pub struct MuVariablePolicyProtocol {
    protocol: protocol::Protocol,
}

unsafe impl ProtocolInterface for MuVariablePolicyProtocol {
    const PROTOCOL_GUID: Guid = protocol::PROTOCOL_GUID;
}

impl MuVariablePolicyProtocol {
    /// Disable the variable policy engine
    pub fn disable_variable_policy(&self) -> Result<(), EfiError> {
        if self.protocol.revision < protocol::PROTOCOL_REVISION_1 {
            return Err(EfiError::Unsupported);
        }

        EfiError::status_to_result((self.protocol.disable_variable_policy)())
    }

    /// Determine whether or not the variable policy engine is enabled
    pub fn is_variable_policy_enabled(&self) -> Result<bool, EfiError> {
        if self.protocol.revision < protocol::PROTOCOL_REVISION_1 {
            return Err(EfiError::Unsupported);
        }

        let mut policy_enabled: bool = false;
        match EfiError::status_to_result((self.protocol.is_variable_policy_enabled)(&mut policy_enabled)) {
            Ok(_) => Ok(policy_enabled),
            Err(status) => Err(status),
        }
    }

    /// Registers a new variable policy
    pub fn register_variable_policy(&self, policy: &VariablePolicy) -> Result<(), EfiError> {
        if self.protocol.revision < protocol::PROTOCOL_REVISION_1 {
            return Err(EfiError::Unsupported);
        }

        let encoded_policy: Box<[u8]> = policy.encode().map_err(|_| EfiError::InvalidParameter)?;

        EfiError::status_to_result((self.protocol.register_variable_policy)(
            encoded_policy.as_ptr() as *const VariablePolicyEntryHeader
        ))
    }

    pub fn dump_variable_policy(&self) -> Result<Vec<VariablePolicy>, EfiError> {
        if self.protocol.revision < protocol::PROTOCOL_REVISION_1 {
            return Err(EfiError::Unsupported);
        }

        let mut size: u32 = 0;

        // Do an initial call to dump_variable_polcy to get the size of the buffer required
        match (self.protocol.dump_variable_policy)(ptr::null_mut(), &mut size) {
            efi::Status::SUCCESS | efi::Status::BUFFER_TOO_SMALL => {}
            status => return Err(EfiError::from(status)),
        };

        if size == 0 {
            return Ok(Vec::new());
        }

        let mut buffer = vec![0u8; size as usize].into_boxed_slice();

        // Call dump_variable_policy again with the allocated buffer, which should fill it
        EfiError::status_to_result((self.protocol.dump_variable_policy)(buffer.as_mut_ptr(), &mut size))?;

        // If the second call to dump_variable_policy returns a size larger than the buffer, then something went wrong
        if buffer.len() < size as usize {
            debug_assert!(false, "Dumped variable policy size is larger than allocated buffer size");
            return Err(EfiError::BadBufferSize);
        }

        // Decode the policies from the buffer
        let mut policies: Vec<VariablePolicy> = Vec::new();
        let mut offset: u32 = 0;
        while offset < size {
            let remaining_space = (size - offset) as usize;

            // Ensure we have enough bytes for the VariablePolicyEntryHeader
            if remaining_space < size_of::<protocol::VariablePolicyEntryHeader>() {
                return Err(EfiError::InvalidParameter);
            }

            // Decode the policy entry header
            let header: &protocol::VariablePolicyEntryHeader =
                unsafe { &*(buffer.as_ptr().add(offset as usize) as *const protocol::VariablePolicyEntryHeader) };

            // Ensure we have enough bytes for the entire policy entry
            if remaining_space < header.size as usize {
                return Err(EfiError::InvalidParameter);
            }

            // Decode the policy
            let policy =
                VariablePolicy::decode(buffer[offset as usize..(offset + header.size as u32) as usize].into())?;
            policies.push(*policy);

            // Move to the next policy entry
            offset += header.size as u32;
        }

        // Ensure the total size matches the size returned by dump_variable_policy
        if offset < size {
            debug_assert!(false, "Dumped variable policy size does not match size of returned policies.");
            return Err(EfiError::InvalidParameter);
        }

        Ok(policies)
    }

    /// Locks the variable policy engine
    pub fn lock_variable_policy(&self) -> Result<(), EfiError> {
        if self.protocol.revision < protocol::PROTOCOL_REVISION_1 {
            return Err(EfiError::Unsupported);
        }

        EfiError::status_to_result((self.protocol.lock_variable_policy)())
    }

    fn get_variable_lock_policy_info(
        &self,
        variable_name: &[u16],
        namespace_guid: &Guid,
    ) -> Result<(protocol::LockOnVarStatePolicy, Option<Box<[u16]>>), EfiError> {
        let mut lock_on_var_state_policy_data = [0u8; size_of::<protocol::LockOnVarStatePolicy>()];
        let mut target_name: Option<Box<[u16]>> = None;
        let mut target_name_buffer_size_in_bytes: usize = 0;

        match (self.protocol.get_lock_on_variable_state_variable_policy_info)(
            variable_name.as_ptr(),
            namespace_guid.as_ptr(),
            (&mut target_name_buffer_size_in_bytes).as_mut_ptr(),
            (&mut lock_on_var_state_policy_data).as_mut_ptr() as *mut c_void,
            null_mut(),
        ) {
            efi::Status::SUCCESS => {}
            efi::Status::BUFFER_TOO_SMALL => {
                if target_name_buffer_size_in_bytes % size_of::<u16>() != 0 {
                    return Err(EfiError::BadBufferSize);
                }

                let mut target_name_box =
                    vec![0 as u16; target_name_buffer_size_in_bytes / size_of::<u16>()].into_boxed_slice();

                // Get the lock on variable state policy again, this time passing in an appropriately sized name buffer
                match (self.protocol.get_lock_on_variable_state_variable_policy_info)(
                    variable_name.as_ptr(),
                    (&namespace_guid).as_ptr(),
                    (&mut target_name_buffer_size_in_bytes).as_mut_ptr(),
                    (&mut lock_on_var_state_policy_data).as_mut_ptr() as *mut c_void,
                    target_name_box.as_mut_ptr(),
                ) {
                    efi::Status::SUCCESS => {}
                    efi::Status::BUFFER_TOO_SMALL => return Err(EfiError::BadBufferSize),
                    status => return Err(EfiError::from(status)),
                }

                target_name = Some(target_name_box);
            }
            status => return Err(EfiError::from(status)),
        }

        let lock_on_var_state_policy: protocol::LockOnVarStatePolicy =
            unsafe { core::ptr::read(lock_on_var_state_policy_data.as_ptr() as *const protocol::LockOnVarStatePolicy) };

        Ok((lock_on_var_state_policy, target_name))
    }

    pub fn get_variable_policy(
        &self,
        variable_name: Option<&[u16]>,
        namespace_guid: Guid,
    ) -> Result<Option<Box<VariablePolicy>>, EfiError> {
        if self.protocol.revision < protocol::PROTOCOL_REVISION_2 {
            return Err(EfiError::Unsupported);
        }

        // Allocate room for the header
        let mut header_data = [0u8; size_of::<protocol::VariablePolicyEntryHeader>()];
        let mut name: Option<Box<[u16]>> = None;
        let mut name_buffer_size_in_bytes: usize = 0;

        if let Some(variable_name) = variable_name {
            // Ensure the variable name is null-terminated
            if !variable_name.ends_with(&[0]) {
                return Err(EfiError::InvalidParameter);
            }
        }

        let variable_name = variable_name.unwrap_or(&[0_u16].as_ref());

        match (self.protocol.get_variable_policy_info)(
            variable_name.as_ptr(),
            (&namespace_guid).as_ptr(),
            (&mut name_buffer_size_in_bytes).as_mut_ptr(),
            (&mut header_data).as_mut_ptr() as *mut c_void,
            null_mut(),
        ) {
            efi::Status::SUCCESS => {}
            efi::Status::BUFFER_TOO_SMALL => {
                if name_buffer_size_in_bytes % size_of::<u16>() != 0 {
                    return Err(EfiError::BadBufferSize);
                }

                let mut name_box = vec![0 as u16; name_buffer_size_in_bytes / size_of::<u16>()].into_boxed_slice();

                // Get the variable policy again, this time passing in an appropriately sized name buffer
                match (self.protocol.get_variable_policy_info)(
                    variable_name.as_ptr(),
                    (&namespace_guid).as_ptr(),
                    (&mut name_buffer_size_in_bytes).as_mut_ptr(),
                    (&mut header_data).as_mut_ptr() as *mut c_void,
                    name_box.as_mut_ptr(),
                ) {
                    efi::Status::SUCCESS => {}
                    efi::Status::BUFFER_TOO_SMALL => return Err(EfiError::BadBufferSize),
                    status => return Err(EfiError::from(status)),
                }

                name = Some(name_box);
            }

            efi::Status::NOT_FOUND => {
                // If the variable policy is not found, return None
                return Ok(None);
            }
            status => return Err(EfiError::from(status)),
        }

        // Interpret the header data as a VariablePolicyEntryHeader
        let header = unsafe { &mut *(header_data.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) };

        let name_size_in_bytes = name.as_ref().map(|n| core::mem::size_of_val(n.as_ref())).unwrap_or(0);

        // If the lock type is lock on var state, then we need to get the lock on var state information
        let var_policy_type = match protocol::VariablePolicyType::try_from(header.lock_policy_type) {
            Ok(var_policy_type) => var_policy_type,
            Err(_) => return Err(EfiError::InvalidParameter),
        };

        let mut lock_on_var_state_policy: Option<protocol::LockOnVarStatePolicy> = None;
        let mut target_name: MaybeUninit<Option<Box<[u16]>>> = MaybeUninit::uninit();

        if var_policy_type == protocol::VariablePolicyType::LockOnVarState {
            // Retrieve the lock on variable state policy information
            match self.get_variable_lock_policy_info(variable_name, &namespace_guid) {
                Ok((retrieved_policy, retrieved_target_name)) => {
                    lock_on_var_state_policy = Some(retrieved_policy);
                    target_name.write(retrieved_target_name);
                }
                Err(EfiError::NotFound) => {
                    debug_assert!(
                        false,
                        "No lock on variable state policy found for variable {:?} (namespace: {:?}) with LockOnVarState policy.",
                        variable_name, namespace_guid
                    );
                    return Err(EfiError::BadBufferSize);
                }
                Err(e) => return Err(e),
            };
        } else {
            // For other variable policy types, we don't need the lock on variable state policy
            target_name.write(None);
        }

        let target_name_val = unsafe { target_name.assume_init() };
        let target_name_size_in_bytes =
            target_name_val.as_ref().map(|n| core::mem::size_of_val(n.as_ref())).unwrap_or(0);

        let encoded_policy_size: usize = size_of::<protocol::VariablePolicyEntryHeader>()
            + name_size_in_bytes
            + match lock_on_var_state_policy {
                Some(_) => size_of::<protocol::LockOnVarStatePolicy>() + target_name_size_in_bytes,
                None => 0,
            };

        let mut encoded_policy = vec![0 as u8; encoded_policy_size].into_boxed_slice();

        // Update the header with the correct size and offset to name
        header.size = encoded_policy_size as u16;
        header.offset_to_name = (encoded_policy_size - name_size_in_bytes) as u16;

        // Put all components into the encoded policy buffer
        unsafe {
            // Insert the VariablePolicyEntryHeader at the start of the buffer
            let header_ptr: *mut VariablePolicyEntryHeader =
                encoded_policy.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader;
            ptr::copy_nonoverlapping((&header).as_ptr(), header_ptr, 1);

            // If the lock type is LockOnVarState, we need to insert the LockOnVarStatePolicy and target name
            if let Some(lock_on_var_state_policy) = lock_on_var_state_policy {
                let lock_on_var_state_ptr = encoded_policy.as_mut_ptr().add(size_of::<VariablePolicyEntryHeader>())
                    as *mut protocol::LockOnVarStatePolicy;
                ptr::copy_nonoverlapping((&lock_on_var_state_policy).as_ptr(), lock_on_var_state_ptr, 1);
            }

            if let Some(target_name) = target_name_val.as_ref() {
                let target_name_ptr = encoded_policy
                    .as_mut_ptr()
                    .add(size_of::<LockOnVarStatePolicy>() + core::mem::size_of::<VariablePolicyEntryHeader>())
                    as *mut u8;
                ptr::copy_nonoverlapping(
                    (*target_name).as_ref().as_ptr() as *const u8,
                    target_name_ptr,
                    target_name_size_in_bytes,
                );
            }

            // Copy the variable name into the end of the buffer, if defined
            if let Some(name) = name.as_ref() {
                ptr::copy_nonoverlapping(
                    (*name).as_ref().as_ptr() as *const u8,
                    encoded_policy.as_mut_ptr().add(header.offset_to_name as usize),
                    name_size_in_bytes,
                );
            }
        }

        // Now that everything is in place, decode and return the policy
        let result = VariablePolicy::decode(&encoded_policy).map(|boxed_policy| Some(boxed_policy));

        result
    }
}

#[cfg(test)]
mod test {
    use core::u32;

    use super::*;
    use utf16_lit::{utf16, utf16_null};

    const DUMMY_GUID_1: r_efi::base::Guid = r_efi::base::Guid::from_fields(1, 2, 3, 4, 5, &[1, 2, 3, 4, 5, 6]);
    const DUMMY_GUID_2: r_efi::base::Guid =
        r_efi::base::Guid::from_fields(12, 12, 13, 14, 15, &[11, 12, 13, 14, 15, 16]);
    const DUMMY_GUID_3: r_efi::base::Guid =
        r_efi::base::Guid::from_fields(13, 12, 13, 14, 15, &[11, 12, 13, 14, 15, 16]);
    const DUMMY_GUID_4: r_efi::base::Guid =
        r_efi::base::Guid::from_fields(14, 12, 13, 14, 15, &[11, 12, 13, 14, 15, 16]);

    const DUMMY_ATTRIBUTES_MUST_HAVE: u32 = 1;
    const DUMMY_ATTRIBUTES_CANT_HAVE: u32 = 2;
    const DUMMY_MIN_SIZE: u32 = 3;
    const DUMMY_MAX_SIZE: u32 = 4;

    const EMPTY_NAME: &[u16] = &utf16_null!("");

    const DUMMY_VAR_VALUE: u8 = 42;

    extern "efiapi" fn mock_disable_variable_policy() -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_lock_variable_policy() -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_get_variable_policy_info(
        variable_name: *const u16,
        vendor_guid: *const Guid,
        variable_policy_variable_name_buffer_size: *mut usize,
        variable_policy: *mut core::ffi::c_void,
        variable_policy_variable_name: *mut u16,
    ) -> efi::Status {
        if variable_name.is_null() || vendor_guid.is_null() || variable_policy.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let mut variable_name_len: usize = 0;

        // Find the null terminator
        while unsafe { ptr::read(variable_name.add(variable_name_len)) } != 0 {
            variable_name_len += 1;
        }
        variable_name_len += 1; // Include the null terminator in the length

        // Construct a slice from the pointer and length
        let variable_name = unsafe { core::slice::from_raw_parts(variable_name, variable_name_len) };

        // For each in the dummy policies
        for policy in get_dummy_policies() {
            let basic_policy = policy.get_basic_policy();

            if variable_name == basic_policy.name().unwrap_or(EMPTY_NAME)
                && basic_policy.namespace == unsafe { *vendor_guid }
            {
                // Check if the buffer is large enough for the name
                if let Some(name) = basic_policy.name() {
                    if variable_policy_variable_name.is_null()
                        || unsafe { *variable_policy_variable_name_buffer_size } < core::mem::size_of_val(name)
                    {
                        unsafe {
                            *variable_policy_variable_name_buffer_size = core::mem::size_of_val(name);
                        }
                        return efi::Status::BUFFER_TOO_SMALL;
                    }
                } else {
                    unsafe {
                        *variable_policy_variable_name_buffer_size = 0;
                    }
                }

                let mut encoded_policy = policy.encode().unwrap();

                // Reduce the size of the variable policy to just the VariablePolicyEntryHeader and the variable name
                let variable_policy_size = size_of::<protocol::VariablePolicyEntryHeader>()
                    + basic_policy.name().unwrap_or(EMPTY_NAME).len() * size_of::<u16>();

                let header: &mut VariablePolicyEntryHeader =
                    unsafe { &mut *(encoded_policy.as_mut_ptr() as *mut VariablePolicyEntryHeader) };

                header.size = variable_policy_size as u16;
                header.offset_to_name = size_of::<protocol::VariablePolicyEntryHeader>() as u16;

                unsafe {
                    ptr::copy_nonoverlapping(
                        encoded_policy.as_ptr(),
                        variable_policy as *mut u8,
                        size_of::<protocol::VariablePolicyEntryHeader>(),
                    );

                    // Copy the variable name into the output buffer
                    if let Some(name) = basic_policy.name() {
                        ptr::copy_nonoverlapping(name.as_ptr(), variable_policy_variable_name as *mut u16, name.len());
                    }
                }

                return efi::Status::SUCCESS;
            }
        }

        return efi::Status::NOT_FOUND;
    }

    extern "efiapi" fn mock_get_lock_on_variable_state_variable_policy_info(
        variable_name: *const u16,
        vendor_guid: *const Guid,
        variable_lock_policy_variable_name_buffer_size: *mut usize,
        variable_policy: *mut core::ffi::c_void,
        variable_lock_policy_variable_name: *mut u16,
    ) -> efi::Status {
        if variable_name.is_null() || vendor_guid.is_null() || variable_policy.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        let mut variable_name_len: usize = 0;

        // Find the null terminator
        while unsafe { ptr::read(variable_name.add(variable_name_len)) } != 0 {
            variable_name_len += 1;
        }
        variable_name_len += 1; // Include the null terminator in the length

        // Construct a slice from the pointer and length
        let variable_name = unsafe { core::slice::from_raw_parts(variable_name, variable_name_len) };

        for policy in get_dummy_policies().iter() {
            let basic_policy = policy.get_basic_policy();
            if variable_name == basic_policy.name().unwrap_or(EMPTY_NAME)
                && basic_policy.namespace == unsafe { *vendor_guid }
            {
                match policy {
                    VariablePolicy::LockOnVarState(_, target_var_state) => {
                        // Check if the buffer is large enough for the target variable name
                        if let Some(target_var_name) = target_var_state.target_var_name() {
                            if variable_lock_policy_variable_name.is_null()
                                || unsafe { *variable_lock_policy_variable_name_buffer_size }
                                    < core::mem::size_of_val(target_var_name)
                            {
                                unsafe {
                                    *variable_lock_policy_variable_name_buffer_size =
                                        core::mem::size_of_val(target_var_name);
                                }
                                return efi::Status::BUFFER_TOO_SMALL;
                            }
                        } else {
                            unsafe {
                                *variable_lock_policy_variable_name_buffer_size = 0;
                            }
                        }

                        // Encode the policy and extract the relevant parts
                        let encoded_policy = policy.encode().unwrap();
                        unsafe {
                            ptr::copy_nonoverlapping(
                                encoded_policy.as_ptr().add(size_of::<protocol::VariablePolicyEntryHeader>()),
                                variable_policy as *mut u8,
                                size_of::<protocol::LockOnVarStatePolicy>(),
                            );

                            // Copy the variable name into the output buffer
                            if let Some(target_var_name) = target_var_state.target_var_name() {
                                ptr::copy_nonoverlapping(
                                    target_var_name.as_ptr(),
                                    variable_lock_policy_variable_name as *mut u16,
                                    target_var_name.len(),
                                );
                            }
                        }

                        return efi::Status::SUCCESS;
                    }
                    _ => {
                        // If a match on a non-LockOnVarState policy is found, return NOT_FOUND
                        return efi::Status::NOT_FOUND;
                    }
                }
            }
        }

        return efi::Status::NOT_FOUND;
    }

    extern "efiapi" fn mock_is_variable_policy_enabled(state: *mut bool) -> efi::Status {
        unsafe {
            *state = true;
        }
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_register_variable_policy(policy_entry: *const VariablePolicyEntryHeader) -> efi::Status {
        if policy_entry.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_dump_variable_policy(buffer: *mut u8, size: *mut u32) -> efi::Status {
        let dummy_policies = get_dummy_policies().iter().map(|policy| policy.encode().unwrap()).collect::<Vec<_>>();
        let policy_dump_size = dummy_policies.iter().map(|p| p.len()).sum::<usize>();

        // The size pointer should never be null
        if size.is_null() {
            assert!(false, "Size pointer was null");
            return efi::Status::INVALID_PARAMETER;
        }

        // Ensure the size is big enough to hold the policies, otherwise return BUFFER_TOO_SMALL
        if unsafe { *size } == 0 || (unsafe { *size } as usize) < policy_dump_size {
            unsafe {
                *size = policy_dump_size as u32;
            };
            return efi::Status::BUFFER_TOO_SMALL;
        }

        // The buffer pointer should never be null if size is non-zero
        if buffer.is_null() {
            assert!(false, "Buffer pointer was null with a non-zero size");
            return efi::Status::INVALID_PARAMETER;
        }

        // Interpret the buffer as a mutable slice of u8
        let buffer = unsafe { core::slice::from_raw_parts_mut(buffer, *size as usize) };

        // Fill the buffer with the encoded policies back-to-back
        let mut offset = 0;
        dummy_policies.iter().for_each(|policy| {
            let policy_size = policy.len();
            if offset + policy_size > buffer.len() {
                assert!(false, "Buffer overflow while dumping variable policies");
                return;
            }
            buffer[offset..offset + policy_size].copy_from_slice(policy);
            offset += policy_size;
        });

        efi::Status::SUCCESS
    }

    const MOCKED_PROTOCOL: protocol::Protocol = protocol::Protocol {
        revision: protocol::PROTOCOL_REVISION_2,
        disable_variable_policy: mock_disable_variable_policy,
        lock_variable_policy: mock_lock_variable_policy,
        get_variable_policy_info: mock_get_variable_policy_info,
        get_lock_on_variable_state_variable_policy_info: mock_get_lock_on_variable_state_variable_policy_info,
        is_variable_policy_enabled: mock_is_variable_policy_enabled,
        register_variable_policy: mock_register_variable_policy,
        dump_variable_policy: mock_dump_variable_policy,
    };

    pub fn get_dummy_policies() -> Vec<VariablePolicy<'static>> {
        vec![
            VariablePolicy::NoLock(
                BasicVariablePolicy::new(
                    Some(&utf16_null!("Var1")),
                    DUMMY_GUID_1,
                    None,
                    None,
                    Some(DUMMY_ATTRIBUTES_MUST_HAVE),
                    Some(DUMMY_ATTRIBUTES_CANT_HAVE),
                )
                .unwrap(),
            ),
            VariablePolicy::LockNow(
                BasicVariablePolicy::new(
                    Some(&utf16_null!("Variable2")),
                    DUMMY_GUID_2,
                    None,
                    None,
                    Some(DUMMY_ATTRIBUTES_MUST_HAVE),
                    Some(DUMMY_ATTRIBUTES_CANT_HAVE),
                )
                .unwrap(),
            ),
            VariablePolicy::LockNow(
                BasicVariablePolicy::new(
                    Some(&utf16_null!("Var3!")),
                    DUMMY_GUID_3,
                    None,
                    None,
                    Some(DUMMY_ATTRIBUTES_MUST_HAVE),
                    Some(DUMMY_ATTRIBUTES_CANT_HAVE),
                )
                .unwrap(),
            ),
            VariablePolicy::LockOnCreate(
                BasicVariablePolicy::new(
                    Some(&utf16_null!("V4")),
                    DUMMY_GUID_4,
                    None,
                    None,
                    Some(DUMMY_ATTRIBUTES_MUST_HAVE),
                    Some(DUMMY_ATTRIBUTES_CANT_HAVE),
                )
                .unwrap(),
            ),
            VariablePolicy::LockOnVarState(
                BasicVariablePolicy::new(
                    Some(&utf16_null!("AMuchLongerVariableNameThatCorrespondsToTheFifthVariable")),
                    DUMMY_GUID_1,
                    None,
                    None,
                    Some(DUMMY_ATTRIBUTES_MUST_HAVE),
                    Some(DUMMY_ATTRIBUTES_CANT_HAVE),
                )
                .unwrap(),
                TargetVarState::new(Some(&utf16_null!("SomeTargetVariableName")), DUMMY_GUID_2, DUMMY_VAR_VALUE)
                    .unwrap(),
            ),
            // Non-LockOnVarState policy with no name
            VariablePolicy::NoLock(
                BasicVariablePolicy::new(None, DUMMY_GUID_1, Some(DUMMY_MIN_SIZE), Some(DUMMY_MAX_SIZE), None, None)
                    .unwrap(),
            ),
            // LockOnVarState with no name
            VariablePolicy::LockOnVarState(
                BasicVariablePolicy::new(None, DUMMY_GUID_2, Some(DUMMY_MIN_SIZE), Some(DUMMY_MAX_SIZE), None, None)
                    .unwrap(),
                TargetVarState::new(Some(&utf16_null!("TargetVariableName1")), DUMMY_GUID_2, DUMMY_VAR_VALUE).unwrap(),
            ),
            // LockOnVarState with no target name
            VariablePolicy::LockOnVarState(
                BasicVariablePolicy::new(
                    Some(&utf16_null!("AnotherVariableName6")),
                    DUMMY_GUID_3,
                    Some(DUMMY_MIN_SIZE),
                    Some(DUMMY_MAX_SIZE),
                    None,
                    None,
                )
                .unwrap(),
                TargetVarState::new(None, DUMMY_GUID_2, DUMMY_VAR_VALUE).unwrap(),
            ),
            // LockOnVarState with no name or target name
            VariablePolicy::LockOnVarState(
                BasicVariablePolicy::new(None, DUMMY_GUID_4, Some(DUMMY_MIN_SIZE), Some(DUMMY_MAX_SIZE), None, None)
                    .unwrap(),
                TargetVarState::new(None, DUMMY_GUID_2, DUMMY_VAR_VALUE).unwrap(),
            ),
        ]
    }

    #[test]
    pub fn test_encode_variable_policy() {
        // Do the following for all policies
        for policy in get_dummy_policies().iter() {
            let encoded_policy: Box<[u8]> = policy.encode().unwrap();

            let basic_policy = policy.get_basic_policy();

            let name_length_in_bytes = basic_policy.name().map(|name| core::mem::size_of_val(name)).unwrap_or(0);
            let target_name_length_in_bytes = policy
                .get_target_var_state()
                .map(|state| state.target_var_name().map(|name| name.len() * size_of::<u16>()).unwrap_or(0))
                .unwrap_or(0);

            // Check size
            assert_eq!(
                encoded_policy.len(),
                match policy {
                    VariablePolicy::LockOnVarState(..) => {
                        size_of::<VariablePolicyEntryHeader>()
                            + size_of::<protocol::LockOnVarStatePolicy>()
                            + target_name_length_in_bytes
                            + name_length_in_bytes
                    }
                    _ => {
                        size_of::<VariablePolicyEntryHeader>() + name_length_in_bytes
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
                    VariablePolicy::LockOnVarState(..) => {
                        size_of::<VariablePolicyEntryHeader>()
                            + size_of::<protocol::LockOnVarStatePolicy>()
                            + target_name_length_in_bytes
                    }
                } as u16
            );

            assert_eq!(encoded_policy[8..24], basic_policy.namespace.as_bytes().to_vec());
            assert_eq!(
                u32::from_le_bytes(encoded_policy[24..28].try_into().unwrap()),
                basic_policy.min_size.unwrap_or(protocol::UNRESTRICTED_MIN_SIZE)
            );
            assert_eq!(
                u32::from_le_bytes(encoded_policy[28..32].try_into().unwrap()),
                basic_policy.max_size.unwrap_or(protocol::UNRESTRICTED_MAX_SIZE)
            );
            assert_eq!(
                u32::from_le_bytes(encoded_policy[32..36].try_into().unwrap()),
                basic_policy.attributes_must_have.unwrap_or(protocol::UNRESTRICTED_ATTRIBUTES_MUST_HAVE)
            );
            assert_eq!(
                u32::from_le_bytes(encoded_policy[36..40].try_into().unwrap()),
                basic_policy.attributes_cant_have.unwrap_or(protocol::UNRESTRICTED_ATTRIBUTES_CANT_HAVE)
            );
            assert_eq!(encoded_policy[40], policy.get_type() as u8);
            assert_eq!(encoded_policy[41..44], vec![0, 0, 0]); // Reserved bytes

            let var_name_slice = basic_policy.name().unwrap_or(&[]);

            match policy {
                VariablePolicy::NoLock(_) | VariablePolicy::LockNow(_) | VariablePolicy::LockOnCreate(_) => {
                    assert_eq!(
                        &encoded_policy[44..],
                        var_name_slice.iter().flat_map(|&c| c.to_le_bytes()).collect::<Vec<u8>>()
                    );
                }
                VariablePolicy::LockOnVarState(_, target_var_state) => {
                    assert_eq!(&encoded_policy[44..60], target_var_state.target_var_namespace.as_bytes().to_vec());
                    assert_eq!(encoded_policy[60], target_var_state.target_var_value);
                    assert_eq!(encoded_policy[61], 0); // Reserved byte

                    let target_var_name_slice = target_var_state.target_var_name().unwrap_or(&[]);

                    // Check the target variable name
                    assert_eq!(
                        &encoded_policy[62..(62 + size_of_val(target_var_name_slice))],
                        target_var_name_slice.iter().flat_map(|&c| c.to_le_bytes()).collect::<Vec<u8>>()
                    );

                    // Check the basic policy variable name
                    assert_eq!(
                        &encoded_policy[(62 + size_of_val(target_var_name_slice))..],
                        var_name_slice.iter().flat_map(|&c| c.to_le_bytes()).collect::<Vec<u8>>()
                    );
                }
            }
        }
    }

    #[test]
    fn test_round_trip_encode_decode() {
        for policy in get_dummy_policies().iter() {
            let encoded_policy = policy.encode().unwrap();
            let decoded_policy = VariablePolicy::decode(encoded_policy.as_ref()).unwrap();
            assert_eq!(*policy, *decoded_policy);
        }
    }

    #[test]
    fn test_encode_variable_policy_invalid_name() {
        let bad_name_policy = VariablePolicy::NoLock(BasicVariablePolicy {
            name: Some((&utf16!("InvalidName")[..]).into()), // Missing null terminator
            namespace: DUMMY_GUID_1,
            min_size: None,
            max_size: None,
            attributes_must_have: Some(DUMMY_ATTRIBUTES_MUST_HAVE),
            attributes_cant_have: Some(DUMMY_ATTRIBUTES_CANT_HAVE),
        });

        assert!(bad_name_policy.encode().unwrap_err() == EfiError::InvalidParameter);

        let bad_target_name_policy = VariablePolicy::LockOnVarState(
            BasicVariablePolicy {
                name: Some((&utf16_null!("SomeVarName")[..]).into()),
                namespace: DUMMY_GUID_1,
                min_size: None,
                max_size: None,
                attributes_must_have: Some(DUMMY_ATTRIBUTES_MUST_HAVE),
                attributes_cant_have: Some(DUMMY_ATTRIBUTES_CANT_HAVE),
            },
            TargetVarState {
                target_var_name: Some((&utf16!("InvalidTargetName")[..]).into()), // Missing null terminator
                target_var_namespace: DUMMY_GUID_2,
                target_var_value: DUMMY_VAR_VALUE,
            },
        );

        assert!(bad_target_name_policy.encode().unwrap_err() == EfiError::InvalidParameter);
    }

    #[test]
    fn test_decode_invalid_variable_policy() {
        // Test decoding with an invalid buffer size (one byte smaller than a header + a minimal (zero char) name)
        let invalid_buffer =
            vec![0u8; size_of::<protocol::VariablePolicyEntryHeader>() + size_of::<u16>() - 1].into_boxed_slice();
        assert!(VariablePolicy::decode(invalid_buffer.as_ref()).is_err());

        for policy in get_dummy_policies() {
            let encoded_policy = policy.encode().unwrap();

            // Test decoding with an invalid version (version incremented by 1)
            let mut invalid_encoding = encoded_policy.clone();
            unsafe { &mut *(invalid_encoding.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }.version += 1;
            assert!(VariablePolicy::decode(invalid_encoding.as_ref()).is_err());

            // Test decoding with an invalid size (size != buffer length)
            let mut invalid_encoding = encoded_policy.clone();
            unsafe { &mut *(invalid_encoding.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }.size =
                invalid_encoding.len() as u16 + 1;
            assert!(VariablePolicy::decode(invalid_encoding.as_ref()).is_err());

            // Test decoding with an invalid offset to name (offset larger than buffer length)
            let mut invalid_encoding = encoded_policy.clone();
            unsafe { &mut *(invalid_encoding.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }
                .offset_to_name = (encoded_policy.len() + 1) as u16;
            assert!(VariablePolicy::decode(invalid_encoding.as_ref()).is_err());

            // Test decoding with an invalid offset to name (offset less than size of VariablePolicyEntryHeader)
            let mut invalid_encoding = encoded_policy.clone();
            unsafe { &mut *(invalid_encoding.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }
                .offset_to_name = size_of::<protocol::VariablePolicyEntryHeader>() as u16 - 1;
            assert!(VariablePolicy::decode(invalid_encoding.as_ref()).is_err());

            // Test decoding with an invalid lock policy type (invalid value)
            let mut invalid_encoding = encoded_policy.clone();
            unsafe { &mut *(invalid_encoding.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }
                .lock_policy_type = 255; // Invalid value
            assert!(VariablePolicy::decode(invalid_encoding.as_ref()).is_err());

            if policy.get_type() == protocol::VariablePolicyType::LockOnVarState {
                // Test decoding with an invalid offset to name (offset less than size of VariablePolicyEntryHeader + LockOnVarStatePolicy + minimal name)
                let mut invalid_encoding = encoded_policy.clone();
                unsafe { &mut *(invalid_encoding.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }
                    .offset_to_name = (size_of::<protocol::VariablePolicyEntryHeader>()
                    + size_of::<protocol::LockOnVarStatePolicy>()
                    + size_of::<u16>()) as u16
                    - 1;
                assert!(VariablePolicy::decode(invalid_encoding.as_ref()).is_err());
            } else {
                // Test to make sure an invalidly sized LockOnVarState policy is detected by modifying non-LockOnVarState policies
                // to have a LockOnVarState type

                let mut invalid_encoding = encoded_policy.clone();

                // Set the name following the header to 0 in u16, then truncate the box and set the size in the header as appropriate
                unsafe {
                    *(invalid_encoding.as_mut_ptr().add(size_of::<protocol::VariablePolicyEntryHeader>()) as *mut u16) =
                        0
                };

                // Truncate the buffer to the size of VariablePolicyEntryHeader
                let mut shortened_policy =
                    invalid_encoding[0..(size_of::<protocol::VariablePolicyEntryHeader>())].to_vec().into_boxed_slice();
                unsafe { &mut *(shortened_policy.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }.size =
                    shortened_policy.len() as u16;

                // We've only shortened the name to be zero length, so it should still be valid
                let _ = VariablePolicy::decode(shortened_policy.clone().as_ref()).unwrap();

                // Modifying the type of the policy to LockOnVarState should fail due to the size being invalid
                unsafe { &mut *(shortened_policy.as_mut_ptr() as *mut protocol::VariablePolicyEntryHeader) }
                    .lock_policy_type = protocol::VariablePolicyType::LockOnVarState as u8;
                assert!(VariablePolicy::decode(shortened_policy.as_ref()).is_err());
            }
        }
    }

    // Test dump_variable_policy mocking policy.dump_variable_policy
    #[test]
    fn test_dump_variable_policy() {
        let protocol = MuVariablePolicyProtocol { protocol: MOCKED_PROTOCOL };

        let policies = protocol.dump_variable_policy().unwrap();

        assert_eq!(policies.len(), get_dummy_policies().len());
        for (i, policy) in policies.iter().enumerate() {
            assert_eq!(policy, &get_dummy_policies()[i]);
        }
    }

    #[test]
    fn test_get_variable_policy() {
        let protocol = MuVariablePolicyProtocol { protocol: MOCKED_PROTOCOL };

        for policy in get_dummy_policies().iter() {
            let basic_policy = policy.get_basic_policy();

            let retrieved_policy: Option<Box<VariablePolicy<'_>>> = protocol
                .get_variable_policy(basic_policy.name(), basic_policy.namespace)
                .expect(format!("Failed to get variable policy for policy {:?}", policy).as_str());
            assert!(retrieved_policy.is_some());
            assert_eq!(retrieved_policy.unwrap().as_ref(), policy);
        }

        // Test getting a variable policy that does not exist
        let non_existent_policy =
            protocol.get_variable_policy(Some(&utf16_null!("NonExistentVariable")), DUMMY_GUID_1).unwrap();
        assert!(non_existent_policy.is_none());

        // Test getting a variable policy with a non-null-terminated name
        let invalid_name_policy = protocol.get_variable_policy(Some(&utf16!("InvalidName")), DUMMY_GUID_1);
        assert!(invalid_name_policy.unwrap_err() == EfiError::InvalidParameter);
    }
}
