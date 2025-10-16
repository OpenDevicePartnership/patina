# Patina MM Integration Test Framework

## Overview

The Patina MM Integration Test Framework provides testing capabilities for Management Mode (MM)
communication. The framework resides in a single integration test crate with to test both lightweight
unit-style communication patterns and full integration tests that exercise real `patina_mm` components
with their actual service interfaces.

The test suite validates:

- The `MmCommunicator` component initialization and dependency injection
- `MmCommunication` service interface testing with actual communication buffers
- MM communicate message parsing and serialization
- Error handling and validation across the MM communication stack
- Stress testing of the MM communicate service

## Running Tests

Logging through the [`env_logger`](https://docs.rs/env_logger/latest/env_logger/) crate is provided for detailed
visibility into the MM communication flow during test execution. See the log targets mentioned in individual module
doc comments to discover how to enable this testing.

### Quick Start

**From the repository root:**

```powershell
# Run all Patina MM tests with debug logging
$env:RUST_LOG="debug"; cargo make test --package patina_mm

# Run the main integration test suite
$env:RUST_LOG="debug"; cargo make test --package patina_mm --test patina_mm_integration

# Run specific test by name within the integration suite
$env:RUST_LOG="debug"; cargo make test --package patina_mm --test patina_mm_integration test_mm_communicator_component_initialization

# Run specific test module (e.g., stress tests)
$env:RUST_LOG="debug"; cargo make test --package patina_mm --test patina_mm_integration mm_communicator::stress_tests
```

> Note: These are Powershell examples.

**From the component directory:**

```powershell
# Navigate to the MM component directory
cd components\patina_mm

# Run all tests with debug logging
$env:RUST_LOG="debug"; cargo make test
```

> Note: These are Powershell examples.

### Log Level Configuration

The following log levels are available:

| Log Level | Description | Environment Variable |
|-----------|-------------|---------------------|
| `error` | Only error messages | `RUST_LOG=error` |
| `warn` | Warnings and errors | `RUST_LOG=warn` |
| `info` | Informational messages, warnings, and errors | `RUST_LOG=info` |
| `debug` | Debug information (recommended for development) | `RUST_LOG=debug` |
| `trace` | Most verbose logging | `RUST_LOG=trace` |

### Advanced Logging Configuration

**Module-specific logging:**

You can target specific modules:

PowerShell:

```powershell
# Only log MM communication at debug level
$env:RUST_LOG="patina_mm=debug"; cargo make test --package patina_mm

# Multiple module filters
$env:RUST_LOG="test_mm_executor=debug,echo_handler=trace"; cargo make test --package patina_mm
```

**Sample Debug Output:**

When running with `RUST_LOG=debug`, you'll see detailed logs like this:

```text
[2025-10-01T13:37:41Z DEBUG real_test_framework] Building real component MM test framework with 1 handlers
[2025-10-01T13:37:41Z DEBUG patina_mm::config] Creating new CommunicateBuffer: id=0, size=0x1000
[2025-10-01T13:37:41Z INFO  patina_mm::component::communicator] MM communication request: data_size=26
[2025-10-01T13:37:41Z DEBUG echo_handler] Echoing 26 bytes of data
[2025-10-01T13:37:41Z DEBUG test_mm_executor] Handler executed successfully, response_len=26
```

### Available Test Suites

The framework organizes tests as a single integration test crate with modules separated logically.

| Test Module | Description | Key Test Files |
|-----------|-------------|-----------|
| `mm_communicator` | Real MM component integration tests | `component_integration_tests.rs`, `stress_tests.rs` |
| `mm_supervisor` | MM supervisor protocol testing | `communication_tests.rs` |
| `framework` | Core functionality tests with lightweight framework | `core_functionality_tests.rs` |
| `common` | Shared utilities and test infrastructure | `framework.rs`, `real_component_framework.rs`, `handlers.rs` |

### Troubleshooting

**If: No log output is visible:**

- Ensure you're using the `RUST_LOG` environment variable
- Verify the `RUST_LOG` environment variable is set correctly
- Confirm that `env_logger` is properly initialized in the test
- Check the log targets used in the code

**If: Tests are failing:**

- Run with `RUST_LOG=debug` to see detailed execution flow
- Review the test logs to verify data integrity
- Ensure all required services and dependencies are properly configured

## High-Level Design

### Organization and Overview

Two complementary testing approaches are available:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                    MM Integration Test Crate Structure                      │
├─────────────────────────────────────────────────────────────────────────────┤
│  main.rs (test crate entry point)                                           │
│  ├── mod common (shared utilities)                                          │
│  ├── mod mm_communicator (real component tests)                             │
│  ├── mod mm_supervisor (supervisor simulated tests)                         │
│  └── mod framework (core functionality tests)                               │
├─────────────────────────────────────────────────────────────────────────────┤
│  1. Unit Test Style (lightweight and tests the framework module)            │
│     ┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐      │
│     │   Test Cases    │───>│ MmTestFramework  │───>│ Handler Registry│      │
│     │                 │    │    (Builder)     │    │  (HashMap)      │      │
│     └─────────────────┘    └──────────────────┘    └─────────────────┘      │
│                                     │                        │              │
│                                     ▼                        ▼              │
│                            ┌──────────────────┐    ┌─────────────────┐      │
│                            │ MmMessageParser  │───>│ Mock MmHandler  │      │
│                            │  (Safe Buffer)   │    │ Implementations │      │
│                            └──────────────────┘    └─────────────────┘      │
├─────────────────────────────────────────────────────────────────────────────┤
│  2. Real Component Framework (mm_communicator module)                       │
│     ┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐      │
│     │   Test Cases    │───>│ Component Tests  │───>│ Real Components │      │
│     │                 │    │   + Storage      │    │ MmCommunicator  │      │
│     └─────────────────┘    └──────────────────┘    └─────────────────┘      │
│                                     │                        │              │
│                                     ▼                        ▼              │
│                            ┌──────────────────┐    ┌─────────────────┐      │
│                            │ TestMmExecutor   │───>│ MmCommunication │      │
│                            │                  │    │    Service      │      │
│                            └──────────────────┘    └─────────────────┘      │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Real MM Communication Service Testing

The framework provides testing of the real `MmCommunication` service through the `mm_communicator` test
module. This approach validates the complete service implementation:

#### Component Initialization Process

```text
┌─────────────────────────────────────────────────────────────────┐
│ Real Component Setup (Integration Tests)                        │
│                                                                 │
│ 1. Storage::new()                                               │
│    └── Create component storage container                       │
│                                                                 │
│ 2. storage.add_config(MmCommunicationConfiguration)             │
│    └── Configure communication buffers and settings             │
│                                                                 │
│ 3. storage.add_service(SwMmiManager::new())                     │
│    └── Provide required SW MMI trigger service                  │
│                                                                 │
│ 4. MmCommunicator::new().into_component()                       │
│    └── Create component                                         │
│                                                                 │
│ 5. component.initialize(&mut storage)                           │
│    └── Resolve dependencies via Patina DI system                │
│                                                                 │
│ 6. component.run(&mut storage)                                  │
│    └── Register MmCommunication service in storage              │
│                                                                 │
│ 7. storage.get_service::<dyn MmCommunication>()                 │
│    └── Retrieve the registered service for testing              │
└─────────────────────────────────────────────────────────────────┘
```

#### Real Service Communication Flow

```text
┌─────────────────────────────────────────────────────────────────┐
│ MmCommunication Service Testing                                 │
│                                                                 │
│ 1. service.communicate(buffer_id, data, recipient_guid)         │
│    ├── Validate buffer_id against available buffers             │
│    ├── Validate data size against buffer capacity               │
│    └── Check recipient_guid format                              │
│                                                                 │
│ 2. CommunicateBuffer Operations                                 │
│    ├── comm_buffer.reset() - Clear existing data                │
│    ├── comm_buffer.set_message_info(&guid) - Set recipient      │
│    └── comm_buffer.set_message(data) - Write request data       │
│                                                                 │
│ 3. MM Execution (via MmExecutor trait)                          │
│    ├── executor.execute_mm(&mut comm_buffer)                    │
│    ├── Real implementation: SW MMI trigger                      │
│    └── Test implementation: TestMmExecutor with handlers        │
│                                                                 │
│ 4. Response Processing                                          │
│    ├── comm_buffer.get_message() - Extract response data        │
│    ├── Validate response size and format                        │
│    └── Return Vec<u8> response to caller                        │
└─────────────────────────────────────────────────────────────────┘
```

### Communication Flow

The framework supports two distinct communication testing approaches:

#### 1. Unit Test Style Communication

This approach tests basic communication patterns within `MmTestFramework`:

```text
Stage 1: Framework Setup
┌─────────────────────────────────────────────────────────────────┐
│ MmTestFramework::builder()                                      │
│   .with_echo_handler()                                          │
│   .with_mm_supervisor_handler()                                 │
│   .build()                                                      │
└─────────────────────────────────────────────────────────────────┘

Stage 2: Direct Communication
┌─────────────────────────────────────────────────────────────────┐
│ framework.communicate(&guid, data)                              │
│   ├── MmMessageParser::new(&mut buffer)                         │
│   ├── parser.write_message(&guid, data)                         │
│   ├── handler.handle_request(data)                              │
│   └── Returns Vec<u8> response                                  │
└─────────────────────────────────────────────────────────────────┘
```

#### 2. Real Component Communication (Integration Testing)

This approach tests the actual `MmCommunication` service interface:

```text
Stage 1: Component Integration Setup
┌─────────────────────────────────────────────────────────────────┐
│ Storage::new() + MmCommunicationConfiguration                   │
│   ├── Creates real CommunicateBuffer instances                  │
│   ├── Configures buffer IDs and sizes                           │
│   └── Adds SwMmiManager service dependency                      │
│                                                                 │
│ MmCommunicator component initialization                         │
│   ├── Resolves dependencies via Patina DI                       │
│   ├── Registers the MmCommunication service                     │
│   └── Makes service available via Storage                       │
└─────────────────────────────────────────────────────────────────┘

Stage 2: Real Service Interface Testing
┌─────────────────────────────────────────────────────────────────┐
│ service.communicate(buffer_id, data, &recipient_guid)           │
│   ├── Real MmCommunicator.communicate() method                  │
│   ├── Real CommunicateBuffer buffer selection & validation      │
│   ├── Real buffer.set_message_info() and set_message()          │
│   ├── TestMmExecutor.execute_mm() (or real SW MMI trigger)      │
│   ├── Real buffer.get_message() for response extraction         │
│   └── Real service error handling and status codes              │
└─────────────────────────────────────────────────────────────────┘
```

### Core Components

#### 1. `MmTestFramework` (Unit Test Style)

Lightweight testing framework for basic MM communication patterns.

- Maintains a configurable handler registry
- Simplifies MM handler invocation without a full MM stack

#### 2. Real MM Communication Service (Integration Testing)

The `mm_communicator` test module validates the actual `MmCommunication` service interface through component
integration tests.

- Uses `MmCommunicator` and `SwMmiManager`
- Mocks MM execution using`TestMmExecutor` to swap it out
- Tests the complete MM Communication flow
- Injects Patina component dependencies

#### 3. `MmMessageParser`

Provides message parsing and buffer operations.

- Message writing with bounds checking
- Buffer validation and error handling
- MM Communication header management

#### 4. `TestMmExecutor`

Simulates MM execution for real component testing.

- Routes MM requests to appropriate test handlers
- Maintains realistic `CommunicateBuffer` semantics
- Provides logging
- Integrates with real component error handling

#### 5. Common Test Infrastructure

The framework provides shared infrastructure components organized as modules within the single test crate:

- **`common/framework.rs`**: Core `MmTestFramework` and unit style tests for the framework
- **`common/real_component_framework.rs`**: `TestMmExecutor` for integration testing with real components
- **`common/handlers.rs`**: Standard MM handler implementations (`EchoHandler`, `MmSupervisorHandler`, `VersionInfoHandler`)
- **`common/message_parser.rs`**: Message parsing utilities
- **`common/constants.rs`**: Test constants, GUIDs, and configuration values
- **`common/mod.rs`**: Module exports and shared imports for the test crate

## Communication Protocol

### Buffer Layout

Each communication buffer follows the standard UEFI MM Communication format:

```text
┌─────────────────┬─────────────────┬─────────────────┐
│   Header GUID   │ Message Length  │  Message Data   │
│    (16 bytes)   │   (8 bytes)     │   (Variable)    │
└─────────────────┴─────────────────┴─────────────────┘
│◄────────── EfiMmCommunicateHeader::size() ─────────►│
```

### Request/Response Cycle

The communication patterns differ between the two framework approaches:

#### Unit Test Style Framework Flow

1. **Framework Setup**: Builder creates framework with registered handlers
2. **Communication Call**: `framework.communicate(guid, data)` invoked
3. **Message Parsing**: `MmMessageParser` writes message to the communicate buffer
4. **Handler Lookup**: Framework finds handler by GUID in registry
5. **Handler Execution**: Handler processes request data
6. **Response Return**: Handler result returned directly as `Vec<u8>`

#### Real Component Service Flow

1. **Service Retrieval**: Gets the `MmCommunication` service from component `Storage`
2. **Service Call**: Invokes `service.communicate(buffer_id, data, recipient_guid)`
3. **Buffer Validation**: Real buffer ID validation and capacity checking
4. **Buffer Operations**: Real `CommunicateBuffer.set_message_info()` and `set_message()` calls
5. **MM Execution**: `MmExecutor.execute_mm()` (TestMmExecutor in tests)
   - Note: This is where the real SW MMI service would be used in production
6. **Response Extraction**: Real `CommunicateBuffer.get_message()` to extract response
7. **Status Handling**: Real error propagation via `Status` enum
8. **Response Return**: Complete response data as `Vec<u8>` or error status

## Handler Implementation

### `MmHandler` Trait

All MM handlers implement this interface:

```rust
pub trait MmHandler: Send + Sync {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>>;
    fn description(&self) -> &str;
}
```

### Built-in Handler Examples

The framework includes several handler examples:

#### `EchoHandler`

A handler that returns input data unchanged:

```rust
impl MmHandler for EchoHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        log::debug!(target: "echo_handler", "Echoing {} bytes of data", data.len());
        Ok(data.to_vec())
    }
}
```

#### `VersionInfoHandler`

Returns static version information. Meant to roughly simulate getting version information from MM.

```rust
impl MmHandler for VersionInfoHandler {
    fn handle_request(&self, _data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        Ok(self.version_string.as_bytes().to_vec())
    }
}
```

#### `MmSupervisorHandler`

Handles MM Supervisor protocol requests:

```rust
impl MmHandler for MmSupervisorHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        let request = MmSupervisorRequestHeader::from_bytes(data)?;
        match request.request {
            mm_supervisor_requests::VERSION_INFO => self.handle_version_request(&request),
            mm_supervisor_requests::CAPABILITIES => self.handle_capabilities_request(&request),
            _ => Err(MmHandlerError::InvalidInput("Unknown request type".to_string())),
        }
    }
}
```

## Test Categories

### 1. Real Component Integration Tests (`mm_communicator` module)

Test the actual `MmCommunication` service implementation through complete component integration.

**Test Files:**

- `component_integration_tests.rs`
- `stress_tests.rs`

**Key Test Cases:**

- `test_mm_communicator_component_initialization()`: Complete component setup with dependencies
- `test_mm_communicator_with_empty_config()`: Service behavior with minimal configuration
- `test_mm_communicator_without_sw_mmi_service()`: SW MMI expected dependency testing
- `test_mm_communicator_dependency_injection()`: Full expected dependency validation
- `test_mm_communication_stress_thousand_calls()`: Stress testing

### 2. Core Framework Tests (`framework` module)

Test core MM communication functionality using `MmTestFramework`.

**Test Files:**

- `core_functionality_tests.rs`: Basic framework testing with mock handlers

**Key Test Cases:**

- `test_basic_communication()`: Basic echo communication
- `test_communication_with_different_data_sizes()`: Variable message size handling
- `test_communication_too_large_for_buffer()`: Buffer overflow check
- `test_communication_unknown_handler()`: Error handling for unregistered handlers
- `test_communication_simple_error_conditions()`: Simple error scenarios
- `test_multiple_sequential_communications()`: Sequential communication patterns
- `test_buffer_state_consistency()`: Buffer state management validation (internal state vs actual contents)

### 3. MM Supervisor Integration Tests (`mm_supervisor` module)

Test MM Supervisor communication patterns.

**Test Files:**

- `communication_tests.rs`: MM Supervisor interaction testing

**MM Supervisor Request Header:**

```rust
#[repr(C, packed(1))]
struct MmSupervisorRequestHeader {
    signature: u32,     // Protocol signature (0x5055534D)
    revision: u32,      // Protocol revision
    request: u32,       // Request type (e.g. VERSION_INFO, CAPABILITIES)
    reserved: u32,      // Reserved field
    result: u64,        // Result status
}
```

**Test Cases:**

- `test_mm_supervisor_version_request()`: Version information retrieval
- `test_mm_supervisor_capabilities_request()`: Capability enumeration
- `test_mm_supervisor_invalid_request()`: Error handling for invalid requests
- `test_mm_supervisor_invalid_signature()`: Protocol validation
- `test_mm_supervisor_small_request()`: Buffer size validation

## Error Handling

### Error Types

The framework defines custom error codes in:

- Framework-level errors: `TestFrameworkError`
- MM handler errors: `MmHandlerError`

## Framework Usage

### Unit Test Style Framework Usage

A test author can "build" the scenario needed. For example, this registers the "echo" and "MM supervisor version"
handlers.

```rust
// Create framework with built-in handlers
let framework = MmTestFramework::builder()
    .with_echo_handler()
    .with_mm_supervisor_handler()
    .build()
    .expect("Framework creation should succeed");

// Communicate with handlers
let response = framework.communicate(&test_guids::ECHO_HANDLER, b"test data")?;
assert_eq!(response, b"test data");
```

### Real Component Service Usage

The real component framework integrates actual `patina_mm` components through dependency injection simulation. For
example, like this which uses the SW MMI and MM Communication services:

```rust
// Set up component dependencies
let mut storage = Storage::new();

// Configure MM communication buffers
let config = MmCommunicationConfiguration {
    comm_buffers: vec![
        CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 0),
    ],
    ..Default::default()
};
storage.add_config(config);
storage.add_service(SwMmiManager::new());

// Initialize real component
let mut communicator = MmCommunicator::new().into_component();
communicator.initialize(&mut storage);
assert_eq!(communicator.run(&mut storage), Ok(true));

// Get the real service
let mm_service = storage.get_service::<dyn MmCommunication>().unwrap();

// Test real communication
let result = mm_service.communicate(buffer_id, request_data, recipient_guid);
```

### Custom Handler Registration

Both frameworks support custom handlers:

```rust
// Unit Test Style framework
let custom_handler = Box::new(MyCustomHandler::new());
let framework = MmTestFramework::builder()
    .with_handler(MY_CUSTOM_GUID, custom_handler)
    .build()?;

// Real component framework (via TestMmExecutor)
let executor = TestMmExecutor::new(handlers_map);
```

### Advanced Features

#### Safe Message Parsing

Both frameworks provide message parsing utilities:

```rust
let mut buffer = vec![0u8; TEST_BUFFER_SIZE];
let mut parser = MmMessageParser::new(&mut buffer);

// Write a message with validation like bounds checking
parser.write_message(&guid, data)?;

// Read a message with validation included
let (parsed_guid, parsed_data) = parser.parse_message()?;
```

## Configuration

### Test Constants

Some test constants can be modified to adjust testing details. See `components/patina_mm/tests/patina_mm_integration/common/constants.rs`.

For example:

```rust
pub const TEST_BUFFER_SIZE: usize = SIZE_4KB;           // 4KB per buffer
pub const MAX_TEST_MESSAGE_SIZE: usize =
    TEST_BUFFER_SIZE - EfiMmCommunicateHeader::size();  // Available message space
```

### Test GUIDs

The framework provides some "standard" test GUIDs referred to in testing.

For example:

```rust
pub mod test_guids {
    // Echo handler for basic testing
    pub const ECHO_HANDLER: efi::Guid =
        efi::Guid::from_fields(0x12345678, 0x1234, 0x5678, 0x12, 0x34,
                              &[0x56, 0x78, 0x90, 0xab, 0xcd, 0xef]);

    // Version handler for version testing
    pub const VERSION_HANDLER: efi::Guid =
        efi::Guid::from_fields(0x87654321, 0x4321, 0x8765, 0x43, 0x21,
                              &[0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54]);

    // MM Supervisor for protocol testing
    pub const MM_SUPERVISOR: efi::Guid =
        efi::Guid::from_fields(0x8c633b23, 0x1260, 0x4ea6, 0x83, 0x0F,
                              &[0x7d, 0xdc, 0x97, 0x38, 0x21, 0x11]);
}
```

## Custom Handler Implementation

### Basic Handler Template

Here's a template for implementing custom handlers:

```rust
use crate::common::*;

struct MyCustomHandler {
    // Handler state
}

impl MyCustomHandler {
    pub fn new() -> Self {
        Self { /* initialize */ }
    }
}

impl MmHandler for MyCustomHandler {
    fn handle_request(&self, data: &[u8]) -> MmHandlerResult<Vec<u8>> {
        // Validate input
        if data.is_empty() {
            return Err(MmHandlerError::InvalidInput("Empty data".to_string()));
        }

        // Process request
        let response = self.process_data(data)?;

        Ok(response)
    }

    fn description(&self) -> &str {
        "Custom handler for x"
    }
}
```

### Integration Test Example

```rust
#[test]
fn test_custom_handler_integration() {
    // Unit test style framework test
    let custom_handler = Box::new(MyCustomHandler::new());
    let framework = MmTestFramework::builder()
        .with_handler(MY_CUSTOM_GUID, custom_handler)
        .build()
        .expect("Framework should build successfully");

    let test_data = b"custom request data";
    let result = framework.communicate(&MY_CUSTOM_GUID, test_data);

    assert!(result.is_ok());
    let response = result.unwrap();
    assert!(response.starts_with(b"CUSTOM:"));
}

#[test]
fn test_custom_handler_real_component() {
    // Real component integration test
    let mut storage = Storage::new();

    // Configure real buffers and services
    let config = MmCommunicationConfiguration {
        comm_buffers: vec![CommunicateBuffer::new(Pin::new(Box::leak(Box::new([0u8; 1024]))), 0)],
        ..Default::default()
    };
    storage.add_config(config);
    storage.add_service(SwMmiManager::new());

    // Initialize component
    let mut communicator = MmCommunicator::new().into_component();
    communicator.initialize(&mut storage);
    assert_eq!(communicator.run(&mut storage), Ok(true));

    // Test real service
    let service = storage.get_service::<dyn MmCommunication>().unwrap();
    let result = service.communicate(0, b"custom request", MY_CUSTOM_GUID);

    // Validate real service behavior
    assert!(result.is_ok());
}
```
