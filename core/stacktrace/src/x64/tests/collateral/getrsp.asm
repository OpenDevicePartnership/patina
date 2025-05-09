; ml64 /c /Fogetrsp.obj getrsp.asm


.code

; Function to get the current RSP (Stack Pointer)
GetRsp PROC
    mov rax, rsp        ; Load the current RSP into RAX
    add rax, 8          ; Account for this function call
    ret                 ; Return to caller
GetRsp ENDP

END
