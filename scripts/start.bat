@echo off
:: Jiezi Cloud — Server launcher
::
:: Usage:
::   scripts\start.bat                  # debug + SQLite (default)
::   scripts\start.bat -r               # release build
::   scripts\start.bat -c               # clean data/ before starting
::   scripts\start.bat db=postgres      # debug + PostgreSQL
::   scripts\start.bat db=mysql         # debug + MySQL
::   scripts\start.bat -r db=postgres   # release + PostgreSQL
::   scripts\start.bat -c -r db=sqlite  # clean + release + SQLite

setlocal EnableDelayedExpansion

:: Resolve workspace root
for %%I in ("%~dp0..") do cd /d "%%~fI"

:: ── Parse arguments ───────────────────────────────────────────────────────────
set "RELEASE_FLAG="
set "DB_FEATURE=db-sqlite"
set "CLEAN_DATA="

:parse
if "%~1"=="" goto :build
set "ARG=%~1"
if /i "%~1"=="-r" ( set "RELEASE_FLAG=--release" & shift & goto :parse )
if /i "%~1"=="-c" ( set "CLEAN_DATA=1"            & shift & goto :parse )
if /i "%ARG:~0,3%"=="db=" (
    set "DB_VALUE=%ARG:~3%"
    if /i "!DB_VALUE!"=="sqlite"   set "DB_FEATURE=db-sqlite"
    if /i "!DB_VALUE!"=="postgres" set "DB_FEATURE=db-postgres"
    if /i "!DB_VALUE!"=="mysql"    set "DB_FEATURE=db-mysql"
    shift & goto :parse
)
echo Unknown argument: %~1 & exit /b 1

:build
:: Clean data directory if requested
if defined CLEAN_DATA (
    if exist "data" rmdir /s /q data
)

:: Stop any running instance to release the exe lock
tasklist /fi "imagename eq jiezi-cloud-server.exe" 2>nul | find /i "jiezi-cloud-server.exe" >nul
if not errorlevel 1 (
    taskkill /f /im jiezi-cloud-server.exe >nul 2>&1
    timeout /t 1 /nobreak >nul
)

if not exist "data" mkdir data

cargo run --bin jiezi-cloud-server --no-default-features --features %DB_FEATURE% %RELEASE_FLAG%

endlocal
exit /b %ERRORLEVEL%
