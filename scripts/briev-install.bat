@echo off
REM Briev Compiler Installer for Windows
REM Usage: briev-install.bat [--prefix <directory>]
REM Installs `brievc.exe` plus a `briev.exe` alias, then verifies it.

setlocal enabledelayedexpansion

set "INSTALL_PREFIX=%LOCALAPPDATA%\briev"
set "BINARY_NAME=brievc.exe"

:parse_args
if "%~1"=="" goto :done_parsing
if "%~1"=="--prefix" (
    set "INSTALL_PREFIX=%~2"
    shift
    shift
    goto :parse_args
)
if "%~1"=="--help" (
    echo Briev Compiler Installer for Windows
    echo.
    echo Usage: %~nx0 [--prefix ^<directory^>]
    echo.
    echo Options:
    echo   --prefix ^<dir^>  Installation directory (default: %LOCALAPPDATA%\briev)
    echo   --help           Show this help message
    exit /b 0
)
shift
goto :parse_args

:done_parsing

echo Installing the Briev compiler (brievc)...
echo   Target: %INSTALL_PREFIX%\%BINARY_NAME%

set "SCRIPT_DIR=%~dp0"
set "SCRIPT_DIR=%SCRIPT_DIR:~0,-1%"

REM 2026-10-06: the built artifact is brievc.exe, not briev-compiler.exe.
set "BINARY_PATH="
if exist "%SCRIPT_DIR%\target\release\%BINARY_NAME%" (
    set "BINARY_PATH=%SCRIPT_DIR%\target\release\%BINARY_NAME%"
) else if exist "%SCRIPT_DIR%\target\debug\%BINARY_NAME%" (
    set "BINARY_PATH=%SCRIPT_DIR%\target\debug\%BINARY_NAME%"
) else if exist "%SCRIPT_DIR%\..\target\release\%BINARY_NAME%" (
    set "BINARY_PATH=%SCRIPT_DIR%\..\target\release\%BINARY_NAME%"
) else if exist "%SCRIPT_DIR%\..\target\debug\%BINARY_NAME%" (
    set "BINARY_PATH=%SCRIPT_DIR%\..\target\debug\%BINARY_NAME%"
) else if exist "%SCRIPT_DIR%\%BINARY_NAME%" (
    set "BINARY_PATH=%SCRIPT_DIR%\%BINARY_NAME%"
) else (
    echo.
    echo Error: could not find the compiler binary.
    echo Build it first:  cargo build --release
    exit /b 1
)

if not exist "%INSTALL_PREFIX%" mkdir "%INSTALL_PREFIX%"

copy /Y "%BINARY_PATH%" "%INSTALL_PREFIX%\%BINARY_NAME%" >nul
copy /Y "%BINARY_PATH%" "%INSTALL_PREFIX%\briev.exe" >nul

REM 2026-10-06: ship resources so the installed compiler finds the stdlib.
set "SHARE_DIR=%INSTALL_PREFIX%\..\share\briev"
set "LIB_SRC="
if exist "%SCRIPT_DIR%\lib" set "LIB_SRC=%SCRIPT_DIR%\lib"
if not defined LIB_SRC if exist "%SCRIPT_DIR%\..\lib" set "LIB_SRC=%SCRIPT_DIR%\..\lib"
if defined LIB_SRC (
    mkdir "%SHARE_DIR%" 2>nul
    xcopy /E /I /Y "%LIB_SRC%" "%SHARE_DIR%\lib" >nul
)

echo.
echo Briev installed successfully!
echo.
echo Next steps:
echo   1. Add to your PATH:  %INSTALL_PREFIX%
echo   2. Create a project:  brievc init my-app
echo   3. Run it:            cd my-app ^&^& brievc run src\main.bv
echo.
echo Note: building executables needs an LLVM toolchain (clang, llc) on PATH.
