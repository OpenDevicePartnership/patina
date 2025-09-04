//! Module for converting UEFI errors to rusty errors.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// A specialized [`Result`](core::result::Result) type for EFI operations.
pub type Result<T> = core::result::Result<T, EfiError>;

use r_efi::efi;
// TODO: Handle difference between warning and error

/// EDK II Error Code equivalent as a Rust Error enum
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum EfiError {
    /// The image failed to load.
    LoadError,
    /// The parameter was incorrect.
    InvalidParameter,
    /// The operation is not supported.
    Unsupported,
    /// The buffer was not the proper size for the request.
    BadBufferSize,
    /// The buffer was not large enough to hold the requested data. The required buffer size is returned in the
    /// appropriate parameter when this error occurs.
    BufferTooSmall,
    /// There is no data pending upon return.
    NotReady,
    /// The physical device reported an error while attempting the operation.
    DeviceError,
    /// The device can not be written to.
    WriteProtected,
    /// The resource has run out.
    OutOfResources,
    /// An inconsistency was detected on the file system causing the operation to fail.
    VolumeCorrupted,
    /// There is no more space on the file system.
    VolumeFull,
    /// The device does not contain any medium to perform the operation.
    NoMedia,
    /// The medium in the device has changed since the last access.
    MediaChanged,
    /// The item was not found.
    NotFound,
    /// Access was denied.
    AccessDenied,
    /// The server was not found or did not respond to the request.
    NoResponse,
    /// A mapping to the device does not exist.
    NoMapping,
    /// A timeout time expired.
    Timeout,
    /// The protocol has not been started.
    NotStarted,
    /// The protocol has already been started.
    AlreadyStarted,
    /// The operation was aborted.
    Aborted,
    /// An ICMP error occurred during the network operation.
    IcmpError,
    /// A TFTP error occurred during the network operation.
    TftpError,
    /// A protocol error occurred during the network operation.
    ProtocolError,
    /// A function encountered an internal version that was incompatible with a version requested by the caller.
    IncompatibleError,
    /// The function was not performed due to a security violation.
    SecurityViolation,
    /// A CRC error was detected.
    CrcError,
    /// The beginning or end of media was reached.
    EndOfMedia,
    /// The end of the file was reached.
    EndOfFile,
    /// The language specified was invalid.
    InvalidLanguage,
    /// The security status of the data is unknown or compromised and the data must be updated or replaced to restore a
    /// valid security status.
    CompromisedData,
    /// There is an address conflict address allocation.
    IpAddressConflict,
    /// A HTTP error occurred during the network operation.
    HttpError,
    /// An unknown EFI status code was encountered.
    Unknown(efi::Status),
}

impl EfiError {
    /// Converts an `r_efi::efi::Status` to a `Result`.
    ///
    /// If the status is `SUCCESS`, it returns `Ok(val)`.
    /// Otherwise, it returns an `Err` with the corresponding `EfiError`.
    /// If a Ok value other than `()` is needed, `.map(|_| val)` can be used.
    pub fn status_to_result(status: efi::Status) -> Result<()> {
        match status {
            efi::Status::SUCCESS => Ok(()),
            efi::Status::LOAD_ERROR => Err(EfiError::LoadError),
            efi::Status::INVALID_PARAMETER => Err(EfiError::InvalidParameter),
            efi::Status::UNSUPPORTED => Err(EfiError::Unsupported),
            efi::Status::BAD_BUFFER_SIZE => Err(EfiError::BadBufferSize),
            efi::Status::BUFFER_TOO_SMALL => Err(EfiError::BufferTooSmall),
            efi::Status::NOT_READY => Err(EfiError::NotReady),
            efi::Status::DEVICE_ERROR => Err(EfiError::DeviceError),
            efi::Status::WRITE_PROTECTED => Err(EfiError::WriteProtected),
            efi::Status::OUT_OF_RESOURCES => Err(EfiError::OutOfResources),
            efi::Status::VOLUME_CORRUPTED => Err(EfiError::VolumeCorrupted),
            efi::Status::VOLUME_FULL => Err(EfiError::VolumeFull),
            efi::Status::NO_MEDIA => Err(EfiError::NoMedia),
            efi::Status::MEDIA_CHANGED => Err(EfiError::MediaChanged),
            efi::Status::NOT_FOUND => Err(EfiError::NotFound),
            efi::Status::ACCESS_DENIED => Err(EfiError::AccessDenied),
            efi::Status::NO_RESPONSE => Err(EfiError::NoResponse),
            efi::Status::NO_MAPPING => Err(EfiError::NoMapping),
            efi::Status::TIMEOUT => Err(EfiError::Timeout),
            efi::Status::NOT_STARTED => Err(EfiError::NotStarted),
            efi::Status::ALREADY_STARTED => Err(EfiError::AlreadyStarted),
            efi::Status::ABORTED => Err(EfiError::Aborted),
            efi::Status::ICMP_ERROR => Err(EfiError::IcmpError),
            efi::Status::TFTP_ERROR => Err(EfiError::TftpError),
            efi::Status::PROTOCOL_ERROR => Err(EfiError::ProtocolError),
            efi::Status::INCOMPATIBLE_VERSION => Err(EfiError::IncompatibleError),
            efi::Status::SECURITY_VIOLATION => Err(EfiError::SecurityViolation),
            efi::Status::CRC_ERROR => Err(EfiError::CrcError),
            efi::Status::END_OF_MEDIA => Err(EfiError::EndOfMedia),
            efi::Status::END_OF_FILE => Err(EfiError::EndOfFile),
            efi::Status::INVALID_LANGUAGE => Err(EfiError::InvalidLanguage),
            efi::Status::COMPROMISED_DATA => Err(EfiError::CompromisedData),
            efi::Status::IP_ADDRESS_CONFLICT => Err(EfiError::IpAddressConflict),
            efi::Status::HTTP_ERROR => Err(EfiError::HttpError),
            _ => Err(EfiError::Unknown(status)),
        }
    }
}

