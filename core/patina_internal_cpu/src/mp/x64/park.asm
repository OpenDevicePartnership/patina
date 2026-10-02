#
# Position-independent long-mode AP parking loop.
#
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: Apache-2.0
#

.code64
.globl ap_park
.globl ap_park_stub_start
.globl ap_park_exception
.globl ap_park_stub_end

# Assembly routine for jumping to the reserved park stub. This routine does not
# use the caller's stack, so it is safe as an exception target or a direct
# x64/EFI ABI call from C or Rust.
ap_park:
    cli
    cld

    mov rcx, qword ptr [rip + AP_PARK_CONFIG]
    mov rax, qword ptr [rip + AP_PARK_ENTRY]
    jmp rax

# Park stub entry
#
# ECX: The base address of the park data.
#
ap_park_stub_start:
    cli
    cld

    mov rax, qword ptr [rcx + {data_cr3_off}]
    lgdt [rcx + {data_gdtr_off}]
    mov rsp, qword ptr [rcx + {data_stack_top_off}]

    # Install the final IDT before counting this terminal entry so any later
    # exception converges on the uncounted halt handler.
    lidt [rcx + {data_idtr_off}]

    # Switch to the park page tables.
    mov cr3, rax

    # Now that the context is transitioned and the AP is no longer
    # dependent on non-park memory, signal the AP has been parked.
    lock inc dword ptr [rcx + {data_parked_count_off}]

    # Clear out all non-essential state
    xor eax, eax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor edi, edi
    xor ebp, ebp
    xor r8d, r8d
    xor r9d, r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d

ap_park_halt:
    hlt
    jmp ap_park_halt

# Exception handler entry.
#
# Must not use the stack! To save memory, all APs will have the same stack pointer for
# NMI context to be pushed to, so it will be overwritten.
#
ap_park_exception:
    cli
ap_park_exception_halt:
    hlt
    jmp ap_park_exception_halt

ap_park_stub_end:
