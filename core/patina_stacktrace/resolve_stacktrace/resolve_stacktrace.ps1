#
# This script resolves symbols for raw stack frames (with module + offset) into
# more useful stack frames, displaying module!function+offset along with other
# relevant information such as file names and line numbers. It takes a directory
# containing PDB files and a raw stack as parameters. Optionally, if the
# PdbDirectory contains the actual modules of the stack trace, those modules
# will be validated against the PDB for an exact match.
#
# Prerequisite:
# -------------
# - The DIA SDK must be installed(usually installed with VS 2022)
# - The script will take care of registering msdia140.dll with regsvr32 and
#   building the pdbhelper.dll from source.
#
# Usage:
# ------
# PS C:\> .\resolve_stacktrace.ps1 -StackTrace "
# >>     # Child-SP              Return Address         Call Site
# >>     0 00000057261FFAE0      00007FFC9AC910E5       x64+1095
# >>     1 00000057261FFB10      00007FFC9AC9115E       x64+10E5
# >>     2 00000057261FFB50      00007FFC9AC911E8       x64+115E
# >>     3 00000057261FFB90      00007FFC9AC9125F       x64+11E8
# >>     4 00000057261FFBD0      00007FF6D3557236       x64+125F
# >>     5 00000057261FFC10      00007FFCC4BDE8D7       patina_stacktrace-cf486b9b613e51dc+7236
# >>     6 00000057261FFC70      00007FFCC6B7FBCC       kernel32+2E8D7
# >>     7 00000057261FFCA0      0000000000000000       ntdll+34521
# >>
# >> " -PdbDirectory "C:\pdbs\"
#
# Output:
# # Source Path                                                           Child-SP         Return Address   Call Site
# 0 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   63] 00000057261FFAE0 00007FFC9AC910E5 x64!func1+25
# 1 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   72] 00000057261FFB10 00007FFC9AC9115E x64!func2+15
# 2 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   84] 00000057261FFB50 00007FFC9AC911E8 x64!func3+1E
# 3 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @   96] 00000057261FFB90 00007FFC9AC9125F x64!func4+28
# 4 [C:\r\patina\core\patina_stacktrace\src\x64\tests\collateral\x64.c     @  109] 00000057261FFBD0 00007FF6D3557236 x64!StartCallStack+1F
# 5 [C:\r\patina\core\patina_stacktrace\src\x64\tests\unwind_test_full.rs  @   98] 00000057261FFC10 00007FFCC4BDE8D7 patina_stacktrace-cf486b9b613e51dc!static unsigned int patina_stacktrace::x64::tests::unwind_test_full::call_stack_thread(union enum2$<winapi::ctypes::c_void> *)+56
# 6 [Failed to load PDB file (HRESULT: 0x806D0005)                      ] 00000057261FFC70 00007FFCC6B7FBCC kernel32+2E8D7
# 7 [Failed to load PDB file (HRESULT: 0x806D0005)                      ] 00000057261FFCA0 0000000000000000 ntdll+34521
#
param (
    [string]$StackTrace,  # Input text containing the stack trace information
    [string]$PdbDirectory  # Path to the directory containing PDB files
)

