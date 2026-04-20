# RFC: `patina_acpi` service interface improvements

This RFC proposes multiple interface changes to the two services (`Service<dyn AcpiProvider>` and
`Service<AcpiTableManager`>) to support ACPI Tables with unknown sizes (DSTs) and to generally improve user ergonomics
when using the service.

## Change Log

- 2026-04-16

## Motivation

The main motivation behind this change is to add support for DSTs (Dynamically Sized Types) in the generics portion of
the service interface (e.g. `Service<AcpiTableManager>::add_acpi_table<T>` and
`Service<AcpiTableManager>::get_acpi_table`). DST ACPI tables are currently supported via non-generic interfaces, e.g.
`Service<dyn AcpiProvider>::install_acpi_table`, but it puts all unsafety burdons on you, such as (1) allocating to the
correct memory type and (2) ensuring the ACPI table has the correct format. This interface simply accepts a pointer to
a memory address and treats it as a ACPI table. This support would be highly beneficial as most ACPI tables tend to
have variable length - e.g. a header with a runtime variable number of bytes behind it representing N number of
parseable structures.

Supporting DSTs will require some fundamental changes to how ACPI table data is managed (described in-depth below),
which means we have the opportunity to make some other fundamental changes to improve the code, such as using zerocopy
instead of raw pointer manipulation. This also allows for tightening the memory safety of the current design.

## Technology Background

- TODO

## Goals

1. Support DSTs through the ACPI service interface.
2. Reduce `unsafe` public interfaces via `AcpiTable` trait.
3. Reduce `unsafe` usage internal to the service via zerocopy.
4. Consolidate the two services provided by `patina_acpi`.
5. Provide updated and more usage documentation.

## Requirements

1. DSTs are supported through the ACPI service interface.
2. Documentation is updated and better usage documentation created.

## Unresolved Questions

## Alternatives

The alternative is to continue the status quo. As mentioned above, DSTs are currently supported via the non-generic
service interface, but it requires you to (1) Manually construct the `AcpiTable` struct (A wrapper around a raw
pointer), (2) Ensure you've allocated to the correct memory type yourself, and (3) Manually attempt casting a generic
table to a table of your choosing.

## Prior Art





## Rust Code Design

### AcpiTable Trait Definition

Below is the expected AcpiTable trait defintion. A derive macro will be provided for types that start with a
`AcpiTableHeader` structure, however it can also be manually implemented if a type does not start with an
`AcpiTableHeader`, but has the required fields as specified in the trait documentation. This trait also requires that
multiple zerocopy traits be implemented on it.

```rust
/// A trait implemented on a type that represents an ACPI Table.
/// 
/// ## Examples
/// 
/// ```rust
/// 
/// #[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
/// #[repr(C, packed)]
/// struct MyTable {
///   signature: u32,
///   length: u32,
///   revision: u8,
///   checksum: u8,
///   data_differing_from_header: [u8; 256]
/// }
/// 
/// // SAFETY: This table starts with the required fields
/// unsafe impl AcpiTable for MyTable {
///   const SIGNATURE: u32 = signature!('F', 'F', 'A', 'B');
/// }
/// ```
/// 
/// ## Safety
/// 
/// The structure implementing this trait must start with either a [AcpiTableHeader] or the following for
/// fields in this exact order: 
/// 
/// 1. signature: u32
/// 2. length: u32
/// 3. revision: u8
/// 4. checksum: u8
unsafe trait AcpiTable: FromBytes + IntoBytes + Immutable + KnownLayout + Packed {
    /// The signature of the table. Must match the signature field in the table.
    const SIGNATURE: u32
}

/// A derive macro for the AcpiTable trait for types that start with [AcpiTableHeader]
/// 
/// ## Examples
/// 
/// ```rust
/// #[derive(AcpiTable, FromBytes, IntoBytes, Immutable, KnownLayout)]
/// #[repr(C, packed)]
/// #[signature('F', 'F', 'A', 'B')]
/// struct MyAcpiTable {
///   header: AcpiTableHeader,
///   data: u32,
///   data2: u8,
///   other: [u32; 8],
/// }
/// 
/// #[derive(AcpiTable, FromBytes, IntoBytes, Immutable, KnownLayout)]
/// #[repr(C, packed)]
/// #[signature('F', 'F', 'B', 'B')]
/// struct OtherAcpiTable(AcpiTableHeader, [u8; 256]);
pub use patina_macro::AcpiTable;
```

### ACPI Table defintion

To support ACPI Tables that are DSTs, the interface consuming and retrieving

```rust

