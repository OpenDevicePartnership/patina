use alloc::{string::String, vec::Vec};
use patina_sdk::{boot_services::BootServices, component::service::IntoService};
use spin::RwLock;

use crate::{
    acpi::StandardAcpiProvider,
    error::AmlError,
    opcode::{
        AML_OPCODE_BYTE_SIZE, AML_OPCODE_EXTENDED_BYTE_SIZE, AML_OPCODE_EXTENDED_PREFIX, OPCODE_TABLE, OperandType,
    },
    service::{AcpiProvider, AmlParser, TableKey},
    signature::ACPI_CHECKSUM_OFFSET,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AmlSdtHandleInternal {
    modified: bool,
    table_key: TableKey,
    // SHERRY: the idea would be to use zerocopy to read opcodes and stuff from the AML Stream
    offset: usize,
    size: usize,
}

impl AmlSdtHandleInternal {
    fn new(table_key: TableKey, offset: usize, size: usize) -> Self {
        Self { modified: false, table_key, offset, size }
    }
}

// Sentinel
const ROOT_NODE: AmlSdtHandleInternal =
    AmlSdtHandleInternal { modified: false, table_key: TableKey(0), offset: 0, size: 0 };

pub type AmlHandle = AmlSdtHandleInternal;

pub enum AmlData {
    None,
    Opcode(u8),
    NameString(AmlNameStringPath),
    OpFn(AmlOp),
    UnsignedInt(u64),
    StringLiteral(String),
    Child(AmlHandle),
}

pub enum AmlOp {}

pub struct AmlNameStringPath {}

#[derive(IntoService)]
#[service(dyn AmlParser)]
struct StandardAmlParser<B: BootServices + 'static> {
    acpi_table_manager: StandardAcpiProvider<B>,
    open_handles: RwLock<Vec<AmlHandle>>,
    root_node: AmlSdtHandleInternal,
}

impl<B> StandardAmlParser<B>
where
    B: BootServices,
{
    pub fn new(acpi_table_manager: StandardAcpiProvider<B>) -> Self {
        Self { acpi_table_manager, open_handles: RwLock::new(Vec::new()), root_node: ROOT_NODE }
    }
}

impl<B> AmlParser for StandardAmlParser<B>
where
    B: BootServices,
{
    fn open_sdt(&self, table_key: crate::service::TableKey) -> Result<AmlHandle, crate::error::AmlError> {
        let table = self.acpi_table_manager.get_acpi_table(table_key).map_err(|_| AmlError::InvalidHandle)?;
        let table_bytes = unsafe { table.as_bytes() };
        let size = self.get_node_size(table_bytes)?;
        let handle = AmlHandle::new(table_key, 0, size);
        self.open_handles.write().push(handle);
        Ok(handle)
    }

    fn close_sdt(&self, handle: AmlHandle) -> Result<(), crate::error::AmlError> {
        let mut handles = self.open_handles.write();
        let handle_idx = handles.iter().position(|h| *h == handle);
        if let Some(index) = handle_idx {
            let handle_to_remove = handles.remove(index);
            let mut table_for_handle = self
                .acpi_table_manager
                .get_acpi_table(handle_to_remove.table_key)
                .map_err(|_| AmlError::InvalidHandle)?;
            table_for_handle.update_checksum(ACPI_CHECKSUM_OFFSET).map_err(|_| AmlError::CloseFailedChecksumUpdate)?;
            Ok(())
        } else {
            Err(crate::error::AmlError::InvalidHandle)
        }
    }

    fn iter_options(&self, handle: AmlHandle) -> Result<Vec<AmlData>, crate::error::AmlError> {
        let table = self.acpi_table_manager.get_acpi_table(handle.table_key).map_err(|_| AmlError::InvalidHandle)?;
        let table_bytes = unsafe { table.as_bytes() };
        let aml_stream =
            table_bytes.get(handle.offset..handle.offset + handle.size).ok_or(AmlError::InvalidAcpiTable)?;
        let mut options_offset = 0; // SHERRY: skip past the opcode and pkglength
        let mut all_options = Vec::new();
        while options_offset < aml_stream.len() {
            let option_size = 1; // SHERRY: figure out how to actually parse this
            let option = aml_stream.get(options_offset..options_offset + option_size).ok_or(AmlError::OutOfBounds)?;
            let option_data = AmlData::None; // SHERRY: parse the option into AmlData
            options_offset += option_size;
            all_options.push(option_data);
        }
        Ok(all_options)
    }

    fn get_child(&self, handle: AmlHandle) -> Result<AmlHandle, crate::error::AmlError> {
        todo!()
    }

    fn get_sibling(&self, handle: AmlHandle) -> Result<AmlHandle, crate::error::AmlError> {
        let table = self.acpi_table_manager.get_acpi_table(handle.table_key).map_err(|_| AmlError::InvalidHandle)?;
        let table_bytes = unsafe { table.as_bytes() };
        let sibling_start = handle.offset + handle.size;
        let sibling = table_bytes.get(sibling_start..).ok_or(AmlError::OutOfBounds)?;
        let sibling_size = 1; // SHERRY: figure out how to actually parse this
        let sibling_handle = AmlHandle::new(handle.table_key, sibling_start, sibling_size);
        Ok(sibling_handle)
    }
}

