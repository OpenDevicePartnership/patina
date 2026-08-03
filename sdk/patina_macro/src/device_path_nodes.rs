//! UEFI device path node text-to-binary conversions.
//!
//! Binary node layouts, type and subtype values, and field semantics follow the
//! [UEFI 2.11 Device Path Nodes specification][device-path-nodes]. Text syntax,
//! parameter ordering, defaults, and aliases follow [Text Representation
//! Basics][text-representation] and the [Text Device Node Reference][text-nodes].
//!
//! [device-path-nodes]: https://uefi.org/specs/UEFI/2.11/10_Protocols_Device_Path_Protocol.html#device-path-nodes
//! [text-representation]: https://uefi.org/specs/UEFI/2.11/10_Protocols_Device_Path_Protocol.html#text-representation-basics
//! [text-nodes]: https://uefi.org/specs/UEFI/2.11/10_Protocols_Device_Path_Protocol.html#text-device-node-reference
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// cspell:ignore UIDSTR SASSATA nvmexpress nguid

use crate::{
    device_path_encoder::{
        NodeWriter, parse_efi_guid, parse_eisa_id, parse_fixed_hex, parse_hex_bytes, parse_ipv4, parse_ipv6, parse_u8,
        parse_u16, parse_u32, parse_u64, parse_unsigned, parse_uuid_bytes,
    },
    device_path_parser::{DevicePathError, ParsedArgument, ParsedNode, ParsedNodeKind},
};

const PC_ANSI_GUID: &str = "e0c14753-f9be-11d2-9a0c-0090273fc14d";
const VT_100_GUID: &str = "dfa66065-b419-11d3-9a2d-0090273fc14d";
const VT_100_PLUS_GUID: &str = "7baec70b-57e0-4c76-8e87-2f9e28088343";
const VT_UTF8_GUID: &str = "ad15a0d6-8bec-4acf-a073-d01de77e2d88";
const UART_FLOW_CONTROL_GUID: &str = "37499a9d-542f-4c89-a026-35da142094e4";
const SAS_GUID: &str = "d487ddb4-008b-11d9-afdc-001083ffca4d";
const DEBUG_PORT_GUID: &str = "eba4e8d2-3858-41ec-a281-2647ba9660d0";
const VIRTUAL_DISK_GUID: &str = "77ab535a-45fc-624b-5560-f7b281d1f96e";
const VIRTUAL_CD_GUID: &str = "3d5abd30-4175-87ce-6d64-d2ade523c4bb";
const PERSISTENT_VIRTUAL_DISK_GUID: &str = "5cea02c9-4d07-69d3-269f-4496fbe096f9";
const PERSISTENT_VIRTUAL_CD_GUID: &str = "08018188-42cd-bb48-100f-5387d53ded3d";

/// Encoding of one custom vendor-defined payload field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VendorDefinedFieldType {
    U8,
    U16Le,
    U32Le,
    U64Le,
    Guid,
    Uuid,
    Bytes,
}

impl VendorDefinedFieldType {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "u8" => Some(Self::U8),
            "u16le" => Some(Self::U16Le),
            "u32le" => Some(Self::U32Le),
            "u64le" => Some(Self::U64Le),
            "guid" => Some(Self::Guid),
            "uuid" => Some(Self::Uuid),
            "bytes" => Some(Self::Bytes),
            _ => None,
        }
    }
}

/// One named field in a custom vendor-defined payload.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VendorDefinedField {
    pub(crate) name: String,
    pub(crate) field_type: VendorDefinedFieldType,
}

/// Device path node type for a user-declared vendor-defined shortcut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VendorDefinedType {
    Hardware,
    Messaging,
    Media,
}

impl VendorDefinedType {
    fn node_type_and_subtype(self) -> (u8, u8) {
        match self {
            Self::Hardware => (1, 4),
            Self::Messaging => (3, 10),
            Self::Media => (4, 3),
        }
    }
}

/// A user-declared shortcut for a vendor-defined device path node.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VendorDefinedSchema {
    pub(crate) name: String,
    pub(crate) vendor_type: VendorDefinedType,
    pub(crate) guid: [u8; 16],
    pub(crate) fields: Vec<VendorDefinedField>,
}

/// Return whether a name is reserved by a built-in UEFI node or shortcut.
pub(crate) fn is_builtin_node_name(name: &str) -> bool {
    matches!(
        name,
        "Path"
            | "HardwarePath"
            | "Pci"
            | "PcCard"
            | "MemoryMapped"
            | "VenHw"
            | "Ctrl"
            | "BMC"
            | "AcpiPath"
            | "Acpi"
            | "PciRoot"
            | "PcieRoot"
            | "Floppy"
            | "Keyboard"
            | "Serial"
            | "ParallelPort"
            | "AcpiEx"
            | "AcpiExp"
            | "AcpiAdr"
            | "NvdimmAcpiAdr"
            | "Msg"
            | "Ata"
            | "Scsi"
            | "Fibre"
            | "FibreEx"
            | "I1394"
            | "USB"
            | "I2O"
            | "Infiniband"
            | "VenMsg"
            | "VenPcAnsi"
            | "VenVt100"
            | "VenVt100Plus"
            | "VenUtf8"
            | "UartFlowCtrl"
            | "SAS"
            | "DebugPort"
            | "MAC"
            | "IPv4"
            | "IPv6"
            | "Uart"
            | "UsbClass"
            | "UsbAudio"
            | "UsbCDCControl"
            | "UsbHID"
            | "UsbImage"
            | "UsbPrinter"
            | "UsbMassStorage"
            | "UsbHub"
            | "UsbCDCData"
            | "UsbSmartCard"
            | "UsbVideo"
            | "UsbDiagnostic"
            | "UsbWireless"
            | "UsbDeviceFirmwareUpdate"
            | "UsbIrdaBridge"
            | "UsbTestAndMeasurement"
            | "UsbWwid"
            | "Unit"
            | "Sata"
            | "iSCSI"
            | "Vlan"
            | "SasEx"
            | "NVMe"
            | "Uri"
            | "UFS"
            | "SD"
            | "Bluetooth"
            | "Wi-Fi"
            | "eMMC"
            | "BluetoothLE"
            | "Dns"
            | "NVDIMM"
            | "RestService"
            | "NVMEoF"
            | "MediaPath"
            | "HD"
            | "CDROM"
            | "VenMedia"
            | "Media"
            | "FvFile"
            | "Fv"
            | "Offset"
            | "RamDisk"
            | "VirtualDisk"
            | "VirtualCD"
            | "PersistentVirtualDisk"
            | "PersistentVirtualCD"
            | "BbsPath"
            | "BBS"
    )
}

/// Encode one parsed node.
pub(crate) fn encode_node(
    node: &ParsedNode,
    vendor_defined_schemas: &[VendorDefinedSchema],
) -> Result<Vec<u8>, DevicePathError> {
    match &node.kind {
        ParsedNodeKind::FilePath(path) => encode_file_path(node, path),
        ParsedNodeKind::Canonical { name, arguments } => {
            encode_canonical(node, name, arguments, vendor_defined_schemas)
        }
    }
}

