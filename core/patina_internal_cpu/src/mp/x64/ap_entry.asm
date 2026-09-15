#
# 64-bit AP entry point.
#
# This assembly stub is intended to take a long-mode enabled AP from another execution
# context (PEI handoff) and bring it into the Patina environment. To do this, it does the
# the fllowing:
#   1. Adopts the BSP's descriptor tables and page tables
#   2. Reads the APIC ID (xAPIC or x2APIC)
#   3. Searches the published ApContext array for the matching APIC ID to determine
#      this processor's context
#   4. Loads the per-processor stack from ApContext[n].stack_top
#   5. Reloads segment registers for the BSP's GDT
#   6. Calls the AP entry point (`ap_entry`, passed as the {ap_entry} operand)
#      with the ApContext pointer in RCX (EFI ABI), then parks when it returns.
#
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: Apache-2.0
#

.code64
.globl ap_entry_64
.globl ap_nmi_abort
.globl ap_park

ap_entry_64:
    # R9D = Indicator to skip started count
    xor r9d, r9d
    jmp setup_processor

ap_recover_64:
    # Skip incrementing started counter for recovery
    mov r9d, 1

setup_processor:
    # APs are not expecting to handle interrupts, and the EFI ABI requires DF clear.
    cli
    cld

    # Install descriptor tables whose storage and handlers are mapped by both
    # the PEI and DXE page tables. Existing segment descriptors remain cached
    # until they are explicitly reloaded below.
    lgdt [rip + AP_SETUP + {setup_gdtr_off}]
    lidt [rip + AP_SETUP + {setup_idtr_off}]

    # Apply EFER before CR3 because the BSP page tables may contain NX entries.
    mov rax, qword ptr [rip + AP_SETUP + {setup_efer_off}]
    mov rdx, rax
    shr rdx, 32
    mov ecx, 0xC0000080
    wrmsr

    # CR4 controls features such as PCID that affect CR3 interpretation.
    mov rax, qword ptr [rip + AP_SETUP + {setup_cr4_off}]
    mov cr4, rax

    mov rax, qword ptr [rip + AP_SETUP + {setup_cr0_off}]
    mov cr0, rax

    # Reset x87 state after restoring CR0 and before entering Rust.
    fninit

    # From this point onward all DXE image data, contexts, and stacks are
    # accessed through the BSP's page tables.
    mov rax, qword ptr [rip + AP_SETUP + {setup_cr3_off}]
    mov cr3, rax

    # Recovery reuses startup setup but does not count the AP a second time.
    test r9d, r9d
    jnz identify_processor
    lock inc dword ptr [rip + AP_SETUP + {setup_started_count_off}]

identify_processor:

    # Check if x2APIC is enabled via IA32_APIC_BASE MSR (0x1B), bit 10.
    mov ecx, 0x1B
    rdmsr
    test eax, (1 << 10)
    jnz x2apic_id

    # xAPIC mode
    mov eax, 1
    cpuid
    shr ebx, 24
    mov eax, ebx
    jmp got_apic_id

x2apic_id:
    # x2APIC mode.
    mov eax, 0xB
    xor ecx, ecx
    cpuid
    mov eax, edx

got_apic_id:
    # EAX = APIC ID. Save in EDX.
    mov edx, eax

    # Load the ApContext array pointer and entry count.
    mov rsi, qword ptr [rip + AP_SETUP + {setup_contexts_off}]
    mov r8d, dword ptr [rip + AP_SETUP + {setup_context_count_off}]

    # Search ApContext for our APIC ID.
    xor ecx, ecx

    # ECX = index
    # EDX = APIC ID
    # RSI = ApContext array pointer
    # R8D = ApContext count
search_loop:
    cmp ecx, r8d
    # If the entry is not found, just park the processor.
    jae ap_park

    # Calculate the entry address from the array base and context size.
    mov rdi, rcx
    imul rdi, {ap_context_size}
    cmp dword ptr [rsi + rdi + {ap_ctx_apic_off}], edx
    je found_id
    inc ecx
    jmp search_loop

found_id:
    # ECX = processor number (index)
    # EDX = APIC ID
    # RSI = ApContext array pointer
    # RDI = Context offset (index * AP_CONTEXT_SIZE)

    # Load per-processor stack from ApContext[processor_number].stack_top.
    mov rsp, [rsi + rdi + {ap_ctx_stack_off}]

    # Reload CS via far return.
    lea rax, [rip + gdt_loaded]
    push {code64_sel}
    push rax
    retfq

gdt_loaded:
    # Reload data segments with BSP's data64 selector.
    mov ax, {data64_sel}
    mov ds, ax
    mov es, ax
    mov ss, ax
    xor ax, ax
    mov fs, ax
    mov gs, ax

    # Pass the selected ApContext as the first argument (RCX for efiapi).
    lea rcx, [rsi + rdi]

    # Reserve the EFIAPI required 32-byte home space and call Rust. RSP was
    # page-aligned when loaded above, so it is also correctly aligned here.
    sub rsp, 0x20
    call {ap_entry}
    add rsp, 0x20

    # Returning from the dispatch loop is a terminal park request.
    jmp ap_park

# Vector 2 is dedicated to AP reset. Every NMI redirects through IRETQ so NMI
# blocking is cleared before the processor re-enters architectural setup.
ap_nmi_abort:
    # Vector 2 has no error code, so RSP points at the hardware-frame RIP.
    lea rax, [rip + ap_recover_64]
    mov qword ptr [rsp], rax
    # Clear TF and IF so no debug or maskable interrupt can preempt recovery
    # between IRETQ and the CLI at the recovery entry.
    and qword ptr [rsp + 16], {rflags_clear_tf_if_mask}
    iretq

# Assembly routine for jumping to the park stub. This routine does not
# use the caller's stack, so it is safe as an exception target or a direct
# x64/EFI ABI call from C or Rust.
ap_park:
    cli
    cld

    mov rcx, qword ptr [rip + AP_PARK_CONFIG]
    test rcx, rcx
    jz park_unavailable

    mov rax, qword ptr [rip + AP_PARK_ENTRY]
    test rax, rax
    jz park_unavailable
    jmp rax

park_unavailable:
    hlt
    jmp park_unavailable
