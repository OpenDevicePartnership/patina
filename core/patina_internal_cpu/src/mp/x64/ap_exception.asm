#
# Application processor exception and failure handling.
#
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: Apache-2.0
#

.code64
.globl ap_exception_generic
.globl ap_exception_divide_error
.globl ap_exception_breakpoint
.globl ap_exception_invalid_opcode
.globl ap_exception_double_fault
.globl ap_exception_general_protection
.globl ap_exception_page_fault
.globl ap_record_failure_current
.globl ap_record_failure

ap_exception_generic:
    xor edx, edx
    mov ecx, {failure_exception}
    jmp ap_record_failure_current

ap_exception_divide_error:
    xor edx, edx
    mov ecx, {failure_divide_error}
    jmp ap_record_failure_current

ap_exception_breakpoint:
    xor edx, edx
    mov ecx, {failure_breakpoint}
    jmp ap_record_failure_current

ap_exception_invalid_opcode:
    xor edx, edx
    mov ecx, {failure_invalid_opcode}
    jmp ap_record_failure_current

ap_exception_double_fault:
    mov rdx, qword ptr [rsp]
    mov ecx, {failure_double_fault}
    jmp ap_record_failure_current

ap_exception_general_protection:
    mov rdx, qword ptr [rsp]
    mov ecx, {failure_general_protection}
    jmp ap_record_failure_current

ap_exception_page_fault:
    mov rdx, cr2
    mov ecx, {failure_page_fault}
    jmp ap_record_failure_current

# ECX: Failure reason.
# RDX: Failure detail.
ap_record_failure_current:
    mov r9d, ecx
    mov r10, rdx

    mov ecx, 0x1B
    rdmsr
    test eax, (1 << 10)
    jnz failure_x2apic_id

    mov eax, 1
    cpuid
    shr ebx, 24
    mov r8d, ebx
    jmp failure_apic_id_ready

failure_x2apic_id:
    mov eax, 0xB
    xor ecx, ecx
    cpuid
    mov r8d, edx

failure_apic_id_ready:
    mov ecx, r9d
    mov rdx, r10

# ECX: Failure reason.
# RDX: Failure detail.
# R8D: APIC ID.
ap_record_failure:
    cli
    lea r11, [rip + AP_FAILURE]

    xor eax, eax
    mov r9d, {failure_recording}
    lock cmpxchg dword ptr [r11 + {failure_reason_off}], r9d
    jne ap_failure_halt

    mov dword ptr [r11 + {failure_apic_id_off}], r8d
    mov qword ptr [r11 + {failure_detail_off}], rdx
    mov dword ptr [r11 + {failure_reason_off}], ecx

ap_failure_halt:
    hlt
    jmp ap_failure_halt
