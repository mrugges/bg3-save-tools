@echo off

:: Skip if already initialized (avoid re-running vcvarsall on repeated calls)
if defined VSCMD_VER goto :run

:: Use vswhere to find VS installation, then vcvarsall to set up the environment
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" (
    echo ERROR: vswhere.exe not found. Install Visual Studio Build Tools.
    echo https://visualstudio.microsoft.com/downloads/
    exit /b 1
)

for /f "delims=" %%I in ('"%VSWHERE%" -latest -products * -property installationPath') do set "VSINSTALL=%%I"
if not defined VSINSTALL (
    echo ERROR: No Visual Studio installation found.
    exit /b 1
)

call "%VSINSTALL%\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul 2>&1
if errorlevel 1 (
    echo ERROR: vcvarsall.bat failed.
    exit /b 1
)

:: Ensure MSVC link.exe is found before Git's /usr/bin/link.exe
:: vcvarsall appends to PATH; we need MSVC's cl/link dir first.
for /f "delims=" %%C in ('where cl.exe') do set "MSVC_BIN=%%~dpC"
set "PATH=%MSVC_BIN%;%PATH%"

:: Force CMake to use NMake (avoids VS generator issues with Build Tools-only installs)
set "CMAKE_GENERATOR=NMake Makefiles"

:run
cd /d "%~dp0"
cargo %*
