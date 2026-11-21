# RFC: `patina_acpi` service interface improvements

This RFC proposes interface changes to reduce the two services (`Service<dyn AcpiProvider>` and
`Service<AcpiTableManager`>) to a single `Service<dyn Acpi>` service and provide a public Helper struct, `Table` for
managing ACPI tables both internal to the service implementation and as a consumer of the service. This `Table`
structure would better support multiple types of ACPI tables, such as statically sized and dynamically sized ACPI
tables.

## Change Log

- 2026-04-22: Initial RFC created.

## Motivation

The main motivation behind this change is to add support for DSTs (Dynamically Sized Types) in the generics portion of
the service interface. DST ACPI tables are currently supported via usage of the non-generic interfaces ACPI service
methods, as they simply take a slice of bytes that start with an ACPI table header. This puts all safety burdens on the
developer. The proposed interface changes and additional support for DSTs would be highly beneficial as it would
provide a standard interface for interacting with both static and dynamically sized ACPI tables.

Supporting DSTs will require some fundamental changes to how ACPI table data is managed (described in-depth below). The
high-level description is that we will be removing the current generic table structure in favor of `Box<[u8]>`, which
self manages the ACPI page allocation while relying on `zerocopy` to interpret the table byte prefixes as an
`AcpiTableHeader`. Due to this, it will remove a large chunk of `unsafe` blocks throughout the crate.

The final benefit is that of safety. This change removes many different areas of the code wrapped in `unsafe`, and most
importantly, stops the `get_acpi_table` method from returning a pointer directly to the actual ACPI table in memory.
Instead, it will now copy the bytes into new memory and provide that to the user. The EDKII EFIAPI side will need to
continue to provide the pointer the actual ACPI table in memory.

## Technology Background