struct Table<'a, T: AcpiTable> {
    data: Vec<u8>,
    _phantom: PhantomData<T>,
}

impl <T: AcpiTable> Table<'_, T> {
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn as_bytes_mut(&mut self) -> &[u8] {
        &mut self.data
    }

    /// Clones and appends all elements into the ACPI Table.
    /// 
    /// Useful for DST ACPI tables that are runtime parseable objects who can have table entries appended to.
    pub fn extend_from_slice(&mut self, other: &[u8]) {
        self.data.extend_from_slice(other)
    }
}

impl <T: AcpiTable> Deref for Table<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        <T as zerocopy::FromBytes>::ref_from_bytes(&self).unwrap()
    }
}

impl <T: AcpiTable> DerefMut for Table<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        <T as zerocopy::FromBytes>::mut_from_bytes(&self).unwrap()
    }
}

impl<T: AcpiTable>TryInto<T> for Table<'_, AcpiTableHeader> {
    type Error = AcpiError;

    fn try_into(self) -> Result<T, Self::Error> {
        if self.header().signature != T::SIGNATURE {
            return AcpiError::InvalidTableFormat;
        }

        Table {
            data: self.data,
            _marker: core::marker::PhantomData
        } 
    }

}

```

### ACPI service definition

```rust 
pub trait Acpi {
    /// Installs the ACPI table and returns a key that can be used for future manipulation
    ///
    /// This method copies the byte slice into the correct memory type.
    ///
    /// ## Safety
    /// 
    /// Meet the safety requirements of [AcpiTable].
    unsafe fn install_acpi_table(&self, acpi_table: &[u8]) -> Result<TableKey, AcpiError>;

    /// Uninstalls an ACPI table associated with the provided [TableKey].
    /// 
    /// Returns the underlying bytes representing the table.
    fn uninstall_acpi_table(&self, table_key: TableKey) -> Option<Box<[u8], PageFree>>;

    /// Retrieves an ACPI table by its table key.
    fn get_acpi_table<'a>(&'a self, table_key: TableKey) -> Option<&'a [u8]>;

    /// Returns all currently installed tables in an iterable format
    fn tables<'a>(&'a self) -> &'a [&'a [u8]]
}

pub trait AcpiProviderExt {
    fn install_acpi_table<T: AcpiTable>(&self, acpi_table: &T) -> Result<TableKey, AcpiError> {
        self.install_acpi_table(acpi_table.as_bytes())
    }

    /// Returns the table if it exists, if the table signature matches expectations
    /// 
    /// ## Returns
    /// 
    /// Returns `None` if if the signatures do not match
    /// Returns `None` if no key is found for the table
    /// Returns `None` if zerocopy fails to convert the bytes
    /// 
    fn get_acpi_table<'a, T: AcpiTable>(&'a self, table_key: TableKey) -> Option<&'a T> {
        let table = self.get_acpi_table_unchecked(table_key)?;

        if table.header().signature != T::SIGNATURE {
            return None
        }

        table
    }

    /// Returns the table if it exists without validating it's signature
    /// 
    /// ## Returns
    /// 
    /// Returns `None` if no key is found for the table
    /// Returns `None` if zerocopy fails to convert the bytes
    /// 
    fn unsafe get_acpi_table_unchecked<'a, T: AcpiTable>(&'a self, table_key: TableKey) -> Option<&'a T> {
        let bytes = <Self as AcpiProvider>::get_acpi_table(self, table_key)?;

        let len = <AcpiTableHeader as FromBytes>::ref_from_prefix(bytes)?.len()
        <T as FromBytes>::ref_from_bytes(bytes[..len])?
    }
}
```