fn encode_canonical(
    node: &ParsedNode,
    name: &str,
    arguments: &[ParsedArgument],
    vendor_defined_schemas: &[VendorDefinedSchema],
) -> Result<Vec<u8>, DevicePathError> {
    match name {
        "Path" => encode_generic(node, arguments, None),
        "HardwarePath" => encode_generic(node, arguments, Some(1)),
        "Pci" => encode_pci(node, arguments),
        "PcCard" => encode_pc_card(node, arguments),
        "MemoryMapped" => encode_memory_mapped(node, arguments),
        "VenHw" => encode_vendor(node, arguments, 1, 4),
        "Ctrl" => encode_controller(node, arguments),
        "BMC" => encode_bmc(node, arguments),
        "AcpiPath" => encode_generic(node, arguments, Some(2)),
        "Acpi" => encode_acpi(node, arguments, None),
        "PciRoot" => encode_acpi(node, arguments, Some("PNP0A03")),
        "PcieRoot" => encode_acpi(node, arguments, Some("PNP0A08")),
        "Floppy" => encode_acpi(node, arguments, Some("PNP0604")),
        "Keyboard" => encode_acpi(node, arguments, Some("PNP0301")),
        "Serial" => encode_acpi(node, arguments, Some("PNP0501")),
        "ParallelPort" => encode_acpi(node, arguments, Some("PNP0401")),
        "AcpiEx" => encode_acpi_ex(node, arguments),
        "AcpiExp" => encode_acpi_exp(node, arguments),
        "AcpiAdr" => encode_acpi_adr(node, arguments),
        "NvdimmAcpiAdr" => encode_nvdimm_acpi_adr(node, arguments),
        "Msg" => encode_generic(node, arguments, Some(3)),
        "Ata" => encode_ata(node, arguments),
        "Scsi" => encode_scsi(node, arguments),
        "Fibre" => encode_fibre(node, arguments),
        "FibreEx" => encode_fibre_ex(node, arguments),
        "I1394" => encode_i1394(node, arguments),
        "USB" => encode_usb(node, arguments),
        "I2O" => encode_i2o(node, arguments),
        "Infiniband" => encode_infiniband(node, arguments),
        "VenMsg" => encode_vendor(node, arguments, 3, 10),
        "VenPcAnsi" => encode_guid_vendor_shortcut(node, arguments, PC_ANSI_GUID),
        "VenVt100" => encode_guid_vendor_shortcut(node, arguments, VT_100_GUID),
        "VenVt100Plus" => encode_guid_vendor_shortcut(node, arguments, VT_100_PLUS_GUID),
        "VenUtf8" => encode_guid_vendor_shortcut(node, arguments, VT_UTF8_GUID),
        "UartFlowCtrl" => encode_uart_flow_control(node, arguments),
        "SAS" => encode_sas(node, arguments),
        "DebugPort" => encode_guid_vendor_shortcut(node, arguments, DEBUG_PORT_GUID),
        "MAC" => encode_mac(node, arguments),
        "IPv4" => encode_ipv4(node, arguments),
        "IPv6" => encode_ipv6(node, arguments),
        "Uart" => encode_uart(node, arguments),
        "UsbClass" => encode_usb_class(node, arguments, None, None),
        "UsbAudio" => encode_usb_class(node, arguments, Some(1), None),
        "UsbCDCControl" => encode_usb_class(node, arguments, Some(2), None),
        "UsbHID" => encode_usb_class(node, arguments, Some(3), None),
        "UsbImage" => encode_usb_class(node, arguments, Some(6), None),
        "UsbPrinter" => encode_usb_class(node, arguments, Some(7), None),
        "UsbMassStorage" => encode_usb_class(node, arguments, Some(8), None),
        "UsbHub" => encode_usb_class(node, arguments, Some(9), None),
        "UsbCDCData" => encode_usb_class(node, arguments, Some(10), None),
        "UsbSmartCard" => encode_usb_class(node, arguments, Some(11), None),
        "UsbVideo" => encode_usb_class(node, arguments, Some(14), None),
        "UsbDiagnostic" => encode_usb_class(node, arguments, Some(220), None),
        "UsbWireless" => encode_usb_class(node, arguments, Some(224), None),
        "UsbDeviceFirmwareUpdate" => encode_usb_class(node, arguments, Some(254), Some(1)),
        "UsbIrdaBridge" => encode_usb_class(node, arguments, Some(254), Some(2)),
        "UsbTestAndMeasurement" => encode_usb_class(node, arguments, Some(254), Some(3)),
        "UsbWwid" => encode_usb_wwid(node, arguments),
        "Unit" => encode_unit(node, arguments),
        "Sata" => encode_sata(node, arguments),
        "iSCSI" => encode_iscsi(node, arguments),
        "Vlan" => encode_vlan(node, arguments),
        "SasEx" => encode_sas_ex(node, arguments),
        "NVMe" => encode_nvme(node, arguments),
        "Uri" => encode_uri(node, arguments),
        "UFS" => encode_ufs(node, arguments),
        "SD" => encode_sd_emmc(node, arguments, 26),
        "Bluetooth" => encode_bluetooth(node, arguments),
        "Wi-Fi" => encode_wifi(node, arguments),
        "eMMC" => encode_sd_emmc(node, arguments, 29),
        "BluetoothLE" => encode_bluetooth_le(node, arguments),
        "Dns" => encode_dns(node, arguments),
        "NVDIMM" => encode_nvdimm(node, arguments),
        "RestService" => encode_rest_service(node, arguments),
        "NVMEoF" => encode_nvme_of(node, arguments),
        "MediaPath" => encode_generic(node, arguments, Some(4)),
        "HD" => encode_hard_drive(node, arguments),
        "CDROM" => encode_cdrom(node, arguments),
        "VenMedia" => encode_vendor(node, arguments, 4, 3),
        "Media" => encode_media(node, arguments),
        "FvFile" => encode_fv(node, arguments, 6),
        "Fv" => encode_fv(node, arguments, 7),
        "Offset" => encode_offset(node, arguments),
        "RamDisk" => encode_ram_disk(node, arguments, None),
        "VirtualDisk" => encode_ram_disk(node, arguments, Some(VIRTUAL_DISK_GUID)),
        "VirtualCD" => encode_ram_disk(node, arguments, Some(VIRTUAL_CD_GUID)),
        "PersistentVirtualDisk" => encode_ram_disk(node, arguments, Some(PERSISTENT_VIRTUAL_DISK_GUID)),
        "PersistentVirtualCD" => encode_ram_disk(node, arguments, Some(PERSISTENT_VIRTUAL_CD_GUID)),
        "BbsPath" => encode_generic(node, arguments, Some(5)),
        "BBS" => encode_bbs(node, arguments),
        _ => vendor_defined_schemas.iter().find(|schema| schema.name == name).map_or_else(
            || Err(DevicePathError::new(node.offset, format!("unknown device path node `{name}`"))),
            |schema| encode_vendor_defined(node, arguments, schema),
        ),
    }
}

struct ResolvedArgs<'a, 'n> {
    node_offset: usize,
    names: &'n [&'n str],
    values: Vec<Option<&'a ParsedArgument>>,
}

impl<'a, 'n> ResolvedArgs<'a, 'n> {
    fn new(node: &ParsedNode, arguments: &'a [ParsedArgument], names: &'n [&'n str]) -> Result<Self, DevicePathError> {
        let mut values = vec![None; names.len()];
        let mut positional_index = 0;

        for argument in arguments {
            let index = if let Some(name) = &argument.name {
                names.iter().position(|candidate| candidate == name).ok_or_else(|| {
                    DevicePathError::new(argument.offset, format!("unknown parameter `{name}` for this node"))
                })?
            } else {
                while values.get(positional_index).is_some_and(Option::is_some) {
                    positional_index += 1;
                }
                if positional_index >= names.len() {
                    return Err(DevicePathError::new(argument.offset, "too many parameters for this node"));
                }
                let index = positional_index;
                positional_index += 1;
                index
            };

            let slot = values.get_mut(index).expect("parameter index is derived from the parameter name table");
            if slot.is_some() {
                return Err(DevicePathError::new(
                    argument.offset,
                    format!(
                        "parameter `{}` was provided more than once",
                        names.get(index).expect("parameter index is derived from the parameter name table")
                    ),
                ));
            }
            *slot = Some(argument);
        }

        Ok(Self { node_offset: node.offset, names, values })
    }

    fn optional(&self, index: usize) -> Option<&'a ParsedArgument> {
        self.values.get(index).copied().flatten().filter(|argument| !argument.value.is_empty())
    }

    fn required(&self, index: usize) -> Result<&'a ParsedArgument, DevicePathError> {
        self.optional(index).ok_or_else(|| {
            DevicePathError::new(
                self.node_offset,
                format!(
                    "{} is required",
                    self.names.get(index).expect("parameter index is derived from the parameter name table")
                ),
            )
        })
    }

    fn text(&self, index: usize, default: &'static str) -> &'a str {
        self.optional(index).map_or(default, |argument| argument.value.as_str())
    }

    fn offset(&self, index: usize) -> usize {
        self.values.get(index).copied().flatten().map_or(self.node_offset, |argument| argument.offset)
    }
}