Please refer to [RFC 5 - ACPI](https://github.com/OpenDevicePartnership/patina/blob/main/docs/src/rfc/text/0005-acpi.md)'s
technology background. While these changes do not directly impact the logic related to ACPI table management, it is
touching the code that does so.

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

- Do we want to specify the revision in the `AcpiTable` trait and require that revision matches in addition to
  signature?

## Alternatives

The alternative is to continue the status quo. As mentioned above, DSTs are currently supported via the non-generic
service interface, but it requires you to (1) Manually construct the `AcpiTable` struct (A wrapper around a raw
pointer), (2) Ensure you've allocated to the correct memory type yourself, and (3) Manually attempt casting a generic
table to a table of your choosing.

## Prior Art

The prior implementation generically stored each ACPI table as a pointer to a leaked union representing the ACPI
table. It would then provide direct pointer access to the caller of `get_acpi_table` both for the patina component
and caller of the EDK II protocol. Additionally, the generic methods (such as `get_acpi_table<T>`) would directly
return a `T`, making it difficult to use DST style ACPI tables, but did successfully clone the structure so that
it was not directly referencing the true ACPI table memory.

## Rust Code Design

This change is focused mostly on the service API to provide common logic for table management (creation, updating)
on the consumer side of the service. Only a very minimal set of changes are made to the service implementation to
accommodate the interface changes. These changes will remove unsafe code in the service implementation as table
access will move away from pointers and dereferencing to slices and safe byte interpretation via `zerocopy`.

Starting with changes to the service implementation and data management: A new unsafe trait, `AcpiTable` is created to
house all unsafe requirements that must be upheld to prevent UB from within the service. This is mainly that the table
must start with an `AcpiTableHeader`, but it also enforces your ACPI table implement certain `zerocopy` traits to
ensure the service can re-interpret the prefix bytes as an AcpiTableHeader. A derive macro is provided to automatically
do this validation when implementing the `AcpiTable` trait. Implementation details will be discussed below.

On the consumer side service changes, a new struct, `Table` is used to ensure you can safely work with your ACPI table
without breaking any UB guarantees. While this table supports statically sized tables (allowing for safe interpretation
of the table as the underlying type), its focus is on simplifying the data management when it comes to dynamically
sized tables. Broadly speaking, it provides an API for inserting, removing, and manipulating elements at the end of
a dynamically sized ACPI table with formats similar to this:

```rust
struct CustomAcpiTable {
    header: AcpiTableHeader,
    fields: CustomFields,
    elements: [CustomElement]
}
```

It should be noted that this interface allows the elements to be statically sized or dynamically sized. More details,
and the provided interface, will be discussed below.

### ACPI Table Trait Definition

The ACPI table trait (`AcpiTable`) is dual hatted. It provides safety guarantees for the service implementation, and
provides important information to the `Table` struct (as described below) to provide safe table manipulation by
consumers of `patina_acpi`.

```rust
/// A trait for types that represent valid ACPI tables.
/// 
/// This trait should not be implemented manually. Use the derive macro to implement this trait on a type representing
/// an ACPI table.
/// 
/// # Safety
/// 
/// Implementors must ensure that:
/// - The type begins with an `AcpiTableHeader` as its first field
/// 
/// # Examples
/// 
/// <Skipped for brevity here as there will be examples below>
/// ```
pub unsafe trait AcpiTable: FromBytes + IntoBytes + KnownLayout + Unaligned + Immutable {
    /// The ACPI table signature, used for validation when converting a table to the implementor type
    const SIGNATURE: u32;

    /// The single struct that contains all additional fields before the table of optional [Self::Element]s 
    type Fields: IntoBytes + Immutable = ();

    /// The element type for the variable-length array portion of DST ACPI tables
    type Element: Element + ?Sized = ();

    /// The kind of ACPI table this is (Fixed, Dst). Used to gate slice style methods to only DST ACPI tables.
    #[allow(private_bounds)]
    type Kind: TableKind = Fixed;
}

/// An element that can be used for the variable-length array portion of DST ACPI tables.
/// 
/// This trait has two roles: (1) Ensure that each element can be converted to and from bytes using `zerocopy` and
/// (2) to differentiate between Sized and Unsized elements for indexing optimizations.
pub trait Element: IntoBytes + FromBytes + Immutable + KnownLayout + Unaligned {
    /// A non-zero size for types that are `Sized`.
    /// 
    /// Element::SIZE == 0 indicates that this is a DST and that [Self::element_size] must be used to determine size.
    const SIZE: usize = 0;

    /// Returns the runtime size (in bytes) of the element.
    /// 
    /// This is used to determine the size (in bytes) of the element. 
    fn element_size(&self) -> usize;
}

/// A blanket implementation of `Element` for all sized types
impl <E: IntoBytes + FromBytes + Immutable + KnownLayout + Unaligned> Element for E {
    const SIZE: usize = core::mem::size_of::<E>();

    fn element_size(&self) -> usize {
        Self::SIZE
    }
}


/// A trait to specify the type of ACPI table this is
/// 
/// It is purposefully kept private so only our implementations can be used.
trait TableKind { }

#[doc(hidden)]
pub struct Fixed;

impl TableKind for Fixed { }

#[doc(hidden)]
pub struct Dst;

impl TableKind for Dst { }
```

### ACPI Table Management Changes

As mentioned above, there are minimal changes to the table management logic inside the service itself. The core of the
change is that the object that manages the underlying data for each table is being changed from a custom struct that
wraps a leaked pointer, to a `Box<[u8]>`. In the current implementation, a union is used to treat the underlying data as
either a (1) signature, (2) AcpiTableHeader, or (3), the entire table. In the new implementation as seen below, we will
utilize `zerocopy` to safely interpret the prefix bytes as an `AcpiTableHeader`, which is used for all interior table
management. Other than some helper functions to assist with this, no other changes will be made.

```rust
struct StandardAcpiProvider<B: BootServices + 'static> {
    /// Platform-installed ACPI tables
    acpi_tables: TplMutex<BTreeMap<usize, Box<[u8], PageFree>>>,
    ...
}

