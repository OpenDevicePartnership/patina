//! Attribute macro for validating component parameter conflicts at compile time.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ImplItem, ItemFn, ItemImpl, Pat, Type, TypePath, parse2, spanned::Spanned};

/// Validates component impl blocks with a unified `#[component]` attribute.
///
/// This macro must be applied to impl blocks that define components. It does the following:
/// 1. Extract the type name from the impl block
/// 2. Verify an `entry_point` method exists
/// 3. Validate the entry_point parameters for conflicts
/// 4. Generate the IntoComponent trait implementation
///
/// ## Usage
///
/// Apply to the impl block:
/// ```rust, ignore
/// pub struct MyComponent;
///
/// #[component]
/// impl MyComponent {
///     fn entry_point(self, config: Config<u32>) -> Result<()> {
///         Ok(())
///     }
/// }
/// ```
///
/// ## Validation Rules
///
/// - Impl block must have an `entry_point` method
/// - Entry point must have `self`, `mut self`, `&self`, or `&mut self` as the first parameter
/// - No duplicate `ConfigMut<T>` parameters with the same type T
/// - Cannot have both `Config<T>` and `ConfigMut<T>` for the same type T
/// - Cannot use `&mut Storage` with `Config<T>` or `ConfigMut<T>`
/// - Cannot use `&Storage` with `ConfigMut<T>`
/// - Cannot have multiple `Commands` parameters or multiple service table parameters
pub(crate) fn component_entry_point(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Try to parse as an impl block
    if let Ok(impl_block) = parse2::<ItemImpl>(item.clone()) {
        return validate_component_impl_block(impl_block);
    }

    // If not an impl block, return error
    quote! {
        compile_error!("#[component] must be applied to an impl block");
        #item
    }
}