fn ensure_no_arguments(_node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<(), DevicePathError> {
    if let Some(argument) = arguments.first() {
        return Err(DevicePathError::new(argument.offset, "this node does not accept parameters"));
    }
    Ok(())
}

fn encode_generic(
    node: &ParsedNode,
    arguments: &[ParsedArgument],
    fixed_type: Option<u8>,
) -> Result<Vec<u8>, DevicePathError> {
    let names: &'static [&'static str] =
        if fixed_type.is_some() { &["Subtype", "Data"] } else { &["Type", "Subtype", "Data"] };
    let args = ResolvedArgs::new(node, arguments, names)?;
    let (r#type, subtype_index) = if let Some(r#type) = fixed_type {
        (r#type, 0)
    } else {
        let argument = args.required(0)?;
        (parse_u8(&argument.value, argument.offset, "Type")?, 1)
    };
    let subtype = args.required(subtype_index)?;
    let data = args
        .optional(subtype_index + 1)
        .map_or(Ok(Vec::new()), |argument| parse_hex_bytes(&argument.value, argument.offset, "Data"))?;

    let mut writer = NodeWriter::new(node, r#type, parse_u8(&subtype.value, subtype.offset, "Subtype")?);
    writer.bytes(&data);
    writer.finish()
}

fn encode_pci(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Device", "Function"])?;
    let device = args.required(0)?;
    let function = args.required(1)?;
    let device = parse_u8(&device.value, device.offset, "Device")?;
    let function = parse_u8(&function.value, function.offset, "Function")?;
    if device > 31 {
        return Err(DevicePathError::new(args.offset(0), "Device must be between 0 and 31"));
    }
    if function > 7 {
        return Err(DevicePathError::new(args.offset(1), "Function must be between 0 and 7"));
    }

    let mut writer = NodeWriter::new(node, 1, 1);
    writer.byte(function);
    writer.byte(device);
    writer.finish()
}

fn encode_pc_card(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Function"])?;
    let function = args.required(0)?;
    let mut writer = NodeWriter::new(node, 1, 2);
    writer.byte(parse_u8(&function.value, function.offset, "Function")?);
    writer.finish()
}

fn encode_memory_mapped(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["EfiMemoryType", "StartingAddress", "EndingAddress"])?;
    let memory_type = args.required(0)?;
    let start = args.required(1)?;
    let end = args.required(2)?;
    let mut writer = NodeWriter::new(node, 1, 3);
    writer.u32(parse_u32(&memory_type.value, memory_type.offset, "EfiMemoryType")?);
    writer.u64(parse_u64(&start.value, start.offset, "StartingAddress")?);
    writer.u64(parse_u64(&end.value, end.offset, "EndingAddress")?);
    writer.finish()
}

fn encode_vendor(
    node: &ParsedNode,
    arguments: &[ParsedArgument],
    r#type: u8,
    subtype: u8,
) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Guid", "Data"])?;
    let guid = args.required(0)?;
    let data = args
        .optional(1)
        .map_or(Ok(Vec::new()), |argument| parse_hex_bytes(&argument.value, argument.offset, "Data"))?;
    let mut writer = NodeWriter::new(node, r#type, subtype);
    writer.bytes(&parse_efi_guid(&guid.value, guid.offset, "Guid")?);
    writer.bytes(&data);
    writer.finish()
}

fn encode_vendor_defined(
    node: &ParsedNode,
    arguments: &[ParsedArgument],
    schema: &VendorDefinedSchema,
) -> Result<Vec<u8>, DevicePathError> {
    let names: Vec<&str> = schema.fields.iter().map(|field| field.name.as_str()).collect();
    let args = ResolvedArgs::new(node, arguments, &names)?;
    let (node_type, subtype) = schema.vendor_type.node_type_and_subtype();
    let mut writer = NodeWriter::new(node, node_type, subtype);
    writer.bytes(&schema.guid);

    for (index, field) in schema.fields.iter().enumerate() {
        let argument = args.required(index)?;
        match field.field_type {
            VendorDefinedFieldType::U8 => {
                writer.byte(parse_u8(&argument.value, argument.offset, &field.name)?);
            }
            VendorDefinedFieldType::U16Le => {
                writer.u16(parse_u16(&argument.value, argument.offset, &field.name)?);
            }
            VendorDefinedFieldType::U32Le => {
                writer.u32(parse_u32(&argument.value, argument.offset, &field.name)?);
            }
            VendorDefinedFieldType::U64Le => {
                writer.u64(parse_u64(&argument.value, argument.offset, &field.name)?);
            }
            VendorDefinedFieldType::Guid => {
                writer.bytes(&parse_efi_guid(&argument.value, argument.offset, &field.name)?);
            }
            VendorDefinedFieldType::Uuid => {
                writer.bytes(&parse_uuid_bytes(&argument.value, argument.offset, &field.name)?);
            }
            VendorDefinedFieldType::Bytes => {
                writer.bytes(&parse_hex_bytes(&argument.value, argument.offset, &field.name)?);
            }
        }
    }

    writer.finish()
}

fn encode_controller(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Controller"])?;
    let controller = args.required(0)?;
    let mut writer = NodeWriter::new(node, 1, 5);
    writer.u32(parse_u32(&controller.value, controller.offset, "Controller")?);
    writer.finish()
}

fn encode_bmc(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Type", "Address"])?;
    let interface_type = args.required(0)?;
    let address = args.required(1)?;
    let mut writer = NodeWriter::new(node, 1, 6);
    writer.byte(parse_u8(&interface_type.value, interface_type.offset, "Type")?);
    writer.u64(parse_u64(&address.value, address.offset, "Address")?);
    writer.finish()
}

fn encode_acpi(
    node: &ParsedNode,
    arguments: &[ParsedArgument],
    fixed_hid: Option<&str>,
) -> Result<Vec<u8>, DevicePathError> {
    let names: &'static [&'static str] = if fixed_hid.is_some() { &["UID"] } else { &["HID", "UID"] };
    let args = ResolvedArgs::new(node, arguments, names)?;
    let (hid, uid_index) = if let Some(hid) = fixed_hid {
        (parse_eisa_id(hid, node.offset, "HID")?, 0)
    } else {
        let hid = args.required(0)?;
        (parse_eisa_id(&hid.value, hid.offset, "HID")?, 1)
    };
    let uid = args.optional(uid_index).map_or(Ok(0), |argument| parse_u32(&argument.value, argument.offset, "UID"))?;

    let mut writer = NodeWriter::new(node, 2, 1);
    writer.u32(hid);
    writer.u32(uid);
    writer.finish()
}

fn encode_acpi_ex(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["HID", "CID", "UID", "HIDSTR", "CIDSTR", "UIDSTR"])?;
    let hid = args.optional(0).map_or(Ok(0), |argument| parse_eisa_id(&argument.value, argument.offset, "HID"))?;
    let cid = args.optional(1).map_or(Ok(0), |argument| parse_eisa_id(&argument.value, argument.offset, "CID"))?;
    let uid = args.optional(2).map_or(Ok(0), |argument| parse_u32(&argument.value, argument.offset, "UID"))?;
    let hid_string = args.text(3, "");
    let cid_string = args.text(4, "");
    let uid_string = args.text(5, "");

    if hid == 0 && hid_string.is_empty() {
        return Err(DevicePathError::new(node.offset, "either HID or HIDSTR is required"));
    }
    if cid != 0 && !cid_string.is_empty() {
        return Err(DevicePathError::new(args.offset(4), "CID and CIDSTR cannot both be non-default"));
    }
    if uid != 0 && !uid_string.is_empty() {
        return Err(DevicePathError::new(args.offset(5), "UID and UIDSTR cannot both be non-default"));
    }

    let mut writer = NodeWriter::new(node, 2, 2);
    writer.u32(hid);
    writer.u32(uid);
    writer.u32(cid);
    writer.ascii_nul(hid_string, "HIDSTR")?;
    writer.ascii_nul(uid_string, "UIDSTR")?;
    writer.ascii_nul(cid_string, "CIDSTR")?;
    writer.finish()
}

fn encode_acpi_exp(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["HID", "CID", "UIDSTR"])?;
    let hid = args.required(0)?;
    let cid = args.optional(1);
    let uid_string = args.required(2)?;
    let mut writer = NodeWriter::new(node, 2, 2);
    writer.u32(parse_eisa_id(&hid.value, hid.offset, "HID")?);
    writer.u32(0);
    writer.u32(cid.map_or(Ok(0), |argument| parse_eisa_id(&argument.value, argument.offset, "CID"))?);
    writer.ascii_nul("", "HIDSTR")?;
    writer.ascii_nul(&uid_string.value, "UIDSTR")?;
    writer.ascii_nul("", "CIDSTR")?;
    writer.finish()
}

fn encode_acpi_adr(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    if arguments.is_empty() {
        return Err(DevicePathError::new(node.offset, "at least one DisplayDevice is required"));
    }
    if let Some(argument) = arguments.iter().find(|argument| argument.name.is_some()) {
        return Err(DevicePathError::new(argument.offset, "AcpiAdr does not accept named parameters"));
    }
    let mut writer = NodeWriter::new(node, 2, 3);
    for argument in arguments {
        writer.u32(parse_u32(&argument.value, argument.offset, "DisplayDevice")?);
    }
    writer.finish()
}

fn encode_nvdimm_acpi_adr(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["NFITDeviceHandle"])?;
    let handle = args.required(0)?;
    let mut writer = NodeWriter::new(node, 2, 4);
    writer.u32(parse_u32(&handle.value, handle.offset, "NFITDeviceHandle")?);
    writer.finish()
}

fn encode_file_path(node: &ParsedNode, path: &str) -> Result<Vec<u8>, DevicePathError> {
    let mut writer = NodeWriter::new(node, 4, 4);
    writer.utf16(path, true);
    writer.finish()
}

