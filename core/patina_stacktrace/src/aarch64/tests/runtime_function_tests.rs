use crate::{
    aarch64::tests::{module::Module, set_logger},
    pe::PE,
};

#[test]
fn test_module_pe_runtime_functions() {
    set_logger();

    let path = r"src\aarch64\tests\collateral\aarch64.dll";
    let module = Module::load(path);
    assert!(module.is_ok());

    let module = module.unwrap();
    println!("base_address: {:X}({})", module.base_address, module.base_address);

    unsafe {
        let image = PE::locate_image(module.base_address);
        assert!(image.is_ok());

        let image = image.unwrap();

        let runtime_functions = image.find_all_functions();
        assert!(runtime_functions.is_ok());

        let runtime_functions = runtime_functions.unwrap();
        println!("runtime_functions: ");
        for func in &runtime_functions {
            println!("{}", func);
        }

        let pc_rva_in_func1 = 0x4488; // pc rva inside aarch64.dll!func1. Can be found using .fnent aarch64!func1
        let runtime_function = image.find_function(pc_rva_in_func1);
        assert!(runtime_function.is_ok());
        let runtime_function = runtime_function.unwrap();
        println!("{}", runtime_function);
    }
    module.unload();
}

#[test]
fn test_pe_locate_image() {
    set_logger();
    let path = r"src\aarch64\tests\collateral\aarch64.dll";
    let module = Module::load(path);
    assert!(module.is_ok());

    let module = module.unwrap();
    println!("base_address: {:X}({})", module.base_address, module.base_address);

    let pc_rva_in_func1 = 0x4488; // pc rva inside aarch64.dll!func1. Can be found using .fnent aarch64!func1
    let pc = module.base_address + pc_rva_in_func1;
    let image = unsafe { PE::locate_image(pc) };
    assert!(image.is_ok());
    let image = image.unwrap();
    println!("Image base/size after scanning: {:X}/{:X}", image.base_address, image._size_of_image);

    assert_eq!(image.base_address, module.base_address);

    module.unload();
}

#[test]
fn test_pe_dump() {
    set_logger();
    let path = r"src\aarch64\tests\collateral\aarch64.dll";
    let module = Module::load(path);
    assert!(module.is_ok());

    let module = module.unwrap();
    println!("base_address: {:X}({})", module.base_address, module.base_address);

    let pc_rva_in_func1 = 0x4488; // pc rva inside aarch64.dll!func1. Can be found using .fnent aarch64!func1
    let pc = module.base_address + pc_rva_in_func1;
    let image = unsafe { PE::locate_image(pc) };
    assert!(image.is_ok());
    let image = image.unwrap();
    println!("Image base/size after scanning: {:X}/{:X}", image.base_address, image._size_of_image);

    assert_eq!(image.base_address, module.base_address);

    module.unload();
}
