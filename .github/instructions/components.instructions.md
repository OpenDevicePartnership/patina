---
name: 'Patina Component Conventions'
description: 'Coding conventions and patterns for Patina component crates'
applyTo: 'components/**'
---
# Patina Component Conventions

These conventions apply to all crates under `components/`. For project-wide rules,
see [copilot-instructions.md](../copilot-instructions.md).

## Component Architecture

Components use dependency injection through their entry point function signature.
The entry point declares what the component needs and the dispatcher provides it.

- Apply the `#[component]` attribute macro on an `impl` block for the component struct.
- The impl block must contain a method named `entry_point` that takes `self` and
  returns `patina::error::Result<()>`.
- Parameters in the entry point define dependencies - the component will not execute
  until all dependencies are satisfied.

```rust
use patina::{
    component::{component, params::Config},
    error::Result,
};

struct MyComponent;

#[component]
impl MyComponent {
    fn entry_point(self, config: Config<MyConfig>) -> Result<()> {
        // Component logic using injected dependencies
        Ok(())
    }
}
```

Services are registered via `#[derive(IntoService)]` with a `#[service(dyn Trait)]`
attribute:

```rust
use patina::component::service::IntoService;

#[derive(IntoService)]
#[service(dyn MyTrait)]
pub struct MyServiceImpl {
    // ...
}
```

## Stored Dependencies Pattern

Components must store all dependency references as fields in the component struct
rather than passing them through call chains. This avoids memory copies and enables
safe reuse in UEFI's memory-constrained environment.

- **Initialization phase:** Store all dependency references in struct fields.
- **Execution phase:** Use stored references via methods - no additional parameter
  resolution needed.
- Use `Service<T>` for service references (these are cheap `Clone` handles that share ownership of the
  implementation, and are `'static` so they can be stored freely).
- Use `Option<T>` for optional dependencies to handle unavailable services gracefully.

## Parameter Semantics

See [component interface](../../docs/src/component/interface.md) for the full parameter
reference table, availability rules, and detailed descriptions of each parameter type.

## Crate Layout

Follow the standardized layout from [component requirements](../../docs/src/component/requirements.md):

1. No public definitions in `lib.rs` - only public module declarations.
2. **Required:** `component` module containing the crate's component(s).
3. **Optional modules** (as needed):
   - `config` - Configuration types registered via `.with_config`
   - `error` - Domain-specific error types for services
   - `hob` - GUID HOB type definitions
   - `service` - Service trait definitions and implementations

## Component Dependencies

Components must only depend on crates in `sdk/` and generic external crates. Never
depend on other component crates or `core/` crates. This keeps components loosely
coupled and independently maintainable.

## Testing Components

- Test names must be prefixed with `test_<component_name>_` (snake_case).
- Use `Config::mock(value)`, `Service::mock(impl)`, `Hob::mock(data)`, and
  `Commands::mock()` for unit testing component entry points.
- Mock service traits with `#[automock]` from `mockall`. Use extension traits for
  any utility methods that call base trait methods.

See [component interface](../../docs/src/component/interface.md) for the full
parameter specification and examples.
