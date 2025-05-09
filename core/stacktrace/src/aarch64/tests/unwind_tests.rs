use crate::{
    aarch64::{
        tests::{module::Module, set_logger},
        unwind::UnwindCode,
    },
    pe::PE,
};

#[test]
fn test_unwind_info() {
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
            println!("Function Name: {}", func);

            let unwind_info = func.get_unwind_info().unwrap();
            println!("unwind_info: {}", unwind_info);

            let (lr_offset, sp_offset) = unwind_info.get_stack_pointer_offset().unwrap();
            println!("lr_offset: {} 0x{:X} sp_offset: {} 0x{:X}", lr_offset, lr_offset, sp_offset, sp_offset);
            println!("------------------------------")
        }
    }

    module.unload();
}

#[test]
fn test_unwind_info_one_function() {
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

        let runtime_function = image.find_function(0x01094);
        assert!(runtime_function.is_ok());

        let runtime_function = runtime_function.unwrap();
        println!("runtime_functions: ");

        let unwind_info = runtime_function.get_unwind_info().unwrap();
        println!("unwind_info: {}", unwind_info);

        let (lr_offset, sp_offset) = unwind_info.get_stack_pointer_offset().unwrap();
        println!("lr_offset: {} 0x{:X} sp_offset: {} 0x{:X}", lr_offset, lr_offset, sp_offset, sp_offset);
    }

    module.unload();
}

