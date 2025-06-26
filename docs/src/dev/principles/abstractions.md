# Trait Abstractions

Due to the complex nature of modern system firmware, it is important to design components or libraries with the
necessary abstractions to allow platforms or IHVs the needed customization to account for silicon, hardware or even
just platform differences. In EDK II, `LibraryClasses` serve as the abstraction point, with the library's header file
as the defined interface. In Rust, `Traits` are the primary abstraction mechanism.

Depending on your use case, your library or component may re-use an existing, well-known trait, or define its own trait.

```admonish important
Unlike EDK II, we do not use traits for code reuse. Instead, use Rust crates as explained in the
[Code Reuse](./reuse.md) section.
```

Traits are well documented in the Rust ecosystem. Here are some useful links:

- [Traits](https://doc.rust-lang.org/book/ch10-02-traits.html)
- [Advanced Traits](https://doc.rust-lang.org/book/ch19-03-advanced-traits.html)
- [D&D Example](https://desmodrone.github.io/posts/traits-101/)

## Examples

This example will show you how to define a trait, implement a trait, and also create a trait that takes a dependency
on another trait being implemented for the same type.

``` rust
    pub trait MyTraitInterface {
        fn my_function(&self) -> i32;
    }

    /// MyOtherTraitInterface requires MyTraitInterface to also be implemented
    pub trait MyOtherTraitInterface: MyTraitInterface {
        fn another_function(&self, value: i32) -> bool;
    }

    pub struct MyTraitImplementation(i32);
    impl MyTraitInterface for MyTraitImplementation {
        fn my_function(&self) -> i32 {
            self.0
        }
    }

    impl MyOtherTraitInterface for MyTraitImplementation
    {
        fn another_function(&self, value: i32) -> bool {
            self.my_function() == value
        }
    }
```

## Logging Example

In this example, we start with the existing [Log](https://docs.rs/log/latest/log/trait.Log.html)
abstraction that works with the `log` crate for, as you guessed, logging purposes. We create a
generic serial logger implementation that implements this trait, but creates an additional
abstraction point as to the underlying serial write. We use this abstraction point to create
multiple implementations that can perform a serial write, including a uart_16550, uart_pl011, and
a simple stdio writer.

``` rust
/// The starting abstraction point, the `Log` trait
use log::Log;

/// Our Abstraction point for implementing different ways to perform a serial write
pub trait SerialIO {
    fn init(&self);
    fn write(&self, buffer: &[u8]);
    fn read(&self) -> u8;
    fn try_read(&self) -> Option<u8>
}

pub struct SerialLogger<S>
where
    S: SerialIO + Send,
{
    /// An implementation of the abstraction point
    serial: S,
    /// Will not log messages above this level
    max_level: log::LevelFilter,
}

impl<S> SerialLogger<S>
where
    S: SerialIO + Send,
{
    pub const fn new(
        serial: S,
        max_level: log::LevelFilter,
    ) -> Self {
        Self { serial, max_level }
    }
}

// Implement Log on our struct. All functions in this are functions that the log trait requires
// be implemented to complete the interface implementation
impl<S> Log for SerialLogger<S>
where
    S: SerialIO + Send,
{
    fn enabled(&self, metadata: &log::MetaData) -> bool {
        return metadata.level().to_level_filter() <= self.max_level
    }

    fn log(&self, record: &log::Record) {
        let formatted = format!("{} - {}\n", record.level(), record.args())
        /// We know our "serial" object must have the "write" function, we just don't know the
        /// implementation details, which is fine.
        self.serial.write(&formatted.into_bytes());
    }

    fn flush(&self) {}
}

// Create a few implementations of the SerialIO trait

// An implementation that just reads and writes from the standard input output
struct Terminal;
impl SerialIO for Terminal {
    fn init(&self) {}

    fn write(&self, buffer: &[u8]) {
        std::io::stdout().write_all(buffer).unwrap();
    }

    fn read(&self) -> u8 {
        let buffer = &mut [0u8; 1];
        std::io::stdin().read_exact(buffer).unwrap();
        buffer[0]
    }

    fn try_read(&self) -> Option<u8> {
        let buffer = &mut [0u8; 1];
        match std::io::stdin().read(buffer) {
            Ok(0) => None,
            Ok(_) => Some(buffer[0]),
            Err(_) => None,
        }
    }
}

use uart_16550::MmioSerialPort;
struct Uart16550(usize);

impl Uart16550 {
    fn new(addr: usize) -> Self {
        Self{addr}
    }
}

impl SerialIO for Uart {
    fn init(&self) {
        unsafe { MmioSerialPort::new(self.0).init() };
    }

    fn write(&self, buffer: &[u8]) {
        let port = unsafe { MmioSerialPort::new(self.0) };

        for b in buffer {
            serial_port.send(*b);
        }
    }

    fn read(&self) -> u8 {
        let port = unsafe { MmioSerialPort::new(self.0) };
        serial_port.receive()
    }

    fn try_read(&self) -> Option<u8> {
        let port = unsafe { MmioSerialPort::new(self.0) };
        if let Ok(value) = serial_port.try_receive() {
            Some(value)
        } else {
            None
        }
    }
}

// Now we can initialize them with our implementations
fn main() {
    let terminal_logger = SerialLogger::new(Terminal, log::LevelFilter::Trace);

    let uart16550_logger = SerialLogger::new(Uart_16550::new(0x4000), log::LevelFilter::Trace);
}
```