fn encode_ata(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Controller", "Drive", "LUN"])?;
    let controller = args.required(0)?;
    let drive = args.required(1)?;
    let lun = args.required(2)?;
    let controller =
        parse_u8_keyword(&controller.value, controller.offset, "Controller", &[("Primary", 0), ("Secondary", 1)])?;
    let drive = parse_u8_keyword(&drive.value, drive.offset, "Drive", &[("Master", 0), ("Slave", 1)])?;
    if controller > 1 {
        return Err(DevicePathError::new(args.offset(0), "Controller must be Primary, Secondary, 0, or 1"));
    }
    if drive > 1 {
        return Err(DevicePathError::new(args.offset(1), "Drive must be Master, Slave, 0, or 1"));
    }

    let mut writer = NodeWriter::new(node, 3, 1);
    writer.byte(controller);
    writer.byte(drive);
    writer.u16(parse_u16(&lun.value, lun.offset, "LUN")?);
    writer.finish()
}

fn encode_scsi(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["PUN", "LUN"])?;
    let pun = args.required(0)?;
    let lun = args.required(1)?;
    let mut writer = NodeWriter::new(node, 3, 2);
    writer.u16(parse_u16(&pun.value, pun.offset, "PUN")?);
    writer.u16(parse_u16(&lun.value, lun.offset, "LUN")?);
    writer.finish()
}

fn encode_fibre(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["WWN", "LUN"])?;
    let wwn = args.required(0)?;
    let lun = args.required(1)?;
    let mut writer = NodeWriter::new(node, 3, 3);
    writer.u32(0);
    writer.u64(parse_u64(&wwn.value, wwn.offset, "WWN")?);
    writer.u64(parse_u64(&lun.value, lun.offset, "LUN")?);
    writer.finish()
}

fn encode_fibre_ex(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["WWN", "LUN"])?;
    let wwn = args.required(0)?;
    let lun = args.required(1)?;
    let mut writer = NodeWriter::new(node, 3, 21);
    writer.bytes(&parse_fixed_hex::<8>(&wwn.value, wwn.offset, "WWN")?);
    writer.bytes(&parse_fixed_hex::<8>(&lun.value, lun.offset, "LUN")?);
    writer.finish()
}

fn encode_i1394(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["GUID"])?;
    let guid = args.required(0)?;
    let mut writer = NodeWriter::new(node, 3, 4);
    writer.u32(0);
    writer.u64(parse_u64(&guid.value, guid.offset, "GUID")?);
    writer.finish()
}

fn encode_usb(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Port", "Interface"])?;
    let port = args.required(0)?;
    let interface = args.required(1)?;
    let mut writer = NodeWriter::new(node, 3, 5);
    writer.byte(parse_u8(&port.value, port.offset, "Port")?);
    writer.byte(parse_u8(&interface.value, interface.offset, "Interface")?);
    writer.finish()
}

fn encode_i2o(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["TID"])?;
    let tid = args.required(0)?;
    let mut writer = NodeWriter::new(node, 3, 6);
    writer.u32(parse_u32(&tid.value, tid.offset, "TID")?);
    writer.finish()
}

fn encode_infiniband(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Flags", "Guid", "ServiceId", "TargetId", "DeviceId"])?;
    let flags = args.required(0)?;
    let guid = args.required(1)?;
    let service_id = args.required(2)?;
    let target_id = args.required(3)?;
    let device_id = args.required(4)?;
    let mut writer = NodeWriter::new(node, 3, 9);
    writer.u32(parse_u32(&flags.value, flags.offset, "Flags")?);
    writer.bytes(&parse_efi_guid(&guid.value, guid.offset, "Guid")?);
    writer.u64(parse_u64(&service_id.value, service_id.offset, "ServiceId")?);
    writer.u64(parse_u64(&target_id.value, target_id.offset, "TargetId")?);
    writer.u64(parse_u64(&device_id.value, device_id.offset, "DeviceId")?);
    writer.finish()
}

fn encode_guid_vendor_shortcut(
    node: &ParsedNode,
    arguments: &[ParsedArgument],
    guid: &str,
) -> Result<Vec<u8>, DevicePathError> {
    ensure_no_arguments(node, arguments)?;
    let mut writer = NodeWriter::new(node, 3, 10);
    writer.bytes(&parse_efi_guid(guid, node.offset, "Guid")?);
    writer.finish()
}

fn encode_uart_flow_control(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Value"])?;
    let value = args.required(0)?;
    let value = parse_u8_keyword(&value.value, value.offset, "Value", &[("None", 0), ("Hardware", 1), ("XonXoff", 2)])?;
    if value > 2 {
        return Err(DevicePathError::new(args.offset(0), "Value must be None, Hardware, XonXoff, 0, 1, or 2"));
    }
    let mut writer = NodeWriter::new(node, 3, 10);
    writer.bytes(&parse_efi_guid(UART_FLOW_CONTROL_GUID, node.offset, "Guid")?);
    writer.u32(u32::from(value));
    writer.finish()
}

fn encode_sas(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(
        node,
        arguments,
        &["Address", "LUN", "RTP", "SASSATA", "Location", "Connect", "DriveBay", "Reserved"],
    )?;
    let address = args.required(0)?;
    let topology = encode_sas_topology(&args, 3, 4, 5, 6)?;
    let mut writer = NodeWriter::new(node, 3, 10);
    writer.bytes(&parse_efi_guid(SAS_GUID, node.offset, "Guid")?);
    writer.u32(args.optional(7).map_or(Ok(0), |argument| parse_u32(&argument.value, argument.offset, "Reserved"))?);
    writer.u64(parse_u64(&address.value, address.offset, "Address")?);
    writer.u64(args.optional(1).map_or(Ok(0), |argument| parse_u64(&argument.value, argument.offset, "LUN"))?);
    writer.u16(topology);
    writer.u16(args.optional(2).map_or(Ok(0), |argument| parse_u16(&argument.value, argument.offset, "RTP"))?);
    writer.finish()
}

fn encode_mac(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["MacAddr", "IfType"])?;
    let address = args.required(0)?;
    let if_type = args.optional(1).map_or(Ok(0), |argument| parse_u8(&argument.value, argument.offset, "IfType"))?;
    let address = parse_hex_bytes(&address.value, address.offset, "MacAddr")?;
    if address.len() > 32 {
        return Err(DevicePathError::new(args.offset(0), "MacAddr cannot exceed 32 bytes"));
    }
    if matches!(if_type, 0 | 1) && address.len() != 6 {
        return Err(DevicePathError::new(args.offset(0), "MacAddr must be exactly 6 bytes when IfType is 0 or 1"));
    }

    let mut writer = NodeWriter::new(node, 3, 11);
    writer.bytes(&address);
    writer.bytes(&vec![0; 32 - address.len()]);
    writer.byte(if_type);
    writer.finish()
}

fn encode_ipv4(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(
        node,
        arguments,
        &["RemoteIp", "Protocol", "Type", "LocalIp", "GatewayIPAddress", "SubnetMask"],
    )?;
    let remote = args.required(0)?;
    let protocol = parse_network_protocol(args.optional(1), "Protocol")?;
    let address_type = match args.text(2, "DHCP") {
        "DHCP" => 0,
        "Static" => 1,
        _ => return Err(DevicePathError::new(args.offset(2), "Type must be DHCP or Static")),
    };
    let local = parse_ipv4(args.text(3, "0.0.0.0"), args.offset(3), "LocalIp")?;
    let gateway = parse_ipv4(args.text(4, "0.0.0.0"), args.offset(4), "GatewayIPAddress")?;
    let subnet = parse_ipv4(args.text(5, "0.0.0.0"), args.offset(5), "SubnetMask")?;

    let mut writer = NodeWriter::new(node, 3, 12);
    writer.bytes(&local);
    writer.bytes(&parse_ipv4(&remote.value, remote.offset, "RemoteIp")?);
    writer.u16(0);
    writer.u16(0);
    writer.u16(u16::from(protocol));
    writer.byte(address_type);
    writer.bytes(&gateway);
    writer.bytes(&subnet);
    writer.finish()
}

fn encode_ipv6(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(
        node,
        arguments,
        &["RemoteIp", "Protocol", "IPAddressOrigin", "LocalIp", "PrefixLength", "GatewayIPAddress"],
    )?;
    let remote = args.required(0)?;
    let protocol = parse_network_protocol(args.optional(1), "Protocol")?;
    let origin = match args.text(2, "Static") {
        "Static" => 0,
        "StatelessAutoConfigure" => 1,
        "StatefulAutoConfigure" => 2,
        _ => {
            return Err(DevicePathError::new(
                args.offset(2),
                "IPAddressOrigin must be Static, StatelessAutoConfigure, or StatefulAutoConfigure",
            ));
        }
    };
    let local = parse_ipv6(args.text(3, "::"), args.offset(3), "LocalIp")?;
    let prefix =
        args.optional(4).map_or(Ok(0), |argument| parse_u8(&argument.value, argument.offset, "PrefixLength"))?;
    if prefix > 128 {
        return Err(DevicePathError::new(args.offset(4), "PrefixLength must be between 0 and 128"));
    }
    let gateway = parse_ipv6(args.text(5, "::"), args.offset(5), "GatewayIPAddress")?;

    let mut writer = NodeWriter::new(node, 3, 13);
    writer.bytes(&local);
    writer.bytes(&parse_ipv6(&remote.value, remote.offset, "RemoteIp")?);
    writer.u16(0);
    writer.u16(0);
    writer.u16(u16::from(protocol));
    writer.byte(origin);
    writer.byte(prefix);
    writer.bytes(&gateway);
    writer.finish()
}