#[test]
fn test_parse_unwind_codes() {
    let bytes = [
        0b00000000, // 0 AllocS(0)
        0b00000001, // 1 AllocS(1)
        0b00011111, // 2 AllocS(31)
        0b00100000, // 3 SaveR19R20X(0)
        0b00111111, // 4 SaveR19R20X(31)
        0b01000000, // 5 SaveFpLr(0)
        0b01111111, // 6 SaveFpLr(63)
        0b10000000, // 7 SaveFpLrX(0)
        0b10111111, // 8 SaveFpLrX(63)
        0b11000000, 0b00000000, // 9 AllocM(0)
        0b11000000, 0b00000001, // 10. AllocM(1)
        0b11000000, 0b11111111, // 11. AllocM(255)
        0b11001000, 0b00000000, // 12. SaveRegP(0, 0)
        0b11001000, 0b00000001, // 13. SaveRegP(0, 1)
        0b11001000, 0b11111111, // 14. SaveRegP(3, 63)
        0b11001100, 0b00000000, // 15. SaveRegPX(0, 0)
        0b11001100, 0b00000001, // 16. SaveRegPX(0, 1)
        0b11001100, 0b11111111, // 17. SaveRegPX(3, 63)
        0b11010000, 0b00000000, // 18. SaveReg(0, 0)
        0b11010000, 0b00000001, // 19. SaveReg(0, 1)
        0b11010000, 0b11111111, // 20. SaveReg(0, 255)
        0b11010100, 0b00000000, // 21. SaveRegX(0, 0)
        0b11010100, 0b00000001, // 22. SaveRegX(0, 1)
        0b11010100, 0b11111111, // 23. SaveRegX(0, 255)
        0b11010110, 0b00000000, // 24. SaveLrPair(0, 0)
        0b11010110, 0b00000001, // 25. SaveLrPair(0, 1)
        0b11010110, 0b11111111, // 26. SaveLrPair(0, 255)
        0b11011000, 0b00000000, // 27. SaveFRegP(0, 0)
        0b11011000, 0b00000001, // 28. SaveFRegP(0, 1)
        0b11011000, 0b11111111, // 29. SaveFRegP(0, 255)
        0b11011010, 0b00000000, // 30. SaveFRegPX(0, 0)
        0b11011010, 0b00000001, // 31. SaveFRegPX(0, 1)
        0b11011010, 0b11111111, // 32. SaveFRegPX(0, 255)
        0b11011100, 0b00000000, // 33. SaveFReg(0, 0)
        0b11011100, 0b00000001, // 34. SaveFReg(0, 1)
        0b11011100, 0b11111111, // 35. SaveFReg(0, 255)
        0b11011110, 0b00000000, // 36. SaveFRegX(0, 0)
        0b11011110, 0b00000001, // 37. SaveFRegX(0, 1)
        0b11011110, 0b11111111, // 38. SaveFRegX(0, 255)
        0b11100000, 0b00000000, 0b00000000, 0b00000000, // 39. AllocL(0)
        0b11100000, 0b00000000, 0b00000000, 0b00000001, // 40. AllocL(1)
        0b11100000, 0b00000000, 0b00000000, 0b11111111, // 41. AllocL(255)
        0b11100001, // 42. SetFp
        0b11100010, 0b00000000, // 43. AddFp(0)
        0b11100010, 0b00000001, // 44. AddFp(1)
        0b11100010, 0b11111111, // 45. AddFp(255)
        0b11100011, // 46. Nop
        0b11100100, // 47. End
        0b11100101, // 48. EndC
        0b11100110, // 49. SaveNext
        0b11111100, // 50. PacSignLr
        0b11100111, // 51. Reserved1
        0b11101000, // 52. Reserved3
        0b11101001, // 53. Reserved4
        0b11101010, // 54. Reserved5
        0b11101011, // 55. Reserved6
        0b11101100, // 56. Reserved7
        0b11101101, // 57. Reserved8
        0b11101110, // 58. Reserved9
        0b11101111, // 59. Reserved20
        0b11111000, 0b00000000, // 60. Reserved12(0)
        0b11111000, 0b00000001, // 61. Reserved12(1)
        0b11111000, 0b11111111, // 62. Reserved12(255)
        0b11111001, 0b00000000, 0b00000000, // 63. Reserved13(0)
        0b11111001, 0b00000000, 0b00000001, // 64. Reserved13(1)
        0b11111001, 0b11111111, 0b11111111, // 65. Reserved13(65535)
        0b11111010, 0b00000000, 0b00000000, 0b00000000, // 66. Reserved14(0)
        0b11111010, 0b00000000, 0b00000000, 0b00000001, // 67. Reserved14(1)
        0b11111010, 0b11111111, 0b11111111, 0b11111111, // 68. Reserved14(16777215)
        0b11111011, 0b00000000, 0b00000000, 0b00000000, 0b00000000, // 69. Reserved15(0)
        0b11111011, 0b00000000, 0b00000000, 0b00000000, 0b00000001, // 70. Reserved15(1)
        0b11111011, 0b11111111, 0b11111111, 0b11111111, 0b11111111, // 71. Reserved15(4294967295)
        0b11111101, // 72. Reserved16
        0b11111110, // 73. Reserved17
        0b11111111, // 74. Reserved18
    ];
    let res = UnwindCode::parse(&bytes);
    assert!(res.is_ok());
    let res = res.unwrap();
    assert_eq!(res.len(), 75);
    assert_eq!(res[0], UnwindCode::AllocS(0));
    assert_eq!(res[1], UnwindCode::AllocS(1));
    assert_eq!(res[2], UnwindCode::AllocS(31));
    assert_eq!(res[3], UnwindCode::SaveR19R20X(0));
    assert_eq!(res[4], UnwindCode::SaveR19R20X(31));
    assert_eq!(res[5], UnwindCode::SaveFpLr(0));
    assert_eq!(res[6], UnwindCode::SaveFpLr(63));
    assert_eq!(res[7], UnwindCode::SaveFpLrX(0));
    assert_eq!(res[8], UnwindCode::SaveFpLrX(63));
    assert_eq!(res[9], UnwindCode::AllocM(0));
    assert_eq!(res[10], UnwindCode::AllocM(1));
    assert_eq!(res[11], UnwindCode::AllocM(255));
    assert_eq!(res[12], UnwindCode::SaveRegP(0, 0));
    assert_eq!(res[13], UnwindCode::SaveRegP(0, 1));
    assert_eq!(res[14], UnwindCode::SaveRegP(3, 63));
    assert_eq!(res[15], UnwindCode::SaveRegPX(0, 0));
    assert_eq!(res[16], UnwindCode::SaveRegPX(0, 1));
    assert_eq!(res[17], UnwindCode::SaveRegPX(3, 63));
    assert_eq!(res[18], UnwindCode::SaveReg(0, 0));
    assert_eq!(res[19], UnwindCode::SaveReg(0, 1));
    assert_eq!(res[20], UnwindCode::SaveReg(3, 63));
    assert_eq!(res[21], UnwindCode::SaveRegX(0, 0));
    assert_eq!(res[22], UnwindCode::SaveRegX(0, 1));
    assert_eq!(res[23], UnwindCode::SaveRegX(7, 31));
    assert_eq!(res[24], UnwindCode::SaveLrPair(0, 0));
    assert_eq!(res[25], UnwindCode::SaveLrPair(0, 1));
    assert_eq!(res[26], UnwindCode::SaveLrPair(3, 63));
    assert_eq!(res[27], UnwindCode::SaveFRegP(0, 0));
    assert_eq!(res[28], UnwindCode::SaveFRegP(0, 1));
    assert_eq!(res[29], UnwindCode::SaveFRegP(3, 63));
    assert_eq!(res[30], UnwindCode::SaveFRegPX(0, 0));
    assert_eq!(res[31], UnwindCode::SaveFRegPX(0, 1));
    assert_eq!(res[32], UnwindCode::SaveFRegPX(3, 63));
    assert_eq!(res[33], UnwindCode::SaveFReg(0, 0));
    assert_eq!(res[34], UnwindCode::SaveFReg(0, 1));
    assert_eq!(res[35], UnwindCode::SaveFReg(3, 63));
    assert_eq!(res[36], UnwindCode::SaveFRegX(0, 0));
    assert_eq!(res[37], UnwindCode::SaveFRegX(0, 1));
    assert_eq!(res[38], UnwindCode::SaveFRegX(3, 63));
    assert_eq!(res[39], UnwindCode::AllocL(0));
    assert_eq!(res[40], UnwindCode::AllocL(1));
    assert_eq!(res[41], UnwindCode::AllocL(255));
    assert_eq!(res[42], UnwindCode::SetFp);
    assert_eq!(res[43], UnwindCode::AddFp(0));
    assert_eq!(res[44], UnwindCode::AddFp(1));
    assert_eq!(res[45], UnwindCode::AddFp(255));
    assert_eq!(res[46], UnwindCode::Nop);
    assert_eq!(res[47], UnwindCode::End);
    assert_eq!(res[48], UnwindCode::EndC);
    assert_eq!(res[49], UnwindCode::SaveNext);
    assert_eq!(res[50], UnwindCode::PacSignLr);
    assert_eq!(res[51], UnwindCode::Reserved1);

    assert_eq!(res[52], UnwindCode::Reserved3);
    assert_eq!(res[53], UnwindCode::Reserved4);
    assert_eq!(res[54], UnwindCode::Reserved5);
    assert_eq!(res[55], UnwindCode::Reserved6);
    assert_eq!(res[56], UnwindCode::Reserved7);
    assert_eq!(res[57], UnwindCode::Reserved8);
    assert_eq!(res[58], UnwindCode::Reserved9);
    assert_eq!(res[59], UnwindCode::Reserved10);

    assert_eq!(res[60], UnwindCode::Reserved12(0));
    assert_eq!(res[61], UnwindCode::Reserved12(1));
    assert_eq!(res[62], UnwindCode::Reserved12(255));
    assert_eq!(res[63], UnwindCode::Reserved13(0));
    assert_eq!(res[64], UnwindCode::Reserved13(1));
    assert_eq!(res[65], UnwindCode::Reserved13(65535));
    assert_eq!(res[66], UnwindCode::Reserved14(0));
    assert_eq!(res[67], UnwindCode::Reserved14(1));
    assert_eq!(res[68], UnwindCode::Reserved14(16777215));
    assert_eq!(res[69], UnwindCode::Reserved15(0));
    assert_eq!(res[70], UnwindCode::Reserved15(1));
    assert_eq!(res[71], UnwindCode::Reserved15(4294967295));
    assert_eq!(res[72], UnwindCode::Reserved16);
    assert_eq!(res[73], UnwindCode::Reserved17);
    assert_eq!(res[74], UnwindCode::Reserved18);
}