impl From<EfiError> for efi::Status {
    fn from(e: EfiError) -> efi::Status {
        match e {
            EfiError::LoadError => efi::Status::LOAD_ERROR,
            EfiError::InvalidParameter => efi::Status::INVALID_PARAMETER,
            EfiError::Unsupported => efi::Status::UNSUPPORTED,
            EfiError::BadBufferSize => efi::Status::BAD_BUFFER_SIZE,
            EfiError::BufferTooSmall => efi::Status::BUFFER_TOO_SMALL,
            EfiError::NotReady => efi::Status::NOT_READY,
            EfiError::DeviceError => efi::Status::DEVICE_ERROR,
            EfiError::WriteProtected => efi::Status::WRITE_PROTECTED,
            EfiError::OutOfResources => efi::Status::OUT_OF_RESOURCES,
            EfiError::VolumeCorrupted => efi::Status::VOLUME_CORRUPTED,
            EfiError::VolumeFull => efi::Status::VOLUME_FULL,
            EfiError::NoMedia => efi::Status::NO_MEDIA,
            EfiError::MediaChanged => efi::Status::MEDIA_CHANGED,
            EfiError::NotFound => efi::Status::NOT_FOUND,
            EfiError::AccessDenied => efi::Status::ACCESS_DENIED,
            EfiError::NoResponse => efi::Status::NO_RESPONSE,
            EfiError::NoMapping => efi::Status::NO_MAPPING,
            EfiError::Timeout => efi::Status::TIMEOUT,
            EfiError::NotStarted => efi::Status::NOT_STARTED,
            EfiError::AlreadyStarted => efi::Status::ALREADY_STARTED,
            EfiError::Aborted => efi::Status::ABORTED,
            EfiError::IcmpError => efi::Status::ICMP_ERROR,
            EfiError::TftpError => efi::Status::TFTP_ERROR,
            EfiError::ProtocolError => efi::Status::PROTOCOL_ERROR,
            EfiError::IncompatibleError => efi::Status::INCOMPATIBLE_VERSION,
            EfiError::SecurityViolation => efi::Status::SECURITY_VIOLATION,
            EfiError::CrcError => efi::Status::CRC_ERROR,
            EfiError::EndOfMedia => efi::Status::END_OF_MEDIA,
            EfiError::EndOfFile => efi::Status::END_OF_FILE,
            EfiError::InvalidLanguage => efi::Status::INVALID_LANGUAGE,
            EfiError::CompromisedData => efi::Status::COMPROMISED_DATA,
            EfiError::IpAddressConflict => efi::Status::IP_ADDRESS_CONFLICT,
            EfiError::HttpError => efi::Status::HTTP_ERROR,
            EfiError::Unknown(status) => status,
        }
    }
}

impl From<efi::Status> for EfiError {
    fn from(status: efi::Status) -> EfiError {
        EfiError::status_to_result(status).unwrap_err()
    }
}