fn encode_uart(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Baud", "DataBits", "Parity", "StopBits"])?;
    let baud = args.optional(0).map_or(Ok(115_200), |argument| parse_u64(&argument.value, argument.offset, "Baud"))?;
    let data_bits =
        args.optional(1).map_or(Ok(8), |argument| parse_u8(&argument.value, argument.offset, "DataBits"))?;
    let parity_text = args.text(2, "0");
    let parity_keyword = matches!(parity_text, "D" | "N" | "E" | "O" | "M" | "S");
    let parity = if parity_keyword {
        match parity_text {
            "D" => 0,
            "N" => 1,
            "E" => 2,
            "O" => 3,
            "M" => 4,
            "S" => 5,
            _ => unreachable!("matched all parity keywords"),
        }
    } else {
        parse_u8(parity_text, args.offset(2), "Parity")?
    };
    let stop_text = args.text(3, "0");
    let stop_bits = if parity_keyword {
        match stop_text {
            "D" => 0,
            "1" => 1,
            "1.5" => 2,
            "2" => 3,
            _ => {
                return Err(DevicePathError::new(args.offset(3), "StopBits must use keyword form with keyword Parity"));
            }
        }
    } else {
        if matches!(stop_text, "D" | "1.5") {
            return Err(DevicePathError::new(args.offset(3), "StopBits must use integer form with integer Parity"));
        }
        parse_u8(stop_text, args.offset(3), "StopBits")?
    };

    let mut writer = NodeWriter::new(node, 3, 14);
    writer.u32(0);
    writer.u64(baud);
    writer.byte(data_bits);
    writer.byte(parity);
    writer.byte(stop_bits);
    writer.finish()
}

fn encode_usb_class(
    node: &ParsedNode,
    arguments: &[ParsedArgument],
    fixed_class: Option<u8>,
    fixed_subclass: Option<u8>,
) -> Result<Vec<u8>, DevicePathError> {
    let names: &'static [&'static str] = match (fixed_class, fixed_subclass) {
        (None, None) => &["VID", "PID", "Class", "SubClass", "Protocol"],
        (Some(_), None) => &["VID", "PID", "SubClass", "Protocol"],
        (Some(_), Some(_)) => &["VID", "PID", "Protocol"],
        (None, Some(_)) => unreachable!("a fixed subclass requires a fixed class"),
    };
    let args = ResolvedArgs::new(node, arguments, names)?;
    let vid = args.optional(0).map_or(Ok(0xffff), |argument| parse_u16(&argument.value, argument.offset, "VID"))?;
    let pid = args.optional(1).map_or(Ok(0xffff), |argument| parse_u16(&argument.value, argument.offset, "PID"))?;
    let mut index = 2;
    let class = if let Some(class) = fixed_class {
        class
    } else {
        let value =
            args.optional(index).map_or(Ok(0xff), |argument| parse_u8(&argument.value, argument.offset, "Class"))?;
        index += 1;
        value
    };
    let subclass = if let Some(subclass) = fixed_subclass {
        subclass
    } else {
        let value =
            args.optional(index).map_or(Ok(0xff), |argument| parse_u8(&argument.value, argument.offset, "SubClass"))?;
        index += 1;
        value
    };
    let protocol =
        args.optional(index).map_or(Ok(0xff), |argument| parse_u8(&argument.value, argument.offset, "Protocol"))?;

    let mut writer = NodeWriter::new(node, 3, 15);
    writer.u16(vid);
    writer.u16(pid);
    writer.byte(class);
    writer.byte(subclass);
    writer.byte(protocol);
    writer.finish()
}

fn encode_usb_wwid(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["VID", "PID", "InterfaceNumber", "WWID"])?;
    let vid = args.required(0)?;
    let pid = args.required(1)?;
    let interface = args.required(2)?;
    let wwid = args.required(3)?;
    if wwid.value.encode_utf16().count() > 64 {
        return Err(DevicePathError::new(wwid.offset, "WWID cannot exceed 64 UTF-16 code units"));
    }

    let mut writer = NodeWriter::new(node, 3, 16);
    writer.u16(u16::from(parse_u8(&interface.value, interface.offset, "InterfaceNumber")?));
    writer.u16(parse_u16(&vid.value, vid.offset, "VID")?);
    writer.u16(parse_u16(&pid.value, pid.offset, "PID")?);
    writer.utf16(&wwid.value, false);
    writer.finish()
}

fn encode_unit(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["LUN"])?;
    let lun = args.required(0)?;
    let mut writer = NodeWriter::new(node, 3, 17);
    writer.byte(parse_u8(&lun.value, lun.offset, "LUN")?);
    writer.finish()
}

fn encode_sata(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["HPN", "PMPN", "LUN"])?;
    let hpn = args.required(0)?;
    let hpn = parse_u16(&hpn.value, hpn.offset, "HPN")?;
    if hpn == u16::MAX {
        return Err(DevicePathError::new(args.offset(0), "HPN must be between 0 and 65534"));
    }
    let lun = args.required(2)?;
    let mut writer = NodeWriter::new(node, 3, 18);
    writer.u16(hpn);
    writer.u16(args.optional(1).map_or(Ok(u16::MAX), |argument| parse_u16(&argument.value, argument.offset, "PMPN"))?);
    writer.u16(parse_u16(&lun.value, lun.offset, "LUN")?);
    writer.finish()
}

fn encode_iscsi(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(
        node,
        arguments,
        &["TargetName", "PortalGroup", "LUN", "HeaderDigest", "DataDigest", "Authentication", "Protocol"],
    )?;
    let target = args.required(0)?;
    if !target.value.is_ascii() || target.value.len() > 223 {
        return Err(DevicePathError::new(target.offset, "TargetName must be ASCII and cannot exceed 223 bytes"));
    }
    let portal_group = args.required(1)?;
    let lun = args.required(2)?;
    let header_digest = match args.text(3, "None") {
        "None" => 0,
        "CRC32C" => 2,
        _ => return Err(DevicePathError::new(args.offset(3), "HeaderDigest must be None or CRC32C")),
    };
    let data_digest = match args.text(4, "None") {
        "None" => 0,
        "CRC32C" => 2 << 2,
        _ => return Err(DevicePathError::new(args.offset(4), "DataDigest must be None or CRC32C")),
    };
    let authentication = match args.text(5, "None") {
        "CHAP_BI" => 0,
        "CHAP_UNI" => 1 << 12,
        "None" => 2 << 10,
        _ => return Err(DevicePathError::new(args.offset(5), "Authentication must be None, CHAP_BI, or CHAP_UNI")),
    };
    if args.text(6, "TCP") != "TCP" {
        return Err(DevicePathError::new(args.offset(6), "Protocol must be TCP"));
    }

    let mut writer = NodeWriter::new(node, 3, 19);
    writer.u16(0);
    writer.u16(header_digest | data_digest | authentication);
    writer.bytes(&parse_fixed_hex::<8>(&lun.value, lun.offset, "LUN")?);
    writer.u16(parse_u16(&portal_group.value, portal_group.offset, "PortalGroup")?);
    writer.ascii_nul(&target.value, "TargetName")?;
    writer.finish()
}

fn encode_vlan(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["VlanId"])?;
    let vlan_argument = args.required(0)?;
    let vlan = parse_u16(&vlan_argument.value, vlan_argument.offset, "VlanId")?;
    if vlan > 4094 {
        return Err(DevicePathError::new(vlan_argument.offset, "VlanId must be between 0 and 4094"));
    }
    let mut writer = NodeWriter::new(node, 3, 20);
    writer.u16(vlan);
    writer.finish()
}

fn encode_sas_ex(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args =
        ResolvedArgs::new(node, arguments, &["Address", "LUN", "RTP", "SASSATA", "Location", "Connect", "DriveBay"])?;
    let address = args.required(0)?;
    let topology = encode_sas_topology(&args, 3, 4, 5, 6)?;
    let mut writer = NodeWriter::new(node, 3, 22);
    writer.bytes(&parse_fixed_hex::<8>(&address.value, address.offset, "Address")?);
    writer.bytes(
        &args
            .optional(1)
            .map_or(Ok([0; 8]), |argument| parse_fixed_hex::<8>(&argument.value, argument.offset, "LUN"))?,
    );
    writer.u16(topology);
    writer.u16(args.optional(2).map_or(Ok(0), |argument| parse_u16(&argument.value, argument.offset, "RTP"))?);
    writer.finish()
}

