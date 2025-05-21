; armasm64 -o getsp.obj getsp.asm

    AREA    .text, CODE, READONLY
    ALIGN   4
    EXPORT  GetSp       ; Make the GetSp function exportable

GetSp PROC
    mov     x0, sp      ; Copy the current stack pointer (SP) into X0
    ret                 ; Return to caller
    ENDP
    END