impl<B: BootServices> AcpiProvider for StandardAcpiProvider<B> {
    /// Installs the bytes as an ACPI table
    /// 
    /// Returns a key usable with [Acpi::get_table].
    /// 
    /// ## Safety
    /// 
    /// - The table must begin with a valid [AcpiTableHeader]
    unsafe fn install_acpi_table(&self, table: &[u8]) -> Result<u32, AcpiError> {
        let table: Box<[u8], PageFree> = Self::allocate_table(table)?;
        let header = AcpiTableHeader::ref_from_bytes(&table).map_err(|_| AcpiError::InvalidLength);
        
        let table_key = match header.signature {
            signature::FACS => { /* Special Handling */ }
            signature::FADT => { /* Special Handling */ }
            signature::DSDT => { /* Special Handling */ }
            _ => self.install_generic_table(table)?
        };

        self.publish_tables()?;
        self.notify_acpi_list(table_key)?;
        Ok(table_key)
    }

    /// Uninstalls the ACPI table.
    fn uninstall_acpi_table(&self, table_key: usize) -> Result<(), AcpiTableError> {
        // Uninstalling a table is slightly simplified as the Box<[u8]> will automatically deallocate the table once
        // it is removed from the BTreeMap and goes out of scope.
        //
        // Note: We are still responsible for updating the XSDT or other special table handling for the DSDT / FACS
    }

    /// Returns a copy of the table if it exists.
    fn get_table(&self, key: &usize) -> Result<Table<AcpiTableHeader>, AcpiTableError> {
        let copied_bytes: Vec<u8> = self.acpi_tables.lock().get(key).ok_or(AcpiError::NotFound)?.as_ref().clone();
        let table: Table<AcpiTableHeader> = Table {
            data: copied_bytes,
            _marker: PhantomData,
        };

        Ok(table)
    }
}

```

### ACPI Service Definition

Below is the updated ACPI service definition that fully replaces the `AcpiTableManager` and `AcpiProvider` services.

```rust
pub trait Acpi {
    /// Installs the bytes as an ACPI table
    /// 
    /// Returns a key usable with [Acpi::get_table].
    /// 
    /// ## Safety
    /// 
    /// - The bytes must represent a valid ACPI Table. A valid ACPI Table meets the same safety requirements as
    ///   required for the [AcpiTable] trait.
    unsafe fn install_table(&self, table: &[u8]) -> Result<u32, AcpiError>;

    /// Gets a table by its key.
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::NotFound] if the table is not found.
    /// Returns [AcpiError::InvalidLength] if the table is not long enough for an [AcpiTableHeader].
    fn get_table(&self, key: usize) -> Result<Table<AcpiTableHeader>, AcpiError>;

    /// Removes a table and returns it to the caller
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::NotFound] if the table is not found.
    /// Returns [AcpiError::InvalidLength] if the table is not long enough for an [AcpiTableHeader].
    fn uninstall_table(&self, key: usize) -> Result<Table<AcpiTableHeader>, AcpiError>;

    /// Registers a function that will be called whenever a new ACPI table is installed.
    fn register_notify(&self, notify_fn: AcpiNotifyFn);

    /// Unregisters a function that will be called whenever a new ACPI table is installed.
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::InvalidNotifyUnregister] if the notify fn was not found.
    fn unregister_notify(&self, notify_fn: AcpiNotifyFn) -> Result<(), AcpiError>;

    /// Returns a copy of all registered ACPI tables.
    fn collect_tables(&self) -> Vec<Table<AcpiTableHeader>>;
}

/// An extension trait to provide generic methods for convenience
pub trait AcpiExt {
    /// Installs the ACPI Table
    /// 
    /// Returns a key usable with [Acpi::get_table].
    /// 
    fn install_table<T: AcpiTable>(&self, table: &T) -> Result<u32, AcpiError>;

    /// Gets a table by its key.
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::NotFound] if the table is not found
    /// Returns [AcpiError::InvalidSignature] if the table signature does not match [T::SIGNATURE]
    /// Returns [AcpiError::BadFormat] if the conversion to `T` failed.
    fn get_table<T: AcpiTable>(&self, key: u32) -> Result<Table<T>, AcpiError>;

    /// A convenience method to uninstall a table, edit it, and re-install the table
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::NotFound] if the table is not found
    /// Returns [AcpiError::InvalidSignature] if the table signature does not match T::SIGNATURE
    /// Returns [AcpiError::BadFormat] if the conversion to `T` failed.
    #[must_use]
    fn mutate_table<F, T>(&self, key: u32, f: F) -> Result<(), AcpiError>
    where
        T: AcpiTable,
        F: FnOnce(&mut Table<T>) -> Result<(), AcpiError>;
}

