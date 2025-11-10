#![feature(coverage_attribute)]
//! A tool that resolves raw stack traces using offline PDB parsing. It reads
//! symbols for each frame and prints the resolved stack trace showing source
//! file locations, demangled function names, and instruction offsets.
//!
//! This tool is meant to be invoked via `./resolve_stacktrace.cmd` or
//! `./resolve_stacktrace.sh`.
//!
//! For more details, see the `README.md` in the stack trace module.
use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL};
use pdb_addr2line::pdb;
use std::{
    fs::File,
    io::{self, BufReader, Write},
    path::{Path, PathBuf},
};

#[derive(Debug)]
struct StackFrame {
    frame_number: String,
    child_stack_pointer: String,
    return_address: String,
    module_name: String,
    start_rva: u32,

    file: Option<String>,
    line: Option<u32>,
    function: Option<String>,
    offset: u32,

    // If any error occurred when resolving stack frame information, store it here
    error: Option<String>,
}

/// Look up debug info for each parsed stack frame and attach file, line, and
/// symbol data. Coverage is off because this function depends on external PDB
/// files
#[coverage(off)]
fn resolve_stack_frames(pdb_directory: &Path, mut stack_frames: Vec<StackFrame>) -> Vec<StackFrame> {
    for stack_frame in &mut stack_frames {
        let mut pdb_path: PathBuf = pdb_directory.join(&stack_frame.module_name);
        pdb_path.set_extension("pdb");

        let Ok(file) = File::open(&pdb_path) else {
            stack_frame.error = Some(format!("Failed to open {:?}", pdb_path));
            continue;
        };

        let reader = BufReader::new(file);
        let Ok(pdb) = pdb::PDB::open(reader) else {
            stack_frame.error = Some(format!("Failed to parse PDB {:?}", pdb_path));
            continue;
        };

        let Ok(context_data) = pdb_addr2line::ContextPdbData::try_from_pdb(pdb) else {
            stack_frame.error = Some(format!("Failed to create context data from PDB {:?}", pdb_path));
            continue;
        };

        let Ok(context) = context_data.make_context() else {
            stack_frame.error = Some(format!("Failed to create context from PDB {:?}", pdb_path));
            continue;
        };

        let Ok(Some(frames)) = context.find_frames(stack_frame.start_rva) else {
            stack_frame.error = Some(format!("Failed to find frames in context for {:?}", stack_frame.start_rva));
            continue;
        };

        let Some(frame) = frames.frames.last() else {
            stack_frame.error = Some(format!("No frames found for RVA 0x{:X}", stack_frame.start_rva));
            continue;
        };

        let function_start_rva = frames.start_rva;
        let file = frame.file.as_deref().unwrap_or("<unknown>").to_string();
        let line = frame.line.unwrap_or(0);
        let function = frame.function.as_deref().unwrap_or("<unknown>").to_string();
        let offset = stack_frame.start_rva - function_start_rva;

        stack_frame.file = Some(file);
        stack_frame.line = Some(line);
        stack_frame.function = Some(function);
        stack_frame.offset = offset;
    }

    stack_frames
}

/// Convert a single textual stack trace line into a structured `StackFrame`.
fn create_stack_frame(line: &str) -> Option<StackFrame> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }

    let idx = parts.len();

    // Parse each column backwards
    let (module_name, start_rva_str) = parts[idx - 1].rsplit_once('+')?;
    let start_rva_str = start_rva_str.strip_prefix("0x").unwrap_or(start_rva_str);
    let start_rva = u32::from_str_radix(start_rva_str, 16).ok()?;
    let return_address = parts[idx - 2].to_string();
    let child_stack_pointer = parts[idx - 3].to_string();
    let frame_number = parts[idx - 4].to_string();

    Some(StackFrame {
        frame_number,
        child_stack_pointer,
        return_address,
        module_name: module_name.to_string(),
        start_rva,
        file: None,     // filled by resolver
        line: None,     // filled by resolver
        function: None, // filled by resolver
        offset: 0,      // filled by resolver
        error: None,    // filled by resolver
    })
}

/// Collect the PDB directory and stack trace text from stdin. Coverage is off
/// because this is I/O code.
#[coverage(off)]
fn read_inputs() -> Result<(PathBuf, Vec<String>), String> {
    let mut pdb_directory = String::new();
    print!("Enter the PDB directory path (leave empty to use STACKTRACE_PDB_DIR env): ");
    io::stdout().flush().map_err(|e| format!("Failed to flush stdout: {}", e))?;
    io::stdin().read_line(&mut pdb_directory).map_err(|e| format!("Failed to read PDB directory from stdin: {}", e))?;

    let mut pdb_directory = pdb_directory.trim().to_owned();
    if pdb_directory.is_empty() {
        // Read from STACKTRACE_PDB_DIR environment variable
        if let Ok(env_dir) = std::env::var("STACKTRACE_PDB_DIR") {
            if !env_dir.is_empty() {
                pdb_directory = env_dir;
            }
        } else {
            return Err("PDB directory not provided or set in STACKTRACE_PDB_DIR".to_string());
        }
    }

    if pdb_directory.is_empty() {
        return Err("PDB directory path cannot be empty".to_string());
    }

    let pdb_directory = PathBuf::from(pdb_directory);

    println!("Enter stack trace lines (press Enter twice to finish):");
    let mut stacktrace = vec![];
    loop {
        let mut line = String::new();
        let bytes_read = io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read stack trace line from stdin: {}", e))?;

        if bytes_read == 0 {
            break;
        }

        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }

        stacktrace.push(trimmed.to_string());
    }

    Ok((pdb_directory, stacktrace))
}

/// Parse the stack trace text into a list of stack frames, skipping headers.
fn create_stack_frames(stack_frames: Vec<String>) -> Vec<StackFrame> {
    stack_frames
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }

            // Skip header line
            if line.contains("Return Address") {
                return None;
            }

            create_stack_frame(line)
        })
        .collect()
}

/// Render the resolved stack frames as a formatted table for display. Coverage
/// is off because this function do not return a value.
#[coverage(off)]
fn dump_stack_frames(stack_frames: Vec<StackFrame>) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL).set_content_arrangement(ContentArrangement::DynamicFullWidth).set_header(vec![
        Cell::new("#").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("Source Path").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("Child-SP").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("Return Address").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("Call Site").add_attribute(comfy_table::Attribute::Bold),
    ]);

    for frame in &stack_frames {
        let source_path = frame.file.as_deref().unwrap_or(frame.error.as_deref().unwrap_or("<unknown>"));
        let source_path = format!("{} @ {}", source_path, frame.line.unwrap_or(0));
        let call_site =
            format!("{}!{}+0x{:X}", frame.module_name, frame.function.as_deref().unwrap_or("<unknown>"), frame.offset);

        table.add_row(vec![
            frame.frame_number.clone(),
            source_path,
            frame.child_stack_pointer.clone(),
            frame.return_address.clone(),
            call_site,
        ]);
    }

    println!("{table}");
}

/// Entry point: read inputs, resolve frames, and print the resolved table.
fn main() -> Result<(), String> {
    let (pdb_directory, stacktrace) = read_inputs()?;

    let stack_frames = create_stack_frames(stacktrace);
    let stack_frames = resolve_stack_frames(&pdb_directory, stack_frames);

    dump_stack_frames(stack_frames);

    Ok(())
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_create_stack_frame_valid() {
        let line = "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore+0x45a3";
        let frame = create_stack_frame(line).expect("Should parse valid frame");

        assert_eq!(frame.frame_number, "00");
        assert_eq!(frame.child_stack_pointer, "000000cd7bbfe830");
        assert_eq!(frame.return_address, "00007ff6ddd0b4ae");
        assert_eq!(frame.module_name, "DxeCore");
        assert_eq!(frame.start_rva, 0x45a3);
        assert!(frame.file.is_none());
        assert!(frame.line.is_none());
        assert!(frame.function.is_none());
        assert_eq!(frame.offset, 0);
        assert!(frame.error.is_none());
    }

    #[test]
    fn test_create_stack_frame_hex_variations() {
        let line = "05 000000cd7bbfe900 00007ff6ddd0ffff TestModule+0xABCD";
        let frame = create_stack_frame(line).expect("Should parse uppercase hex");
        assert_eq!(frame.start_rva, 0xABCD);

        let line2 = "05 000000cd7bbfe900 00007ff6ddd0ffff TestModule+0xabcd";
        let frame2 = create_stack_frame(line2).expect("Should parse lowercase hex");
        assert_eq!(frame2.start_rva, 0xabcd);
    }

    #[test]
    fn test_create_stack_frame_invalid_too_few_parts() {
        let line = "00 000000cd7bbfe830 DxeCore+0x45a3";
        assert!(create_stack_frame(line).is_none());
    }

    #[test]
    fn test_create_stack_frame_invalid_no_plus() {
        let line = "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore0x45a3";
        assert!(create_stack_frame(line).is_none());
    }

    #[test]
    fn test_create_stack_frame_invalid_hex() {
        let line = "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore+0xGGGG";
        assert!(create_stack_frame(line).is_none());
    }

    #[test]
    fn test_create_stack_frame_empty() {
        assert!(create_stack_frame("").is_none());
    }

    #[test]
    fn test_create_stack_frames_batch() {
        let lines = vec![
            "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore+0x45a3".to_string(),
            "01 000000cd7bbfe900 00007ff6ddd12345 TestMod+0x1000".to_string(),
            "02 000000cd7bbfea00 00007ff6ddd67890 AnotherMod+0x2000".to_string(),
        ];

        let frames = create_stack_frames(lines);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].frame_number, "00");
        assert_eq!(frames[1].frame_number, "01");
        assert_eq!(frames[2].frame_number, "02");
    }

    #[test]
    fn test_create_stack_frames_with_header() {
        let lines = vec![
            "# Child-SP          Return Address       Call Site".to_string(),
            "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore+0x45a3".to_string(),
            "01 000000cd7bbfe900 00007ff6ddd12345 TestMod+0x1000".to_string(),
        ];

        let frames = create_stack_frames(lines);
        // Header should be filtered out
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].frame_number, "00");
    }

    #[test]
    fn test_create_stack_frames_with_empty_lines() {
        let lines = vec![
            "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore+0x45a3".to_string(),
            "".to_string(),
            "01 000000cd7bbfe900 00007ff6ddd12345 TestMod+0x1000".to_string(),
            "   ".to_string(),
        ];

        let frames = create_stack_frames(lines);
        assert_eq!(frames.len(), 2);
    }

    #[test]
    fn test_create_stack_frames_filters_invalid() {
        let lines = vec![
            "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore+0x45a3".to_string(),
            "invalid line".to_string(),
            "01 000000cd7bbfe900 00007ff6ddd12345 TestMod+0x1000".to_string(),
        ];

        let frames = create_stack_frames(lines);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].frame_number, "00");
        assert_eq!(frames[1].frame_number, "01");
    }

    #[test]
    fn test_create_stack_frames_empty_input() {
        let lines: Vec<String> = vec![];
        let frames = create_stack_frames(lines);
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn test_stack_frame_debug() {
        let line = "00 000000cd7bbfe830 00007ff6ddd0b4ae DxeCore+0x45a3";
        let frame = create_stack_frame(line).unwrap();

        let debug_str = format!("{:?}", frame);
        assert!(debug_str.contains("StackFrame"));
    }
}