# Function to build the pdbhelper.dll if it doesn't exist
function AddPdbHelperType {

$PdbHelperDllPath = "$PSScriptRoot\pdbhelper.dll" -replace '\\', '\\\\'

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class PdbHelper
{
    [DllImport("$PdbHelperDllPath", CharSet = CharSet.Unicode, CallingConvention = CallingConvention.Cdecl)]
    public static extern int ResolveStackFrameSymbols(
        string PdbFilePath,
        uint Rva,
        out IntPtr FileNameOut,
        out int LineNumberOut,
        out IntPtr FunctionNameOut,
        out int DisplacementOut,
        out IntPtr ErrorMessageOut);

    [DllImport("$PdbHelperDllPath", CharSet = CharSet.Unicode, CallingConvention = CallingConvention.Cdecl)]
    public static extern int MatchModuleWithPdbFile(
        string ExeFilePath,
        string PdbFilePath,
        out bool IsMatched,
        out IntPtr ErrorMessageOut);

    public static int ResolveSymbols(
        string pdbFilePath,
        uint rva,
        out string fileName,
        out int lineNumber,
        out string functionName,
        out int displacement,
        out string errorMessage)
    {
        IntPtr fileNamePtr = IntPtr.Zero;
        int lineNumberOut = 0;
        IntPtr functionNamePtr = IntPtr.Zero;
        int displacementOut = 0;
        IntPtr errorMessagePtr = IntPtr.Zero;

        // Initialize the out parameters
        fileName = "";
        functionName = "";
        lineNumber = 0;
        displacement = 0;
        errorMessage = "";

        int hr = ResolveStackFrameSymbols(pdbFilePath,
                                          rva,
                                          out fileNamePtr,
                                          out lineNumberOut,
                                          out functionNamePtr,
                                          out displacementOut,
                                          out errorMessagePtr);
        if (hr == 0) { // S_OK
            if (fileNamePtr != IntPtr.Zero) {
                fileName = Marshal.PtrToStringBSTR(fileNamePtr);
                Marshal.FreeBSTR(fileNamePtr);
            }

            if (functionNamePtr != IntPtr.Zero) {
                functionName = Marshal.PtrToStringBSTR(functionNamePtr);
                Marshal.FreeBSTR(functionNamePtr);
            }

            lineNumber = lineNumberOut;
            displacement = displacementOut;

            return 0; // S_OK
        } else {
            if (errorMessagePtr != IntPtr.Zero) {
                errorMessage = Marshal.PtrToStringBSTR(errorMessagePtr);
                Marshal.FreeBSTR(errorMessagePtr);
            }
            return hr;
        }
    }

    public static int MatchModuleWithPdb(
        string exeFilePath,
        string pdbFilePath,
        out bool isMatched,
        out string errorMessage)
    {
        bool isMatchedOut = false;
        IntPtr errorMessagePtr = IntPtr.Zero;

        // Initialize the out parameters
        isMatched = false;
        errorMessage = "";

        int hr = MatchModuleWithPdbFile(exeFilePath,
                                        pdbFilePath,
                                        out isMatchedOut,
                                        out errorMessagePtr);
        if (hr == 0) { // S_OK
            isMatched = isMatchedOut;
            return 0; // S_OK
        } else {
            if (errorMessagePtr != IntPtr.Zero) {
                errorMessage = Marshal.PtrToStringBSTR(errorMessagePtr);
                Marshal.FreeBSTR(errorMessagePtr);
            }
            return hr;
        }
    }
}
"@ -PassThru | Out-Null
}

