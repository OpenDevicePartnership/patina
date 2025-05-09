use crate::{
    pe::PE,
    x64::{
        runtime_function::RuntimeFunction,
        tests::{module::Module, set_logger},
    },
};

#[test]
fn test_module_pe_runtime_functions() {
    set_logger();

    let path = r"src\x64\tests\collateral\x64.dll";
    let module = Module::load(path);
    assert!(module.is_ok());

    let module = module.unwrap();
    println!("base_address: {:X}({})", module.base_address, module.base_address);

    unsafe {
        let image = PE::locate_image(module.base_address);
        assert!(image.is_ok());

        let image = image.unwrap();

        let runtime_functions = RuntimeFunction::find_all_functions(&image);
        assert!(runtime_functions.is_ok());

        let runtime_functions = runtime_functions.unwrap();
        println!("runtime_functions: ");
        for func in &runtime_functions {
            println!("{}", func);
        }

        let rip_rva_in_func1 = 0x1080; // rip rva inside x64.dll!func1. Can be found using .fnent x64!func1
        let runtime_function = RuntimeFunction::find_function(&image, rip_rva_in_func1);
        assert!(runtime_function.is_ok());
        let runtime_function = runtime_function.unwrap();
        println!("{}", runtime_function);
    }
    module.unload();
}

#[test]
fn test_pe_locate_image() {
    set_logger();
    let path = r"src\x64\tests\collateral\x64.dll";
    let module = Module::load(path);
    assert!(module.is_ok());

    let module = module.unwrap();
    println!("base_address: {:X}({})", module.base_address, module.base_address);

    let rip_rva_in_func1 = 0x1080; // rip rva inside x64.dll!func1. Can be found using .fnent x64!func1
    let rip = module.base_address + rip_rva_in_func1;
    let image = unsafe { PE::locate_image(rip) };
    assert!(image.is_ok());
    let image = image.unwrap();
    println!("Image base/size after scanning: {:X}/{:X}", image.base_address, image._size_of_image);

    assert_eq!(image.base_address, module.base_address);

    module.unload();
}

#[test]
fn test_pe_dump() {
    set_logger();
    let path = r"src\x64\tests\collateral\x64.dll";
    let module = Module::load(path);
    assert!(module.is_ok());

    let module = module.unwrap();
    println!("base_address: {:X}({})", module.base_address, module.base_address);

    let rip_rva_in_func1 = 0x1080; // rip rva inside x64.dll!func1. Can be found using .fnent x64!func1
    let rip = module.base_address + rip_rva_in_func1;
    let image = unsafe { PE::locate_image(rip) };
    assert!(image.is_ok());
    let image = image.unwrap();
    println!("Image base/size after scanning: {:X}/{:X}", image.base_address, image._size_of_image);

    assert_eq!(image.base_address, module.base_address);

    module.unload();
}