fn encode_nvme(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["NSID", "EUI"])?;
    let namespace = args.required(0)?;
    let namespace = parse_u32(&namespace.value, namespace.offset, "NSID")?;
    if matches!(namespace, 0 | u32::MAX) {
        return Err(DevicePathError::new(args.offset(0), "NSID cannot be 0 or 0xffffffff"));
    }
    let eui = args.required(1)?;
    let eui = u64::from_be_bytes(parse_fixed_hex::<8>(&eui.value, eui.offset, "EUI")?);
    let mut writer = NodeWriter::new(node, 3, 23);
    writer.u32(namespace);
    writer.u64(eui);
    writer.finish()
}

fn encode_uri(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Uri"])?;
    let uri = args.text(0, "");
    if !uri.is_ascii() {
        return Err(DevicePathError::new(args.offset(0), "Uri must contain only ASCII characters"));
    }
    let mut writer = NodeWriter::new(node, 3, 24);
    writer.bytes(uri.as_bytes());
    writer.finish()
}

fn encode_ufs(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["PUN", "LUN"])?;
    let pun = args.required(0)?;
    if parse_u8(&pun.value, pun.offset, "PUN")? != 0 {
        return Err(DevicePathError::new(pun.offset, "PUN must be 0"));
    }
    let lun = args.required(1)?;
    let lun = parse_u8(&lun.value, lun.offset, "LUN")?;
    if !matches!(lun, 0..=7 | 0x81 | 0xd0 | 0xb0 | 0xc4) {
        return Err(DevicePathError::new(args.offset(1), "LUN is not a valid UFS logical unit"));
    }
    let mut writer = NodeWriter::new(node, 3, 25);
    writer.byte(0);
    writer.byte(lun);
    writer.finish()
}

fn encode_sd_emmc(node: &ParsedNode, arguments: &[ParsedArgument], subtype: u8) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["SlotNumber"])?;
    let slot = args.optional(0).map_or(Ok(0), |argument| parse_u8(&argument.value, argument.offset, "SlotNumber"))?;
    let mut writer = NodeWriter::new(node, 3, subtype);
    writer.byte(slot);
    writer.finish()
}

fn encode_bluetooth(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["BD_ADDR"])?;
    let address = args.required(0)?;
    let mut writer = NodeWriter::new(node, 3, 27);
    writer.bytes(&parse_fixed_hex::<6>(&address.value, address.offset, "BD_ADDR")?);
    writer.finish()
}

fn encode_wifi(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["SSID"])?;
    let ssid = args.required(0)?;
    if ssid.value.len() > 32 {
        return Err(DevicePathError::new(ssid.offset, "SSID cannot exceed 32 UTF-8 bytes"));
    }
    let mut writer = NodeWriter::new(node, 3, 28);
    writer.bytes(ssid.value.as_bytes());
    writer.bytes(&vec![0; 32 - ssid.value.len()]);
    writer.finish()
}

fn encode_bluetooth_le(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["BD_ADDR", "AddressType"])?;
    let address = args.required(0)?;
    let address_type = args.required(1)?;
    let address_type = parse_u8(&address_type.value, address_type.offset, "AddressType")?;
    if address_type > 1 {
        return Err(DevicePathError::new(args.offset(1), "AddressType must be 0 (public) or 1 (random)"));
    }
    let mut writer = NodeWriter::new(node, 3, 30);
    writer.bytes(&parse_fixed_hex::<6>(&address.value, address.offset, "BD_ADDR")?);
    writer.byte(address_type);
    writer.finish()
}

fn encode_dns(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    if arguments.is_empty() {
        return Err(DevicePathError::new(node.offset, "at least one DnsServerIp is required"));
    }
    if let Some(argument) = arguments.iter().find(|argument| argument.name.is_some()) {
        return Err(DevicePathError::new(argument.offset, "Dns does not accept named parameters"));
    }

    let first_is_ipv6 = arguments
        .first()
        .expect("an empty DNS argument list is rejected above")
        .value
        .parse::<std::net::Ipv6Addr>()
        .is_ok();
    let mut writer = NodeWriter::new(node, 3, 31);
    writer.byte(u8::from(first_is_ipv6));
    for argument in arguments {
        if first_is_ipv6 {
            writer.bytes(&parse_ipv6(&argument.value, argument.offset, "DnsServerIp")?);
        } else {
            writer.bytes(&parse_ipv4(&argument.value, argument.offset, "DnsServerIp")?);
            writer.bytes(&[0; 12]);
        }
    }
    writer.finish()
}

fn encode_nvdimm(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["UUID"])?;
    let uuid = args.required(0)?;
    let mut writer = NodeWriter::new(node, 3, 32);
    writer.bytes(&parse_efi_guid(&uuid.value, uuid.offset, "UUID")?);
    writer.finish()
}

fn encode_rest_service(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    if let Some(argument) = arguments.iter().find(|argument| argument.name.is_some()) {
        return Err(DevicePathError::new(argument.offset, "RestService does not accept named parameters"));
    }
    if arguments.len() < 2 {
        return Err(DevicePathError::new(node.offset, "RestExServiceType and AccessMode are required"));
    }
    let service_argument = arguments.first().expect("fewer than two REST arguments are rejected above");
    let access_argument = arguments.get(1).expect("fewer than two REST arguments are rejected above");
    let service = parse_u8(&service_argument.value, service_argument.offset, "RestExServiceType")?;
    let access = parse_u8(&access_argument.value, access_argument.offset, "AccessMode")?;
    if !matches!(access, 1 | 2) {
        return Err(DevicePathError::new(access_argument.offset, "AccessMode must be 1 or 2"));
    }

    let mut writer = NodeWriter::new(node, 3, 33);
    writer.byte(service);
    writer.byte(access);
    match service {
        1 | 2 => {
            if arguments.len() != 2 {
                return Err(DevicePathError::new(
                    arguments.get(2).expect("the argument list has more than two entries").offset,
                    "standard REST services accept exactly two parameters",
                ));
            }
        }
        0xff => {
            let guid = arguments
                .get(2)
                .filter(|argument| !argument.value.is_empty())
                .ok_or_else(|| DevicePathError::new(node.offset, "VendorRestServiceGuid is required"))?;
            writer.bytes(&parse_efi_guid(&guid.value, guid.offset, "VendorRestServiceGuid")?);
            for argument in arguments.get(3..).expect("a vendor REST service has a GUID argument") {
                writer.byte(parse_u8(&argument.value, argument.offset, "VendorDefinedData")?);
            }
        }
        _ => return Err(DevicePathError::new(service_argument.offset, "RestExServiceType must be 1, 2, or 0xff")),
    }
    writer.finish()
}

fn encode_nvme_of(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["SubsystemNQN", "NID"])?;
    let nqn = args.required(0)?;
    if nqn.value.as_bytes().contains(&0) || nqn.value.len() + 1 > 224 {
        return Err(DevicePathError::new(nqn.offset, "SubsystemNQN must be at most 223 UTF-8 bytes"));
    }
    let nid = args.required(1)?;
    let (nidt, nid_bytes) = parse_nvme_of_nid(&nid.value, nid.offset)?;
    let mut writer = NodeWriter::new(node, 3, 34);
    writer.byte(nidt);
    writer.bytes(&nid_bytes);
    writer.bytes(nqn.value.as_bytes());
    writer.byte(0);
    writer.finish()
}

fn encode_hard_drive(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Partition", "Type", "Signature", "Start", "Size"])?;
    let partition =
        args.optional(0).map_or(Ok(0), |argument| parse_u32(&argument.value, argument.offset, "Partition"))?;
    let type_text = args.text(1, "GPT");
    let signature = args.required(2)?;
    let (format, signature_type, signature_bytes) = match type_text {
        "MBR" | "1" | "0x1" | "0X1" => {
            let mut bytes = [0; 16];
            bytes[..4].copy_from_slice(&parse_u32(&signature.value, signature.offset, "Signature")?.to_le_bytes());
            (1, 1, bytes)
        }
        "GPT" | "2" | "0x2" | "0X2" => (2, 2, parse_efi_guid(&signature.value, signature.offset, "Signature")?),
        _ => {
            let signature_type = parse_u8(type_text, args.offset(1), "Type")?;
            if signature.value != "0" {
                return Err(DevicePathError::new(signature.offset, "Signature must be 0 when Type is not MBR or GPT"));
            }
            (0, signature_type, [0; 16])
        }
    };
    let (start, size) = if partition == 0 {
        if args.optional(3).is_some() || args.optional(4).is_some() {
            return Err(DevicePathError::new(args.offset(3), "Start and Size are prohibited when Partition is 0"));
        }
        (0, 0)
    } else {
        let start = args.required(3)?;
        let size = args.required(4)?;
        (parse_u64(&start.value, start.offset, "Start")?, parse_u64(&size.value, size.offset, "Size")?)
    };

    let mut writer = NodeWriter::new(node, 4, 1);
    writer.u32(partition);
    writer.u64(start);
    writer.u64(size);
    writer.bytes(&signature_bytes);
    writer.byte(format);
    writer.byte(signature_type);
    writer.finish()
}

