//
// PdbHelper.cpp
//
// This file implements functions to resolve stack frame symbols from a PDB file
// using Microsoft's Debug Interface Access (DIA) SDK. It extracts source file
// names, line numbers, function names, and displacement values for a given RVA
// in a module. This wrapper is required as the DIA COM lib cannot be directly
// interacted from powershell.
//
// Exported Function:
// ------------------
// HRESULT ResolveStackFrameSymbols(
//     const wchar_t* PdbFilePath,
//     DWORD Rva,
//     BSTR* FileNameOut,
//     LONG* LineNumberOut,
//     BSTR* FunctionNameOut,
//     LONG* DisplacementOut,
//     BSTR* ErrorMessageOut
// );
//
// - Loads a PDB file and creates a DIA session.
// - Uses DIA APIs to find the corresponding source file, line number, function
//   name, and displacement for the given RVA.
// - Returns HRESULT and error messages in case of failure.
//
// HRESULT MatchModuleWithPdbFile (
//     const wchar_t* ExePath,
//     const wchar_t* PdbPath,
//     bool* IsMatched,
//     BSTR* ErrorMessageOut
// );
//
// - Loads a Exe and PDB file and creates a DIA sessions.
// - Uses DIA APIs to find the GUID, signature and age for both Exe and PDB.
// - Compares the GUID, signature and age of both Exe and PDB files.
// - Returns HRESULT and error messages in case of failure.
//
// Prerequisite:
// -------------
// - The DIA SDK must be installed(usually installed with VS 2022)
// - The resolve_stacktrace.ps1 will take care of registering msdia140.dll with
//   regsvr32 and building this file in to pdbhelper.dll.
//

#include <windows.h>
#include <dia2.h>
#include <stdio.h>
#include <strsafe.h>

extern "C" {

HRESULT ResolveStackFrameSymbolsInternal (
    IDiaSession* Session,
    DWORD Rva,
    BSTR* FileNameOut,
    LONG* LineNumberOut,
    BSTR* FunctionNameOut,
    LONG* DisplacementOut,
    BSTR* ErrorMessageOut
)
{
    IDiaSymbol* Symbol = NULL;
    LONG Displacement = 0;
    HRESULT Hr = S_OK;
    BSTR FunctionName = NULL;
    BSTR FileName = NULL;
    IDiaEnumLineNumbers* DiaEnumLineNumbers = NULL;
    IDiaLineNumber* DiaLineNumber = NULL;
    IDiaSourceFile* DiaSourceFile = NULL;
    DWORD LineNumber = 0;
    ULONG Count = 0;
    wchar_t ErrorMessageFmtBuffer[256];

    // Find the Symbol by RVA
    Hr = Session->findSymbolByRVAEx(Rva, SymTagFunction, &Symbol, &Displacement);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to find Symbol by RVA (HRESULT: 0x%lX)",
                         Hr);
        goto Exit;
    }

    // Get the function name
    if (Symbol->get_undecoratedName(&FunctionName) != S_OK || SysStringLen(FunctionName) == 0) {
        if (Symbol->get_name(&FunctionName) != S_OK || SysStringLen(FunctionName) == 0) {
           FunctionName = SysAllocString(L"(None)");
        }
    }

    // Retrieve line number
    if (FAILED(Session->findLinesByRVA(Rva, 1, &DiaEnumLineNumbers))) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to find line info by RVA (HRESULT: 0x%lX)",
                         Hr);
        goto Exit;
    }

    Hr = DiaEnumLineNumbers->Next(1, &DiaLineNumber, &Count);
    if (FAILED(Hr) || Count != 1) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to enumerate line number (HRESULT: 0x%lX)",
                         Hr);
        goto Exit;
    }

    // Retrieve line number details
    Hr = DiaLineNumber->get_lineNumber(&LineNumber);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get line number (HRESULT: 0x%lX)",
                         Hr);
        goto Exit;
    }

    // Retrieve source file name
    Hr = DiaLineNumber->get_sourceFile(&DiaSourceFile);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get source file (HRESULT: 0x%lX)",
                         Hr);
        goto Exit;
    }

    // Retrieve source file name path
    Hr = DiaSourceFile->get_fileName(&FileName);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get source file name (HRESULT: 0x%lX)",
                         Hr);
        goto Exit;
    }

    *FileNameOut = FileName;
    *LineNumberOut = LineNumber;
    *FunctionNameOut = FunctionName;
    *DisplacementOut = Displacement;

    Hr = S_OK;
Exit:
    if (DiaSourceFile != NULL) DiaSourceFile->Release();
    if (FAILED(Hr)) {
        if (FileName != NULL) SysFreeString(FileName);
        if (FunctionName != NULL) SysFreeString(FunctionName);
    }

    *ErrorMessageOut = SysAllocString(ErrorMessageFmtBuffer);
    return Hr;
}