/// This is a way to have generic methods on a service interface, by implementing the generics on the
/// service itself.
impl AcpiExt for Service<dyn Acpi> { ... }

```

### Consumer ACPI Table Management

As alluded to, a major portion of this change was to better support ACPI table management generically; particularly
for DST style ACPI tables. This change introduces the `Table` structure, as referenced many times above, as the way to
manage ACPI tables. As seen above, this structure is used by the service implementation to treat all tables generically
as just their ACPI header, but it also provides an interface for consumers of the service to safely generate ACPI
tables that can consumed by the service. The main purpose is to create an easy way to generate and consume dynamically
sized ACPI tables outside of the service.

The `Table` structure provides a rich interface for creating ACPI Tables and working with DSTs as if they were a vector
of the underlying elements. That is to say, an ACPI Table with a layout of `{ AcpiHeader, [MyElement] }` will
have an interface similar to a `Vec<MyElement>`. Below is the provided interface. The implementations are left out for
brevity's sake

```rust
pub struct Table<T: AcpiTable + ?Sized> {
    data: Vec<u8>,
    _marker: PhantomData<T>,
}

/// Methods in this impl block are common to all ACPI tables.
impl <T: AcpiTable + ?Sized> Table<T> {
    /// Creates a new ACPI table with the given header and fields.
    /// 
    /// If your structure has no fields, consider using [Table::new] instead.
    pub fn new_with_fields(header: AcpiTableHeader, fields: T::Fields) -> Self;

    /// Updates the length field of the header to match [Self::data]'s length
    fn update_length(&mut self);

    /// Performs a checksum and sets the checksum field to it.
    fn checksum(&mut self);

    /// Returns a reference to the ACPI table header.
    fn header(&self) -> &AcpiTableHeader;

    /// Returns a mutable reference to the ACPI table header.
    fn header_mut(&mut self) -> &mut AcpiTableHeader;

    /// Consumes the table and returns the underlying bytes.
    pub fn into_bytes(self) -> Vec<u8>;
}

/// Methods in this impl block are specific to ACPI tables with no fields.
/// 
/// ## Panic
/// 
/// This function will panic if there is a mismatch of the expected table signature.
impl <T: AcpiTable<Fields = ()>> Table<T> {
    /// Creates a new ACPI table with no fields.
    pub fn new(header: AcpiTableHeader) -> Self;
}

/// Methods in this impl block are specific to ACPI tables with a dynamically sized list of elements at the end.
impl <T: AcpiTable<Kind = Dst> + ?Sized> Table<T> {
    /// Returns the number of elements in the table.
    pub fn len(&self) -> usize;

    /// Returns `true` if the table contains no elements.
    pub fn is_empty(&self) -> bool;

    /// Pushes a new [AcpiTable::Element] to the end of the table.
    /// 
    /// Useful for ACPI tables that have a dynamically sized list of elements at the end.
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::AllocationError] if the new table size would exceed [u32::MAX].
    pub fn push(&mut self, element: &T::Element);

    /// Pushes multiple [AcpiTable::Element]s to the end of the table.
    /// 
    /// Useful for ACPI tables that have a dynamically sized list of elements at the end.
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::AllocationError] if the new table size would exceed [u32::MAX].
    pub fn extend_from_slice(&mut self, elements: &[&T::Element]) -> Result<(), AcpiError>;

    /// Returns a reference to the element at the given index, if it exists.
    pub fn get(&self, index: usize) -> Option<&T::Element>;

    /// Returns a mutable reference to the element at the given index, if it exists.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T::Element>;

    /// Removes the element at the given index from the table and returns it, if it exists.
    /// 
    /// **Note:** Since the element may be a DST, the underlying bytes are returned instead of the type.
    pub fn remove(&mut self, index: usize) -> Option<Vec<u8>>;

    /// Removes the last element from the table and returns it, if it exists.
    /// 
    /// **Note:** Since the element may be a DST, the underlying bytes are returned instead of the type.
    pub fn pop(&mut self) -> Option<Vec<u8>>;

    /// Returns an iterator over the elements of the table.
    pub fn iter(&self) -> impl Iterator<Item = &T::Element>;

    /// Returns a mutable iterator over the elements of the table.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T::Element>;
}