fn encode_cdrom(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Entry", "Start", "Size"])?;
    let start = args.required(1)?;
    let size = args.required(2)?;
    let mut writer = NodeWriter::new(node, 4, 2);
    writer.u32(args.optional(0).map_or(Ok(0), |argument| parse_u32(&argument.value, argument.offset, "Entry"))?);
    writer.u64(parse_u64(&start.value, start.offset, "Start")?);
    writer.u64(parse_u64(&size.value, size.offset, "Size")?);
    writer.finish()
}

fn encode_media(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Guid"])?;
    let guid = args.required(0)?;
    let mut writer = NodeWriter::new(node, 4, 5);
    writer.bytes(&parse_efi_guid(&guid.value, guid.offset, "Guid")?);
    writer.finish()
}

fn encode_fv(node: &ParsedNode, arguments: &[ParsedArgument], subtype: u8) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Guid"])?;
    let guid = args.required(0)?;
    let mut writer = NodeWriter::new(node, 4, subtype);
    writer.bytes(&parse_efi_guid(&guid.value, guid.offset, "Guid")?);
    writer.finish()
}

fn encode_offset(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["StartingOffset", "EndingOffset"])?;
    let start = args.required(0)?;
    let end = args.required(1)?;
    let mut writer = NodeWriter::new(node, 4, 8);
    writer.u32(0);
    writer.u64(parse_u64(&start.value, start.offset, "StartingOffset")?);
    writer.u64(parse_u64(&end.value, end.offset, "EndingOffset")?);
    writer.finish()
}

fn encode_ram_disk(
    node: &ParsedNode,
    arguments: &[ParsedArgument],
    fixed_guid: Option<&str>,
) -> Result<Vec<u8>, DevicePathError> {
    let names: &'static [&'static str] = if fixed_guid.is_some() {
        &["StartingAddress", "EndingAddress", "DiskInstance"]
    } else {
        &["StartingAddress", "EndingAddress", "DiskInstance", "DiskTypeGuid"]
    };
    let args = ResolvedArgs::new(node, arguments, names)?;
    let start = args.required(0)?;
    let end = args.required(1)?;
    let instance =
        args.optional(2).map_or(Ok(0), |argument| parse_u16(&argument.value, argument.offset, "DiskInstance"))?;
    let guid = if let Some(guid) = fixed_guid {
        parse_efi_guid(guid, node.offset, "DiskTypeGuid")?
    } else {
        let guid = args.required(3)?;
        parse_efi_guid(&guid.value, guid.offset, "DiskTypeGuid")?
    };
    let mut writer = NodeWriter::new(node, 4, 9);
    writer.u64(parse_u64(&start.value, start.offset, "StartingAddress")?);
    writer.u64(parse_u64(&end.value, end.offset, "EndingAddress")?);
    writer.bytes(&guid);
    writer.u16(instance);
    writer.finish()
}

fn encode_bbs(node: &ParsedNode, arguments: &[ParsedArgument]) -> Result<Vec<u8>, DevicePathError> {
    let args = ResolvedArgs::new(node, arguments, &["Type", "Id", "Flags"])?;
    let device_type = args.required(0)?;
    let description = args.required(1)?;
    let device_type = parse_u16_keyword(
        &device_type.value,
        device_type.offset,
        "Type",
        &[("Floppy", 1), ("HD", 2), ("CDROM", 3), ("PCMCIA", 4), ("USB", 5), ("Network", 6)],
    )?;
    let mut writer = NodeWriter::new(node, 5, 1);
    writer.u16(device_type);
    writer.u16(args.optional(2).map_or(Ok(0), |argument| parse_u16(&argument.value, argument.offset, "Flags"))?);
    writer.ascii_nul(&description.value, "Id")?;
    writer.finish()
}

fn parse_u8_keyword(value: &str, offset: usize, field: &str, keywords: &[(&str, u8)]) -> Result<u8, DevicePathError> {
    keywords
        .iter()
        .find_map(|(keyword, value_for_keyword)| (*keyword == value).then_some(*value_for_keyword))
        .map_or_else(|| parse_u8(value, offset, field), Ok)
}

fn parse_u16_keyword(
    value: &str,
    offset: usize,
    field: &str,
    keywords: &[(&str, u16)],
) -> Result<u16, DevicePathError> {
    keywords
        .iter()
        .find_map(|(keyword, value_for_keyword)| (*keyword == value).then_some(*value_for_keyword))
        .map_or_else(|| parse_u16(value, offset, field), Ok)
}

fn parse_network_protocol(argument: Option<&ParsedArgument>, field: &str) -> Result<u8, DevicePathError> {
    argument.map_or(Ok(17), |argument| {
        parse_u8_keyword(&argument.value, argument.offset, field, &[("UDP", 17), ("TCP", 6)])
    })
}

fn encode_sas_topology(
    args: &ResolvedArgs<'_, '_>,
    sassata_index: usize,
    location_index: usize,
    connect_index: usize,
    drive_bay_index: usize,
) -> Result<u16, DevicePathError> {
    let sassata = args.text(sassata_index, "NoTopology");
    if sassata == "NoTopology" {
        reject_present(args, &[location_index, connect_index, drive_bay_index], "topology details")?;
        return Ok(0);
    }
    if let Ok(topology) = parse_u16(sassata, args.offset(sassata_index), "SASSATA") {
        reject_present(args, &[location_index, connect_index, drive_bay_index], "topology details")?;
        return Ok(topology);
    }
    if !matches!(sassata, "SAS" | "SATA") {
        return Err(DevicePathError::new(
            args.offset(sassata_index),
            "SASSATA must be SAS, SATA, NoTopology, or a 16-bit integer",
        ));
    }

    let location = args.required(location_index)?;
    let connect = args.required(connect_index)?;
    let location = parse_u8_keyword(&location.value, location.offset, "Location", &[("Internal", 0), ("External", 1)])?;
    if location > 1 {
        return Err(DevicePathError::new(args.offset(location_index), "Location must be Internal, External, 0, or 1"));
    }
    let connect = parse_u8_keyword(&connect.value, connect.offset, "Connect", &[("Direct", 0), ("Expanded", 1)])?;
    if connect > 3 {
        return Err(DevicePathError::new(args.offset(connect_index), "Connect must be between 0 and 3"));
    }
    let drive_bay = args.optional(drive_bay_index).map(|argument| {
        let drive_bay = parse_unsigned(&argument.value, 16, argument.offset, "DriveBay")?;
        if !(1..=256).contains(&drive_bay) {
            return Err(DevicePathError::new(argument.offset, "DriveBay must be between 1 and 256"));
        }
        Ok((drive_bay - 1) as u8)
    });
    let information = if drive_bay.is_some() { 2 } else { 1 };
    let device_type = u16::from(sassata == "SATA") | (u16::from(location) << 1);
    Ok(information
        | (device_type << 4)
        | (u16::from(connect) << 6)
        | (u16::from(drive_bay.transpose()?.unwrap_or(0)) << 8))
}

fn reject_present(args: &ResolvedArgs<'_, '_>, indices: &[usize], description: &str) -> Result<(), DevicePathError> {
    if let Some(index) = indices.iter().copied().find(|index| args.optional(*index).is_some()) {
        return Err(DevicePathError::new(
            args.offset(index),
            format!("{description} are prohibited for this SASSATA value"),
        ));
    }
    Ok(())
}

fn parse_nvme_of_nid(value: &str, offset: usize) -> Result<(u8, [u8; 16]), DevicePathError> {
    if let Some(value) = value.strip_prefix("eui.") {
        let eui = parse_fixed_hex::<8>(value, offset + 4, "NID")?;
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&eui);
        return Ok((1, bytes));
    }
    if let Some(value) = value.strip_prefix("nguid.") {
        return Ok((2, parse_fixed_hex::<16>(value, offset + 6, "NID")?));
    }
    if let Some(value) = value.strip_prefix("urn:uuid:") {
        return Ok((3, parse_uuid_bytes(value, offset + 9, "NID")?));
    }
    Err(DevicePathError::new(offset, "NID must use eui., nguid., or urn:uuid: namespace identifier syntax"))
}

#[cfg(test)]
mod tests {
    use crate::device_path_encoder::encode_device_path;