__declspec(dllexport)
HRESULT ResolveStackFrameSymbols (
    const wchar_t* PdbFilePath,
    DWORD Rva,
    BSTR* FileNameOut,
    LONG* LineNumberOut,
    BSTR* FunctionNameOut,
    LONG* DisplacementOut,
    BSTR* ErrorMessageOut
)
{
    HRESULT Hr = S_OK;
    IDiaDataSource* DiaDataSource = NULL;
    IDiaSession* Session = NULL;
    wchar_t ErrorMessageFmtBuffer[256];

    // Initialize output parameters
    *FileNameOut = NULL;
    *LineNumberOut = 0;
    *FunctionNameOut = NULL;
    *DisplacementOut = 0;
    *ErrorMessageOut = NULL;

    // Initialize COM
    Hr = CoInitialize(NULL);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to initialize COM (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Create an instance of the DIA data source
    Hr = CoCreateInstance(__uuidof(DiaSource),
                          NULL,
                          CLSCTX_INPROC_SERVER,
                          __uuidof(IDiaDataSource),
                          (void**)&DiaDataSource);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to create DIA data source instance. (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Load the PDB file
    Hr = DiaDataSource->loadDataFromPdb(PdbFilePath);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to load PDB file (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Create a Session to access the symbols
    Hr = DiaDataSource->openSession(&Session);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to open DIA Session (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Resolve the symbols for the stack frame
    Hr = ResolveStackFrameSymbolsInternal(Session,
                                          Rva,
                                          FileNameOut,
                                          LineNumberOut,
                                          FunctionNameOut,
                                          DisplacementOut,
                                          ErrorMessageOut);
    if (FAILED(Hr)) {
        goto Cleanup;
    }

Cleanup:
    if (Session != NULL) Session->Release();
    if (DiaDataSource != NULL) DiaDataSource->Release();
    CoUninitialize();
    *ErrorMessageOut = SysAllocString(ErrorMessageFmtBuffer);
    return Hr;
}

__declspec(dllexport)
HRESULT MatchModuleWithPdbFile (
    const wchar_t* ExePath,
    const wchar_t* PdbPath,
    bool* IsMatched,
    BSTR* ErrorMessageOut
)
{
    IDiaDataSource* DiaDataSource = NULL;
    IDiaSession* DiaSession = NULL;
    IDiaSymbol* DiaSymbol = NULL;

    GUID ExeGuid;
    DWORD ExeSignature = 0;
    DWORD ExeAge = 0;

    GUID PdbGuid;
    DWORD PdbSignature = 0;
    DWORD PdbAge = 0;

    HRESULT Hr = S_OK;
    wchar_t ErrorMessageFmtBuffer[256];

    *IsMatched = false;
    *ErrorMessageOut = NULL;

    // Initialize COM
    Hr = CoInitialize(NULL);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to initialize COM (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Create an instance of the DIA data source to process the exe
    Hr = CoCreateInstance(__uuidof(DiaSource),
                          NULL,
                          CLSCTX_INPROC_SERVER,
                          __uuidof(IDiaDataSource),
                          (void**)&DiaDataSource);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to create DIA data source instance. (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Load the exe file
    Hr = DiaDataSource->loadDataForExe(ExePath, NULL, NULL);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to load exe file (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Open a session for the exe
    Hr = DiaDataSource->openSession(&DiaSession);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to open DIA Session for exe (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Retrieve the global symbol
    Hr = DiaSession->get_globalScope(&DiaSymbol);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get global scope of the exe (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Get the GUID, signature and age from the exe
    Hr = DiaSymbol->get_guid(&ExeGuid);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get GUID of the exe (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    Hr = DiaSymbol->get_signature(&ExeSignature);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get signature of the exe (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    Hr = DiaSymbol->get_age(&ExeAge);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get age of the exe (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    DiaSymbol->Release();
    DiaSession->Release();
    DiaDataSource->Release();
    DiaDataSource = NULL;
    DiaSession = NULL;
    DiaSymbol = NULL;

    // Create an instance of the DIA data source to process the PDB
    Hr = CoCreateInstance(__uuidof(DiaSource),
                          NULL,
                          CLSCTX_INPROC_SERVER,
                          __uuidof(IDiaDataSource),
                          (void**)&DiaDataSource);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to create DIA data source instance. (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Load the PDB file
    Hr = DiaDataSource->loadDataFromPdb(PdbPath);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to load PDB file (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Open a session for the PDB
    Hr = DiaDataSource->openSession(&DiaSession);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to open DIA Session for PDB (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Retrieve the global symbol
    Hr = DiaSession->get_globalScope(&DiaSymbol);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get global scope of the PDB (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Get the GUID, signature and age from the PDB
    Hr = DiaSymbol->get_guid(&PdbGuid);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get GUID of the PDB (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    Hr = DiaSymbol->get_signature(&PdbSignature);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get signature of the PDB (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    Hr = DiaSymbol->get_age(&PdbAge);
    if (FAILED(Hr)) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to get age of the PDB (HRESULT: 0x%lX)",
                         Hr);
        goto Cleanup;
    }

    // Compare the GUIDs, signatures and ages
    *IsMatched = memcmp(&PdbGuid, &ExeGuid, sizeof(GUID)) == 0 &&
               PdbSignature == ExeSignature &&
               PdbAge == ExeAge;

    Hr = S_OK;

Cleanup:
    if (DiaSymbol != NULL) DiaSymbol->Release();
    if (DiaSession != NULL) DiaSession->Release();
    if (DiaDataSource != NULL) DiaDataSource->Release();
    CoUninitialize();
    if (Hr == 0x80040154) {
        StringCchPrintfW(ErrorMessageFmtBuffer,
                         _countof(ErrorMessageFmtBuffer),
                         L"Failed to load DIA SDK. Please install the DIA SDK.");
    }
    *ErrorMessageOut = SysAllocString(ErrorMessageFmtBuffer);
    return Hr;
}

}