impl Table<AcpiTableHeader> {
    /// Converts a generic ACPI table into a specific table.
    /// 
    /// ## Errors
    /// 
    /// Returns [AcpiError::InvalidSignature] if the table signature does not match T::SIGNATURE.
    /// Returns [AcpiError::InvalidLength] if the table is not long enough for a T.
    fn try_into<T: AcpiTable + ?Sized>(self) -> Result<Table<T>, AcpiError>;
}

impl <T: AcpiTable + ?Sized> Deref for Table<T> {
    type Target = T;
}

impl <T: AcpiTable + ?Sized> DerefMut for Table<T>

impl <T: AcpiTable<Kind = Dst> + ?Sized> Index<usize> for Table<T> {
    type Output = T::Element;
}

impl <T: AcpiTable<Kind = Dst> + ?Sized> IndexMut<usize> for Table<T>

impl <'a, T: AcpiTable<Kind = Dst> + ?Sized> IntoIterator for &'a Table<T> {
    type Item = &'a T::Element;
    type IntoIter = Iter<'a, T>;
}

impl <'a, T: AcpiTable<Kind = Dst> + ?Sized> IntoIterator for &'a mut Table<T> {
    type Item = &'a mut T::Element;
    type IntoIter = IterMut<'a, T>;
}
```

### Consumer ACPI Table Management Examples

This first example is consumer interface for statically sized ACPI tables

```rust
#[derive(FromBytes, IntoBytes, KnownLayout, Unaligned, Immutable)]
struct Fields {
    a: u32,
    b: u32,
    c: u64,
    d: u64
}

#[derive(AcpiTable, FromBytes, IntoBytes, KnownLayout, Unaligned, Immutable)]
#[signature = "STBL"]
struct SimpleTable {
    header: AcpiTableHeader,
    fields: Fields,
}

struct MyComponent;

#[component]
impl MyComponent {
    fn entry_point(self, acpi: Service<dyn Acpi>) -> Result<()> {

        let header = AcpiTableHeader { ... };

        // let table = Table::new(header) -> This method is not available because this struct has fields
        let mut table: Table<SimpleTable> = Table::new_with_fields(header, Fields { a: 20, b: 40, c: 60, d: 70});

        let key = acpi.install_table(table).unwrap();

        acpi.mutate_table(key, |table: &mut Table<SimpleTable>| {
            table.fields.c = 200;
        }).unwrap()

        assert_eq!(acpi.get_table(key).unwrap().fields.c, 200);

        Ok(())
    }
}
```

The second example is a DST style table where the element is statically sized

```rust
#[derive(FromBytes, IntoBytes, KnownLayout, Unaligned, Immutable)]
struct Element {
    a: u32,
    b: u32,
}

#[derive(AcpiTable, FromBytes, IntoBytes, KnownLayout, Unaligned, Immutable)]
#[signature('S', 'T', 'B', 'L')]
struct DstTable {
    header: AcpiTableHeader,
    elements: [Element],
}

struct MyComponent;

#[component]
impl MyComponent {
    fn entry_point(self, acpi: Service<dyn Acpi>) -> Result<()> {

        let header = AcpiTableHeader { ... };

        let mut table: Table<DstTable> = Table::new(header);

        table.push(&Element { a: 20, b: 40 })
        table.push(&Element { a: 60, b: 80 })
        table.extend_from_slice(&[&Element { a: 100, b: 120}, &Element { a: 140, b: 160 }]);

        assert_eq!(table.len(), 4);

        let _ table.pop().unwrap();

        assert_eq!(table.len(), 3);

        let iter = table.iter();

        let _ = table.get(0).unwrap();
        let _ = table[0];

        let key = acpi.install_table(table).unwrap();

        acpi.mutate_table(key, |table: &mut Table<SimpleTable>| {
            for element in &mut table {
                element.a *= 2;
            }
        }).unwrap();

        Ok(())
    }
}
```