    #[test]
    fn test_encode_every_node_spelling() {
        for path in [
            "Path(1,1,00)",
            "HardwarePath(1,00)",
            "Pci(1,0)",
            "PcCard(0)",
            "MemoryMapped(0,0,0)",
            "VenHw(00112233-4455-6677-8899-aabbccddeeff)",
            "Ctrl(0)",
            "BMC(0,0)",
            "AcpiPath(1,0000000000000000)",
            "Acpi(PNP0A03,0)",
            "PciRoot()",
            "PcieRoot()",
            "Floppy()",
            "Keyboard()",
            "Serial()",
            "ParallelPort()",
            "AcpiEx(PNP0A03,,,,,)",
            "AcpiExp(PNP0A03,,Root)",
            "AcpiAdr(0)",
            "NvdimmAcpiAdr(0)",
            "Msg(1,00)",
            "Ata(Primary,Master,0)",
            "Scsi(0,0)",
            "Fibre(0,0)",
            "FibreEx(0000000000000000,0000000000000000)",
            "I1394(0)",
            "USB(0,0)",
            "I2O(0)",
            "Infiniband(0,00112233-4455-6677-8899-aabbccddeeff,0,0,0)",
            "VenMsg(00112233-4455-6677-8899-aabbccddeeff)",
            "VenPcAnsi()",
            "VenVt100()",
            "VenVt100Plus()",
            "VenUtf8()",
            "UartFlowCtrl(None)",
            "SAS(0)",
            "DebugPort()",
            "MAC(001122334455,1)",
            "IPv4(192.0.2.1)",
            "IPv6(2001:db8::1,TCP,Static,2001:db8::2,64,2001:db8::ffff)",
            "Uart()",
            "UsbClass()",
            "UsbAudio()",
            "UsbCDCControl()",
            "UsbHID()",
            "UsbImage()",
            "UsbPrinter()",
            "UsbMassStorage()",
            "UsbHub()",
            "UsbCDCData()",
            "UsbSmartCard()",
            "UsbVideo()",
            "UsbDiagnostic()",
            "UsbWireless()",
            "UsbDeviceFirmwareUpdate()",
            "UsbIrdaBridge()",
            "UsbTestAndMeasurement()",
            "UsbWwid(0,0,0,WWID)",
            "Unit(0)",
            "Sata(0,,0)",
            "iSCSI(iqn.test,0,0000000000000000)",
            "Vlan(0)",
            "SasEx(0000000000000000)",
            "NVMe(1,00-00-00-00-00-00-00-00)",
            "Uri(https://example.com/)",
            "UFS(0,0)",
            "SD()",
            "Bluetooth(001122334455)",
            "Wi-Fi(SSID)",
            "eMMC()",
            "BluetoothLE(001122334455,0)",
            "Dns(192.0.2.53)",
            "NVDIMM(00112233-4455-6677-8899-aabbccddeeff)",
            "RestService(1,1)",
            "NVMEoF(nqn.2014-08.org.nvmexpress:test,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            "MediaPath(5,00000000000000000000000000000000)",
            "HD(0,GPT,00112233-4455-6677-8899-aabbccddeeff)",
            "CDROM(0,0,0)",
            "VenMedia(00112233-4455-6677-8899-aabbccddeeff)",
            "EFI",
            "Media(00112233-4455-6677-8899-aabbccddeeff)",
            "FvFile(00112233-4455-6677-8899-aabbccddeeff)",
            "Fv(00112233-4455-6677-8899-aabbccddeeff)",
            "Offset(0,0)",
            "RamDisk(0,0,0,00112233-4455-6677-8899-aabbccddeeff)",
            "VirtualDisk(0,0)",
            "VirtualCD(0,0)",
            "PersistentVirtualDisk(0,0)",
            "PersistentVirtualCD(0,0)",
            "BbsPath(1,00000000)",
            "BBS(USB,Device)",
        ] {
            encode_device_path(path).unwrap_or_else(|error| panic!("{path}: {error}"));
        }
    }

    #[test]
    fn test_encode_messaging_media_and_bbs_nodes() {
        for path in [
            "Ata(Primary,Master,0)",
            "IPv4(192.0.2.1,TCP,Static,192.0.2.2,192.0.2.254,255.255.255.0)",
            "SasEx(0102030405060708)",
            "NVDIMM(00112233-4455-6677-8899-aabbccddeeff)",
            "RestService(0xff,1,00112233-4455-6677-8899-aabbccddeeff,0xaa,0x55)",
            "NVMEoF(nqn.2014-08.org.nvmexpress:uuid:test,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
            "HD(1,GPT,00112233-4455-6677-8899-aabbccddeeff,0x800,0x1000)",
            "BBS(USB,\"USB Device\",0)",
        ] {
            encode_device_path(path).unwrap_or_else(|error| panic!("{path}: {error}"));
        }
    }

    #[test]
    fn test_nvdimm_uses_efi_guid_byte_order() {
        let encoded = encode_device_path("NVDIMM(00112233-4455-6677-8899-aabbccddeeff)").expect("path should encode");
        assert_eq!(
            &encoded[4..20],
            &[0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]
        );
    }

    #[test]
    fn test_nvme_of_uuid_uses_rfc_byte_order() {
        let encoded = encode_device_path(
            "NVMEoF(nqn.2014-08.org.nvmexpress:uuid:test,urn:uuid:00112233-4455-6677-8899-aabbccddeeff)",
        )
        .expect("path should encode");
        assert_eq!(
            &encoded[5..21],
            &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]
        );
    }

    #[test]
    fn test_defaults_named_arguments_and_numeric_bases_are_equivalent() {
        assert_eq!(
            encode_device_path("PciRoot()").expect("shortcut should encode"),
            encode_device_path("Acpi(PNP0A03,0)").expect("canonical form should encode")
        );
        assert_eq!(
            encode_device_path("Pci(Function=0,Device=0x11)").expect("named form should encode"),
            encode_device_path("Pci(17,0)").expect("positional form should encode")
        );
        assert_eq!(
            encode_device_path("Uart()").expect("default form should encode"),
            encode_device_path("Uart(115200,8,0,0)").expect("explicit form should encode")
        );
        assert_eq!(
            encode_device_path("UsbClass()").expect("default form should encode"),
            encode_device_path("UsbClass(0xffff,65535,0xff,255,0xff)").expect("explicit form should encode")
        );
    }

    #[test]
    fn test_vendor_usb_and_ram_disk_aliases_match_canonical_forms() {
        assert_eq!(
            encode_device_path("VenPcAnsi()").expect("shortcut should encode"),
            encode_device_path("VenMsg(e0c14753-f9be-11d2-9a0c-0090273fc14d)").expect("canonical form should encode")
        );
        assert_eq!(
            encode_device_path("UsbAudio()").expect("shortcut should encode"),
            encode_device_path("UsbClass(0xffff,0xffff,1,0xff,0xff)").expect("canonical form should encode")
        );
        assert_eq!(
            encode_device_path("VirtualDisk(1,2)").expect("shortcut should encode"),
            encode_device_path("RamDisk(1,2,0,77ab535a-45fc-624b-5560-f7b281d1f96e)")
                .expect("canonical form should encode")
        );
    }

    #[test]
    fn test_encode_vendor_rest_service_golden_vector() {
        assert_eq!(
            encode_device_path("RestService(0xff,1,00112233-4455-6677-8899-aabbccddeeff,0xaa,0x55)")
                .expect("path should encode"),
            [
                0x03, 0x21, 0x18, 0x00, 0xff, 0x01, 0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa,
                0xbb, 0xcc, 0xdd, 0xee, 0xff, 0xaa, 0x55, 0x7f, 0xff, 0x04, 0x00,
            ]
        );
    }

    #[test]
    fn test_encode_sas_ex_topology_golden_vector() {
        assert_eq!(
            encode_device_path("SasEx(0102030405060708,0001020304050607,0x1234,SATA,External,Expanded,256)")
                .expect("path should encode"),
            [
                0x03, 0x16, 0x18, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x00, 0x01, 0x02, 0x03, 0x04,
                0x05, 0x06, 0x07, 0x72, 0xff, 0x34, 0x12, 0x7f, 0xff, 0x04, 0x00,
            ]
        );
    }

    #[test]
    fn test_nvme_of_identifier_forms() {
        let eui = encode_device_path("NVMEoF(nqn.test,eui.0011223344556677)").expect("EUI should encode");
        assert_eq!(
            eui.get(4..21).expect("NVMe-oF node contains NIDT and NID"),
            &[0x01, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );

        let nguid =
            encode_device_path("NVMEoF(nqn.test,nguid.00112233445566778899aabbccddeeff)").expect("NGUID should encode");
        assert_eq!(
            nguid.get(4..21).expect("NVMe-oF node contains NIDT and NID"),
            &[0x02, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]
        );
    }

    #[test]
    fn test_rejects_cross_field_constraint_violations() {
        assert!(encode_device_path("Dns(192.0.2.1,2001:db8::1)").is_err());
        assert!(encode_device_path("Uart(115200,8,N,0)").is_err());
        assert!(encode_device_path("HD(0,GPT,00112233-4455-6677-8899-aabbccddeeff,1,1)").is_err());
        assert!(encode_device_path("Sata(65535,,0)").is_err());
        assert!(encode_device_path("SAS(1,0,0,NoTopology,Internal,Direct)").is_err());
    }
}