/// Validates a component impl block and generates the IntoComponent implementation.
fn validate_component_impl_block(impl_block: ItemImpl) -> TokenStream {
    // Extract the type name from the impl block
    let type_path = match &*impl_block.self_ty {
        Type::Path(type_path) => type_path,
        _ => {
            return quote! {
                compile_error!("#[component] can only be applied to impl blocks for named types");
                #impl_block
            };
        }
    };

    // Get the type identifier
    let type_ident = match type_path.path.segments.last() {
        Some(segment) => &segment.ident,
        None => {
            return quote! {
                compile_error!("Could not extract type name from impl block");
                #impl_block
            };
        }
    };

    // Find the entry_point method
    let entry_point_method = impl_block.items.iter().find_map(|item| {
        if let ImplItem::Fn(method) = item
            && method.sig.ident == "entry_point"
        {
            return Some(method);
        }
        None
    });

    let entry_point = match entry_point_method {
        Some(method) => method,
        None => {
            return quote! {
                compile_error!("#[component] impl block must contain an `entry_point` method");
                #impl_block
            };
        }
    };

    // Convert ImplItemFn to ItemFn for validation
    let item_fn = ItemFn {
        attrs: entry_point.attrs.clone(),
        vis: entry_point.vis.clone(),
        sig: entry_point.sig.clone(),
        block: Box::new(entry_point.block.clone()),
    };

    // Validate that entry_point has self parameter
    if let Err(error) = check_impl_method_has_self(&item_fn) {
        return quote! {
            #error
            #impl_block
        };
    }

    // Validate parameters for conflicts
    if let Err(error) = check_param_conflicts(&item_fn) {
        return quote! {
            #error
            #impl_block
        };
    }

    // Generate the IntoComponent implementation
    let generics = &impl_block.generics;
    let where_clause = &generics.where_clause;
    let self_ty = &impl_block.self_ty;
    let impl_items = &impl_block.items;
    let alloc_name = quote::format_ident!("__alloc_component_{}", type_ident);

    // Just extract the parameter identifiers (not bounds) for putting them into turbofish
    let turbofish = if !generics.params.is_empty() {
        let param_idents = generics.params.iter().map(|param| match param {
            syn::GenericParam::Type(type_param) => {
                let ident = &type_param.ident;
                quote!(#ident)
            }
            syn::GenericParam::Lifetime(lifetime_param) => {
                let lifetime = &lifetime_param.lifetime;
                quote!(#lifetime)
            }
            syn::GenericParam::Const(const_param) => {
                let ident = &const_param.ident;
                quote!(#ident)
            }
        });
        quote!(::<#(#param_idents),*>)
    } else {
        quote!()
    };

    let entry_point_fn = quote!(#type_ident #turbofish :: entry_point);

    // Manually reconstruct the impl block to avoid quote! issues
    let impl_block_output = quote! {
        impl #generics #self_ty #where_clause {
            #(#impl_items)*
        }
    };

    // Return the reconstructed impl block plus the generated IntoComponent impl
    quote! {
        #impl_block_output

        extern crate alloc as #alloc_name;

        impl #generics patina::component::params::ComponentInput for #self_ty #where_clause {}

        impl #generics patina::component::IntoComponent<(#self_ty,)> for #self_ty #where_clause {
            fn into_component(self) -> #alloc_name::boxed::Box<dyn patina::component::Component> {
                #alloc_name::boxed::Box::new(
                    patina::component::StructComponent::new(
                        #entry_point_fn,
                        self
                    )
                )
            }
        }
    }
}

/// Validates that an impl method has `self`, `mut self`, `&self`, or `&mut self` as the first parameter.
pub(crate) fn check_impl_method_has_self(func: &ItemFn) -> Result<(), TokenStream> {
    let first_param = func.sig.inputs.first();

    match first_param {
        Some(FnArg::Receiver(_)) => Ok(()),
        Some(FnArg::Typed(_)) => {
            let error_msg =
                "The impl entry point must use 'self', 'mut self', '&self' or '&mut self' as the first parameter.";
            Err(quote! {
                compile_error!(#error_msg);
                #func
            })
        }
        None => {
            let error_msg = "Impl method entry point must have a 'self' parameter.";
            Err(quote! {
                compile_error!(#error_msg);
                #func
            })
        }
    }
}

/// Validates that a component's entry_point function doesn't have conflicting parameters.
// Note: Marked as dead code since it's only used by tests.
#[allow(dead_code)]
pub(crate) fn validate_component_params2(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = match parse2::<ItemFn>(item.clone()) {
        Ok(func) => func,
        Err(e) => return e.to_compile_error(),
    };

    if let Err(error) = check_param_conflicts(&func) {
        return error;
    }

    quote! { #func }
}

/// Represents a parameter type we care about for conflict detection
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParamType {
    Config(String),          // Config<T> where String is T
    ConfigMut(String),       // ConfigMut<T> where String is T
    Storage,                 // &Storage
    StorageMut,              // &mut Storage
    Commands,                // Commands
    StandardBootServices,    // StandardBootServices (UEFI Boot Services)
    StandardRuntimeServices, // StandardRuntimeServices (UEFI Runtime Services)
    Other,                   // Any other parameter type
}

impl ParamType {
    /// Checks if this parameter type conflicts with another parameter type.
    ///
    /// Returns `Some(error_message)` if there is a conflict, or `None` if there is no conflict.
    /// The error message describes the nature of the conflict.
    fn conflicts_with(&self, other: &ParamType) -> Option<&'static str> {
        match (self, other) {
            // Duplicate ConfigMut<T> with the same inner type
            (ParamType::ConfigMut(t1), ParamType::ConfigMut(t2)) if t1 == t2 => {
                Some("Each ConfigMut<T> type can only appear once in a component's entry point.")
            }

            // Config<T> conflicts with ConfigMut<T> for the same type
            (ParamType::Config(t1), ParamType::ConfigMut(t2)) | (ParamType::ConfigMut(t1), ParamType::Config(t2))
                if t1 == t2 =>
            {
                Some("You cannot have both Config<T> and ConfigMut<T> for the same type.")
            }

            // &mut Storage conflicts with Config<T> or ConfigMut<T>
            (ParamType::StorageMut, ParamType::Config(_))
            | (ParamType::Config(_), ParamType::StorageMut)
            | (ParamType::StorageMut, ParamType::ConfigMut(_))
            | (ParamType::ConfigMut(_), ParamType::StorageMut) => {
                Some("You cannot use &mut Storage together with Config<T> or ConfigMut<T> parameters.")
            }

            // &Storage conflicts with ConfigMut<T>
            (ParamType::Storage, ParamType::ConfigMut(_)) | (ParamType::ConfigMut(_), ParamType::Storage) => {
                Some("You cannot use &Storage together with ConfigMut<T> parameters.")
            }

            // Duplicate Commands
            (ParamType::Commands, ParamType::Commands) => Some("Only one Commands parameter is allowed."),

            // Duplicate StandardBootServices
            (ParamType::StandardBootServices, ParamType::StandardBootServices) => {
                Some("Only one StandardBootServices parameter is allowed.")
            }

            // Duplicate StandardRuntimeServices
            (ParamType::StandardRuntimeServices, ParamType::StandardRuntimeServices) => {
                Some("Only one StandardRuntimeServices parameter is allowed.")
            }

            // No conflict
            _ => None,
        }
    }
}

/// Normalize a type path to its canonical string representation.
///
/// Converts type paths to a consistent format that allows comparing
/// qualified and unqualified paths. For example:
/// - `Config` -> "Config"
/// - `patina::component::Config` -> "patina::component::Config"
/// - `crate::Config` -> "crate::Config"
fn normalize_type_path(path: &syn::Path) -> String {
    let segments: Vec<String> = path.segments.iter().map(|seg| seg.ident.to_string()).collect();
    segments.join("::")
}

/// Extract the inner type from a generic type like Config<T> or ConfigMut<T>
/// and return its normalized canonical representation.
fn extract_generic_inner(path: &TypePath) -> Option<String> {
    if let Some(segment) = path.path.segments.last()
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(ty)) = args.args.first()
    {
        // Normalize the inner type for consistent comparison
        return Some(normalize_type(ty));
    }
    None
}

/// Normalize a type to its canonical string representation.
///
/// This handles various type forms and converts them to a consistent format:
/// - Path types: normalized path representation
/// - Generic types: includes normalized generic arguments
/// - Reference types: includes mutability
fn normalize_type(ty: &Type) -> String {
    match ty {
        Type::Path(type_path) => {
            let base_path = normalize_type_path(&type_path.path);

            // Handle generic arguments
            if let Some(segment) = type_path.path.segments.last()
                && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
            {
                let inner_types: Vec<String> = args
                    .args
                    .iter()
                    .map(|arg| match arg {
                        syn::GenericArgument::Type(inner_ty) => normalize_type(inner_ty),
                        other => quote!(#other).to_string(),
                    })
                    .collect();

                if !inner_types.is_empty() {
                    return format!("{}<{}>", base_path, inner_types.join(", "));
                }
            }

            base_path
        }
        Type::Reference(type_ref) => {
            let inner = normalize_type(&type_ref.elem);
            if type_ref.mutability.is_some() { format!("&mut {}", inner) } else { format!("&{}", inner) }
        }
        _ => quote!(#ty).to_string(),
    }
}

/// Get the base type name from a type path (the last segment without qualifiers).
///
/// Examples:
/// - `Config` -> "Config"
/// - `patina::component::Config` -> "Config"
/// - `crate::something::ConfigMut` -> "ConfigMut"
fn get_base_type_name(path: &syn::Path) -> Option<String> {
    path.segments.last().map(|seg| seg.ident.to_string())
}

/// Classify a parameter type
fn classify_param(ty: &Type) -> ParamType {
    match ty {
        Type::Path(type_path) => {
            // Get the base type name (last segment) for matching
            let base_name = match get_base_type_name(&type_path.path) {
                Some(name) => name,
                None => return ParamType::Other,
            };

            // Check for Config<T>
            if base_name == "Config"
                && let Some(inner) = extract_generic_inner(type_path)
            {
                return ParamType::Config(inner);
            }

            // Check for ConfigMut<T>
            if base_name == "ConfigMut"
                && let Some(inner) = extract_generic_inner(type_path)
            {
                return ParamType::ConfigMut(inner);
            }

            // Check for Commands
            if base_name == "Commands" {
                return ParamType::Commands;
            }

            // Check for StandardBootServices
            if base_name == "StandardBootServices" {
                return ParamType::StandardBootServices;
            }

            // Check for StandardRuntimeServices
            if base_name == "StandardRuntimeServices" {
                return ParamType::StandardRuntimeServices;
            }

            ParamType::Other
        }
        Type::Reference(type_ref) => {
            if let Type::Path(type_path) = &*type_ref.elem {
                let base_name = match get_base_type_name(&type_path.path) {
                    Some(name) => name,
                    None => return ParamType::Other,
                };

                // Check for &Storage or &mut Storage
                if base_name == "Storage" {
                    if type_ref.mutability.is_some() {
                        return ParamType::StorageMut;
                    } else {
                        return ParamType::Storage;
                    }
                }
            }
            ParamType::Other
        }
        _ => ParamType::Other,
    }
}

/// Check for parameter conflicts and return compile error if found
/// Checks for parameter conflicts in the function signature.
pub(crate) fn check_param_conflicts(func: &ItemFn) -> Result<(), TokenStream> {
    let mut params: Vec<(usize, ParamType, String, proc_macro2::Span)> = Vec::new();

    // Collect all parameters (skip 'self')
    for (idx, arg) in func.sig.inputs.iter().enumerate() {
        if let FnArg::Typed(pat_type) = arg {
            let param_type = classify_param(&pat_type.ty);
            let param_name = match &*pat_type.pat {
                Pat::Ident(ident) => ident.ident.to_string(),
                _ => format!("param_{}", idx),
            };
            // Get the span of the entire parameter (pattern + type)
            let param_span = pat_type.span();
            params.push((idx, param_type, param_name, param_span));
        }
    }

    // Check for conflicts
    for i in 0..params.len() {
        for j in (i + 1)..params.len() {
            let (idx1, type1, name1, span1) = &params[i];
            let (idx2, type2, name2, span2) = &params[j];

            if let Some(conflict_msg) = type1.conflicts_with(type2) {
                // Build a detailed error message with parameter information
                // For ConfigMut conflicts, include the concrete type in the message
                let detailed_conflict_msg = match (type1, type2) {
                    (ParamType::ConfigMut(t1), ParamType::ConfigMut(_)) => {
                        format!("Each ConfigMut<{}> type can only appear once in a component's entry point.", t1)
                    }
                    (ParamType::Config(t1), ParamType::ConfigMut(_))
                    | (ParamType::ConfigMut(t1), ParamType::Config(_)) => {
                        format!("You cannot have both Config<{}> and ConfigMut<{}> for the same type.", t1, t1)
                    }
                    _ => conflict_msg.to_string(),
                };

                let error_msg = format!(
                    "Patina component parameter conflict detected: parameter '{}' (position {}) conflicts with parameter '{}' (position {}). {}",
                    name2, idx2, name1, idx1, detailed_conflict_msg
                );

                // Create the primary error at the second parameter location
                let mut error = syn::Error::new(*span2, error_msg);

                // Add a note pointing to the first conflicting parameter
                // Use "first..." for duplicate parameters, "conflicts with..." for incompatible types
                let note_msg = match (type1, type2) {
                    (ParamType::ConfigMut(_), ParamType::ConfigMut(_))
                    | (ParamType::Commands, ParamType::Commands)
                    | (ParamType::StandardBootServices, ParamType::StandardBootServices)
                    | (ParamType::StandardRuntimeServices, ParamType::StandardRuntimeServices) => {
                        format!("first '{}' parameter here", name1)
                    }
                    _ => format!("conflicts with '{}' parameter here", name1),
                };
                error.combine(syn::Error::new(*span1, note_msg));

                return Err(error.to_compile_error());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_allows_valid_params() {
        let input = quote! {
            fn entry_point(self, config: Config<u32>, other: Service<Foo>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        // Should not contain compile_error
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_duplicate_config_mut() {
        let input = quote! {
            fn entry_point(self, c1: ConfigMut<u32>, c2: ConfigMut<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
        assert!(result.to_string().contains("ConfigMut"));
    }

    #[test]
    fn test_detects_config_and_config_mut_conflict() {
        let input = quote! {
            fn entry_point(self, c1: Config<u32>, c2: ConfigMut<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
    }

    #[test]
    fn test_detects_storage_mut_and_config_conflict() {
        let input = quote! {
            fn entry_point(self, storage: &mut Storage, config: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("&mut Storage"));
    }

    #[test]
    fn test_component_entry_point_validates_name() {
        // Entry point must be named 'entry_point'
        let input = quote! {
            impl MyComponent {
                fn my_custom_function(self, config: Config<u32>) -> Result<()> {
                    Ok(())
                }
            }
        };

        let result = component_entry_point(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("entry_point"));
    }

    #[test]
    fn test_component_entry_point_validates_params() {
        let input = quote! {
            impl MyComponent {
                fn entry_point(self, c1: ConfigMut<u32>, c2: ConfigMut<u32>) -> Result<()> {
                    Ok(())
                }
            }
        };

        let result = component_entry_point(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
    }

    #[test]
    fn test_component_entry_point_no_params_error() {
        let input = quote! {
            impl MyComponent {
                fn entry_point(self) -> Result<()> {
                    Ok(())
                }
            }
        };

        let result = component_entry_point(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_duplicate_config_mut_different_positions() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                service: Service<Foo>,
                c1: ConfigMut<u32>,
                other: Service<Bar>,
                c2: ConfigMut<u32>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("conflicts"));
        assert!(result.to_string().contains("ConfigMut"));
    }

    #[test]
    fn test_allows_different_config_mut_types() {
        let input = quote! {
            fn entry_point(comp: MyComponent, c1: ConfigMut<u32>, c2: ConfigMut<String>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_duplicate_config_mut_complex_types() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                c1: ConfigMut<Vec<String>>,
                c2: ConfigMut<Vec<String>>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
    }

    #[test]
    fn test_detects_config_mut_and_config_conflict() {
        let input = quote! {
            fn entry_point(self, c1: ConfigMut<u32>, c2: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
        assert!(result.to_string().contains("Config<"));
        assert!(result.to_string().contains("ConfigMut<"));
    }

    #[test]
    fn test_allows_config_and_config_mut_different_types() {
        let input = quote! {
            fn entry_point(comp: MyComponent, c1: Config<u32>, c2: ConfigMut<String>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_allows_multiple_config_same_type() {
        let input = quote! {
            fn entry_point(comp: MyComponent, c1: Config<u32>, c2: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_storage_mut_and_config_mut_conflict() {
        let input = quote! {
            fn entry_point(self, storage: &mut Storage, config: ConfigMut<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("&mut Storage"));
    }

    #[test]
    fn test_detects_storage_and_config_mut_conflict() {
        let input = quote! {
            fn entry_point(self, storage: &Storage, config: ConfigMut<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("&Storage"));
        assert!(result.to_string().contains("ConfigMut"));
    }

    #[test]
    fn test_detects_config_mut_and_storage_conflict() {
        let input = quote! {
            fn entry_point(self, config: ConfigMut<u32>, storage: &Storage) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("ConfigMut"));
    }

    #[test]
    fn test_allows_storage_and_config() {
        let input = quote! {
            fn entry_point(comp: MyComponent, storage: &Storage, config: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_allows_storage_mut_without_configs() {
        let input = quote! {
            fn entry_point(comp: MyComponent, storage: &mut Storage, service: Service<Foo>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_duplicate_commands() {
        let input = quote! {
            fn entry_point(self, cmd1: Commands, cmd2: Commands) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
        assert!(result.to_string().contains("Commands"));
    }

    #[test]
    fn test_allows_single_commands() {
        let input = quote! {
            fn entry_point(comp: MyComponent, cmd: Commands) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_duplicate_commands_with_other_params() {
        let input = quote! {
            fn entry_point(
                self,
                config: Config<u32>,
                cmd1: Commands,
                service: Service<Foo>,
                cmd2: Commands
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Commands"));
    }

    #[test]
    fn test_check_impl_method_has_self_accepts_self() {
        let input = quote! {
            fn entry_point(self, config: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let func = parse2::<ItemFn>(input).unwrap();
        let result = check_impl_method_has_self(&func);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_impl_method_has_self_accepts_mut_self() {
        let input = quote! {
            fn entry_point(mut self, config: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let func = parse2::<ItemFn>(input).unwrap();
        let result = check_impl_method_has_self(&func);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_impl_method_has_self_accepts_ref_self() {
        let input = quote! {
            fn entry_point(&self, config: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let func = parse2::<ItemFn>(input).unwrap();
        let result = check_impl_method_has_self(&func);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_impl_method_has_self_accepts_ref_mut_self() {
        let input = quote! {
            fn entry_point(&mut self, config: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let func = parse2::<ItemFn>(input).unwrap();
        let result = check_impl_method_has_self(&func);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_impl_method_has_self_rejects_typed_param() {
        let input = quote! {
            fn entry_point(comp: MyComponent, config: Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let func = parse2::<ItemFn>(input).unwrap();
        let result = check_impl_method_has_self(&func);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("self"));
    }

    #[test]
    fn test_check_impl_method_has_self_rejects_no_params() {
        let input = quote! {
            fn entry_point() -> Result<()> {
                Ok(())
            }
        };

        let func = parse2::<ItemFn>(input).unwrap();
        let result = check_impl_method_has_self(&func);
        assert!(result.is_err());
    }

    #[test]
    fn test_allows_many_valid_params() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                config1: Config<u32>,
                config2: Config<String>,
                config3: Config<Vec<u8>>,
                service1: Service<Foo>,
                service2: Service<Bar>,
                hob: Hob<MyHob>,
                storage: &Storage,
                commands: Commands
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_multiple_conflicts() {
        let input = quote! {
            fn entry_point(
                self,
                c1: ConfigMut<u32>,
                c2: ConfigMut<u32>,
                storage: &mut Storage,
                config: Config<String>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        // Should detect at least one of the conflicts
    }

    #[test]
    fn test_allows_config_mut_without_conflicts() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                config_mut: ConfigMut<u32>,
                service: Service<Foo>,
                hob: Hob<MyHob>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_allows_empty_params_after_component() {
        let input = quote! {
            fn entry_point(comp: MyComponent) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_allows_only_self() {
        let input = quote! {
            fn entry_point(self) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_classifies_qualified_config() {
        let input = quote! {
            fn entry_point(comp: MyComponent, c: patina::component::Config<u32>) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_conflict_with_qualified_types() {
        // This test verifies that qualified type paths are properly normalized and
        // detected as conflicts. For example, Config<u32> should conflict with
        // patina::component::ConfigMut<u32> because they both operate on the same
        // inner type u32.
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                c1: Config<u32>,
                c2: patina::component::ConfigMut<u32>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("conflict"));
    }

    #[test]
    fn test_allows_option_wrapped_params() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                config: Option<Config<u32>>,
                service: Option<Service<Foo>>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        // Option-wrapped types should be treated as Other and not conflict
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_component_entry_point_detects_duplicate_config_mut() {
        let input = quote! {
            impl MyComponent {
                fn entry_point(
                    self,
                    c1: ConfigMut<u32>,
                    c2: ConfigMut<u32>
                ) -> Result<()> {
                    Ok(())
                }
            }
        };

        let result = component_entry_point(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
    }

    #[test]
    fn test_component_entry_point_with_multiple_params() {
        let input = quote! {
            impl MyComponent {
                fn entry_point(
                    self,
                    config: Config<u32>,
                    service: Service<Foo>,
                    hob: Hob<MyHob>
                ) -> Result<()> {
                    Ok(())
                }
            }
        };

        let result = component_entry_point(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_qualified_config_mut_duplicate_detection() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                c1: ConfigMut<u32>,
                c2: patina::component::ConfigMut<u32>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
    }

    #[test]
    fn test_fully_qualified_both_sides() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                c1: patina::component::Config<String>,
                c2: patina::component::ConfigMut<String>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("conflict"));
    }

    #[test]
    fn test_crate_qualified_paths() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                c1: crate::Config<u64>,
                c2: crate::ConfigMut<u64>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("conflict"));
    }

    #[test]
    fn test_mixed_qualified_storage_conflicts() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                storage: &mut patina::Storage,
                config: Config<u32>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Storage"));
    }

    #[test]
    fn test_normalized_complex_inner_types() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                c1: Config<Vec<String>>,
                c2: patina::component::ConfigMut<Vec<String>>
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("conflict"));
    }

    #[test]
    fn test_detects_duplicate_standard_boot_services() {
        let input = quote! {
            fn entry_point(self, bs1: StandardBootServices, bs2: StandardBootServices) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
        assert!(result.to_string().contains("StandardBootServices"));
    }

    #[test]
    fn test_allows_single_standard_boot_services() {
        let input = quote! {
            fn entry_point(comp: MyComponent, bs: StandardBootServices) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_duplicate_standard_runtime_services() {
        let input = quote! {
            fn entry_point(self, rs1: StandardRuntimeServices, rs2: StandardRuntimeServices) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("Patina component parameter conflict detected"));
        assert!(result.to_string().contains("StandardRuntimeServices"));
    }

    #[test]
    fn test_allows_single_standard_runtime_services() {
        let input = quote! {
            fn entry_point(comp: MyComponent, rs: StandardRuntimeServices) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_allows_both_boot_and_runtime_services() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                bs: StandardBootServices,
                rs: StandardRuntimeServices
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }

    #[test]
    fn test_detects_qualified_standard_boot_services() {
        let input = quote! {
            fn entry_point(
                self,
                bs1: StandardBootServices,
                bs2: patina::boot_services::StandardBootServices
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("StandardBootServices"));
    }

    #[test]
    fn test_detects_qualified_standard_runtime_services() {
        let input = quote! {
            fn entry_point(
                self,
                rs1: StandardRuntimeServices,
                rs2: patina::runtime_services::StandardRuntimeServices
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(result.to_string().contains("compile_error"));
        assert!(result.to_string().contains("StandardRuntimeServices"));
    }

    #[test]
    fn test_allows_services_with_other_params() {
        let input = quote! {
            fn entry_point(
                comp: MyComponent,
                bs: StandardBootServices,
                rs: StandardRuntimeServices,
                config: Config<u32>,
                service: Service<Foo>,
                commands: Commands
            ) -> Result<()> {
                Ok(())
            }
        };

        let result = validate_component_params2(quote!(), input);
        assert!(!result.to_string().contains("compile_error"));
    }
}