# Function to build the pdbhelper.dll if it doesn't exist
function BuildPdbHelperDll {
    if (-not (Get-Module -ListAvailable -Name VSSetup)) {
        # Install the VSSetup module if not already installed
        Install-Module VSSetup -Scope CurrentUser -Force -AllowClobber
    }

    # Get the latest Visual Studio installation with required components/workload
    $vsInstance = Get-VSSetupInstance | Select-VSSetupInstance -Latest -Require 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64'

    if (-not $vsInstance) {
        Write-Host "Error: No suitable Visual Studio installation found."
        return
    }

    $msdiaDllPath = "$($vsInstance.InstallationPath)\Common7\IDE\msdia140.dll"

    Write-Host "[+] Identifying Visual Studio environment"
    Write-Host "    - Instance ID: $($vsInstance.InstanceId)"
    Write-Host "    - Display Name: $($vsInstance.DisplayName)"
    Write-Host "    - Version: $($vsInstance.InstallationVersion)"
    Write-Host "    - Installation Path: $($vsInstance.InstallationPath)"
    Write-Host "    - Install Date: $($vsInstance.InstallDate)"
    Write-Host "    - MSDIA DLL Path: $msdiaDllPath"

    # Check if the script is running as an administrator
    if (-not ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
      Write-Host "Warning: This script must be run as an administrator!" -ForegroundColor Red
      exit
    }

    # Register msdia140.dll
    regsvr32 /s $msdiaDllPath
    Write-Host "[+] msdia140.dll registered successfully."

    # Check if pdbhelper.dll already exists
    if (Test-Path "$PSScriptRoot\pdbhelper.dll") {
        return
    }

    # Launch VS Developer PowerShell session
    Import-Module "$($vsInstance.InstallationPath)\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
    Enter-VsDevShell $vsInstance.InstanceId -SkipAutomaticLocation  -Arch amd64 -HostArch amd64

    # Remove existing build artifacts
    Remove-Item *.dll, *.exe, *.exp, *.lib, *.obj, *.pdb -ErrorAction SilentlyContinue

    # Compile the DLL
    Write-Host "[+] Building pdbhelper.dll."
    cl pdbhelper.cpp /nologo /W3 /LD /MT /Zi `
        -I"$($vsInstance.InstallationPath)\DIA SDK\include" `
        /link /nologo /LIBPATH:"$($vsInstance.InstallationPath)\DIA SDK\lib" `
        "ole32.lib" "oleaut32.lib" /DEBUG /INCREMENTAL:NO | Out-Null

    # Clean up extra files
    Remove-Item *.exe, *.exp, *.lib, *.obj, *.pdb -ErrorAction SilentlyContinue
    Write-Host "[+] pdbhelper.dll build completed successfully."
}

# Function to match module and PDB files if they exist
function MatchModuleWithPdbIfExists {
    param (
        [string]$Module,
        [string]$PdbDirectory
    )

    # Construct the PDB file path
    $pdbFile = Join-Path -Path $PdbDirectory -ChildPath "$Module.pdb"

    # Check if the corresponding module file exists
    $exeFile = Join-Path -Path $PdbDirectory -ChildPath "$Module.exe"
    $dllFile = Join-Path -Path $PdbDirectory -ChildPath "$Module.dll"
    $efiFile = Join-Path -Path $PdbDirectory -ChildPath "$Module.efi"

    $matched = $false
    $errorMessage = ""
    $hr = -1  # Default to an error state

    if (Test-Path $exeFile) {
        $hr = [PdbHelper]::MatchModuleWithPdb($exeFile, $pdbFile, [ref]$matched, [ref]$errorMessage)
    } elseif (Test-Path $dllFile) {
        $hr = [PdbHelper]::MatchModuleWithPdb($dllFile, $pdbFile, [ref]$matched, [ref]$errorMessage)
    } elseif (Test-Path $efiFile) {
        $hr = [PdbHelper]::MatchModuleWithPdb($efiFile, $pdbFile, [ref]$matched, [ref]$errorMessage)
    } else {
        return $true # No module file found, so we assume the given pdb is an exact match
    }

    # Process the result of MatchModuleWithPdb
    if ($hr -eq 0 -and $matched) {
        return $true
    } else {
        return $false
    }
}

# Check if the PdbDirectory parameter is provided
if (-not $PdbDirectory) {
    Write-Host "Please provide the path to the directory containing PDB files."
    return
}

# Check if the StackTrace parameter is provided
if (-not $StackTrace) {
    Write-Host "Please provide the stack trace information."
    return
}

$CurrentDirectory = Get-Location
Write-Host "Current Directory: $CurrentDirectory"
Set-Location -Path $PSScriptRoot

# Build the PdbHelper type
BuildPdbHelperDll | Out-Null
AddPdbHelperType | Out-Null

# Split the stack trace into lines
$lines = $StackTrace -split "`n"
foreach ($line in $lines) {
    # Trim leading and trailing whitespace from the line
    $line = $line.Trim()

    # Skip any empty lines
    if ($line -match "^\s*$") {
        continue
    }

    # Skip the header line, but allow for prefixes like "INFO -"
    if ($line -match "^\s*[^#]*#") {
        Write-Output " # Source Path                                                           Child-SP         Return Address   Call Site"
        continue
    }

    # Remove any prefix before the frame number (e.g., "INFO -    ")
    # This regex matches: optional prefix, then frame number, then the rest
    if ($line -match "^[^\d]*?(\d+)\s+(.*)$") {
        $frameNumber = $matches[1]
        $restOfLine = $matches[2]
        $columns = @($frameNumber) + ($restOfLine -split "\s+")
    } else {
        # If it doesn't match, skip the line
        continue
    }

    # Now $columns[0] is the frame number, $columns[1] is Child-SP, $columns[2] is Return Address, $columns[3] is Call Site
    if ($columns.Count -lt 4) {
        continue
    }

    $callSite = $columns[3]

    # Transform the Call Site
    if ($callSite -match "\+") {
        # Split the Call Site into module and RVA
        $module = $callSite -replace "\+.*", ""  # Extract everything before the +
        $rva = $callSite -replace ".*\+", ""  # Extract everything after the +

        # Construct the PDB file path
        $pdbFile = Join-Path -Path $PdbDirectory -ChildPath "$module.pdb"

        # Match the module and PDB files if they exist
        if (-not (MatchModuleWithPdbIfExists -Module $module -PdbDirectory $PdbDirectory)) {
            Write-Output ("{0,2} [{1,-67}] {2} {3} {4}" -f $columns[0], "Failed to match PDB file with the module", $columns[1], $columns[2], $columns[3], $columns[4]);
            continue
        }

        # Ensure RVA is a valid number
        if ($rva -match "^[0-9A-Fa-f]+$") {
            $rvaDecimal = [Convert]::ToUInt32($rva, 16)  # Convert hex string to uint

            # Declare variables to hold the out parameters
            $fileName = ""
            $lineNumber = 0
            $functionName = ""
            $displacement = 0
            $errorMessage = ""

            $hr = [PdbHelper]::ResolveSymbols($pdbFile, $rvaDecimal, [ref]$fileName, [ref]$lineNumber, [ref]$functionName, [ref]$displacement, [ref]$errorMessage)
            if ($hr -eq 0) {
                Write-Output ("{0,2} [{1,-60} @ {2,4}] {3} {4} {5}!{6}+{7:X}" -f $columns[0], $fileName, $lineNumber, $columns[1], $columns[2],  $module, $functionName, $displacement);
            } else {
                Write-Output ("{0,2} [{1,-67}] {2} {3} {4}" -f $columns[0], $errorMessage, $columns[1], $columns[2], $columns[3], $columns[4]);
            }
        }
    }
}

Set-Location -Path $CurrentDirectory