impl<B> StandardAmlParser<B>
where
    B: BootServices,
{
    // option bc not all opcodes have pkg length. if called on wrong type returns None
    fn get_node_size(&self, table_bytes: &[u8]) -> Result<usize, AmlError> {
        let mut offset = 0;
        let (opcode, opcode_size) = if table_bytes[0] == AML_OPCODE_EXTENDED_PREFIX {
            ((0x5B << 8) | table_bytes[1] as u16, AML_OPCODE_EXTENDED_BYTE_SIZE) // extended opcode always starts with 0x5B
        } else {
            (table_bytes[0] as u16, AML_OPCODE_BYTE_SIZE)
        };

        // Advance offset by opcode size
        let mut offset = opcode_size;

        // Step 2: lookup with the *full* opcode
        let op_info = OPCODE_TABLE.get(&opcode).ok_or(AmlError::InvalidOpcode)?;

        if op_info.has_pkg_length {
            let pkg_lead_byte = table_bytes[offset];
            let pkg_len_field_size = (pkg_lead_byte >> 6) + 1; //  The high 2 bits of the first byte reveal how many follow bytes are in the PkgLength.
            let mut pkg_length = (pkg_lead_byte & 0x3F) as usize; //  If the PkgLength has only
            //  one byte, bit 0 through 5 are used to encode the package length (in other words, values 0-63).
            //  If the package length
            //  value is more than 63, more than one byte must be used for the encoding in which case bit 4 and 5 of the PkgLeadByte
            //  are reserved and must be zero. If the multiple bytes encoding is used, bits 0-3 of the PkgLeadByte become the least
            //  significant 4 bits of the resulting package length value.

            for i in 0..pkg_len_field_size {
                //  The next ByteData will become the next least significant 8 bits
                //  of the resulting value and so on, up to 3 ByteData bytes.
                let next_byte = table_bytes.get(offset + 1 + (i as usize)).ok_or(AmlError::OutOfBounds)?;
                pkg_length |= (*next_byte as usize) << (6 + (i * 8)); // first byte has 6 bits used, each subsequent byte has 8 bits
            }

            Ok(offset + pkg_len_field_size as usize + pkg_length)
        } else {
            for operand in op_info.operands {
                match operand {
                    OperandType::Byte => offset += 1,
                    OperandType::Word => offset += 2,
                    OperandType::DWord => offset += 4,
                    OperandType::QWord => offset += 8,
                    OperandType::String => {
                        let str_end =
                            table_bytes[offset..].iter().position(|&b| b == 0).ok_or(AmlError::OutOfBounds)?; // find null term
                        offset += str_end + 1; // include null terminator
                    }
                    OperandType::NameString => {
                        let name_str_end = self.get_name_string_size(&table_bytes[offset..])?;
                        offset += name_str_end;
                    }
                    OperandType::DataRefObject => {
                        // recurse here
                        offset += self.get_node_size(table_bytes.get(offset..).ok_or(AmlError::OutOfBounds)?)?;
                    }
                    _ => {
                        return Err(AmlError::InvalidOpcode); // unknown or opcode is pkg_length which shouldn't be true here
                    }
                }
            }
            Ok(offset)
        }
    }

    fn get_name_string_size(&self, name_bytes: &[u8]) -> Result<usize, AmlError> {
        //  NameSeg :=
        //  <leadnamechar namechar namechar namechar>
        // Notice that NameSegs shorter than 4 characters are filled with trailing underscores (‘_’s).

        //  NamePath := NameSeg (4 bytes) | DualNamePath (8 bytes) | MultiNamePath (MultiNamePath := 0x2F SegCount NameSeg(SegCount)) | NullName (0x00)

        // NameString := <\ namepath> | <^ namepath>

        let mut offset = 0;

        // optional root char
        if name_bytes[0] == b'\\' {
            offset += 1;
        }

        // some number of prefixes (can be zero)
        while name_bytes[offset] == b'^' {
            offset += 1;
        }

        // reached namepath
        let namepath_type = name_bytes[offset];
        match namepath_type {
            0x00 => {
                // null name
                offset += 1;
            }
            0x2F => {
                // multi name path
                offset += 1;
                let seg_count = name_bytes[offset] as usize;
                offset += 1;
                offset += seg_count * 4; // each seg is 4 bytes
            }
            0x2E => {
                // dual name path
                offset += 1;
                offset += 8; // two segs of 4 bytes each
            }
            _ => {
                // single name seg
                offset += 4; // one seg of 4 bytes
            }
        }

        Ok(offset)
    }
}

// https://uefi.org/sites/default/files/resources/ACPI_Spec_6_5_Aug29.pdf
