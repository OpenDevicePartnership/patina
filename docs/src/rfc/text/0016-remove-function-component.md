# RFC: Remove FunctionComponent Implementation

A request for comments regarding the removal of the `FunctionComponent` implementation of the `Component` trait from
`patina_sdk` in favor of only providing the `StructComponent` implementation.

## Change Log

- 2025-09-15: Initial RFC created.
- 2026-09-24: Finalize RFC.

## Motivation

A blanket trait implementation of `IntoComponent` exists for any function that implements the required function
interface to be a component. This blanket trait implementation of `IntoComponent` is used to instantiate the
`FunctionComponent` struct, which implements `Component`. Ultimately this design allows a function with the
appropriate interface to be used as a dispatchable component for the core.

The `FunctionComponent` was one of two types of components created and provided by `patina_sdk` (The second being
`StructComponent` which does the same as above, but for instantiated objects that have an appropriate function
interface).

The `FunctionComponent` implementation has yet to be used as a component for any real-world component and the following
complaints have been noted by users

1. A zero-sized struct component can accomplish the same thing.
2. There is not immediate feedback when the function interface is not correct. The error message (which is cryptic)
  only appears when attempting to pass the function to `Core::with_component`. This differs from the struct component,
  which errors immediately in the derive macro for `IntoComponent` when the interface is wrong.
3. Confusing diagnostic error messages due to the function interface not matching.
4. Passing a function pointer as a component in the `Core::with_component` is unintuitive - e.g.
  `Core::with_component(my_func)`.

## Technology Background

No technical background knowledge as this is a non technical change in regards to platform driver changes. This change
merely changes the underlying component interface layer.

## Goals

1. Remove the `FunctionComponent` implementation and the associated blanket `IntoComponent` trait implementation for
  functions matching the required interface.
2. Improve the error diagnostic messages when the function used as the entry point does not properly implement the
  `ParamFunction` trait

## Requirements

1. Ensure any usage of function components have a clear transition to a struct component
2. Human readable error diagnostic messages via `diagnostic::on_unimplemented` when an invalid function is marked as
  the entry point for the `StructComponent`
3. Update or remove any existing function components
4. Update or remove documentation regarding function components

## Unresolved Questions

- N/A

## Prior Art

See [Alternatives](#alternatives).

## Alternatives

The alternative is to keep the `FunctionComponent` and corresponding `IntoComponent` implementation, however this will
lead to additional confusion in developers using Patina, with no corresponding benifit by using it, as the
`StructComponent` implementation can utilize a zero-sized struct to accomplish the same thing.

```text
┌──────────────┐          ┌───────────────┐           
│ User-defined │          │ User-defined  │           
│ struct       │          │ function      │           
└──────┬───────┘          └──────┬────────┘           
       │                         │                    
┌──────▼─────────────┐    ┌──────▼─────────────┐      
│ IntoComponent impl │    │ IntoComponent impl │      
│ via derive macro   │    │ via blanket impl   │      
└──────┬─────────────┘    └──────┬─────────────┘      
       │                         │                    
┌──────▼─────────────┐    ┌──────▼─────────────┐      
│ StructComponent    │    │ FunctionComponent  │      
│ (impl Component)   │    │ (impl Component)   │      
└──────────────────┬─┘    └─┬──────────────────┘      
                   │        │                         
                   │        │                         
             ┌─────▼────────▼─────┐                   
             │ Box<dyn Component> │                   
             └─────────┬──────────┘                   
                       │                              
                       │                              
         ┌─────────────▼────────────┐                 
         │ Core (Storage, Dispatch) │                 
         └──────────────────────────┘                
```

## Rust Code Design

1. Removal of the `patina_sdk::component::function_component` module
2. Removal of the `patina_samples::function_component` module
3. usage of `#[diagnostic::on_unimplemented(...)]` macro on the `ParamFunction` trait to provide better error message
  diagnostics when `#[entry_point(path = func)]` is used where `func` does not implement `ParamFunction`.

```text
    ┌──────────────┐        
    │ User-defined │        
    │ struct       │        
    └──────┬───────┘        
           │                
    ┌──────▼─────────────┐  
    │ IntoComponent impl │  
    │ via derive macro   │  
    └──────┬─────────────┘  
           │                
    ┌──────▼─────────────┐  
    │ StructComponent    │  
    │ (impl Component)   │  
    └─────────┬──────────┘  
              │             
    ┌─────────▼──────────┐  
    │ Box<dyn Component> │  
    └────────────────────┘  
              │             
              │             
┌─────────────▼────────────┐
│ Core (Storage, Dispatch) │
└──────────────────────────┘
```
