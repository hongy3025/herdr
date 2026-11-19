@echo off
rem Launch this checkout's DEBUG herdr client and server, isolated from any
rem installed release build.
rem
rem Isolation:
rem   * debug builds already use %APPDATA%\herdr-dev and %LOCALAPPDATA%\herdr-dev
rem   * HERDR_SOCKET_PATH points at herdr-dev\run, so the server and the client
rem     socket (-client.sock) never touch the release sockets
rem   * inherited session/config/agent variables from an outer Herdr pane are
rem     cleared so this client cannot attach to the release server
setlocal
cd /d "%~dp0"

if /i "%~1"=="build" (
  shift
  if not defined ZIG set "ZIG=%LOCALAPPDATA%\Microsoft\WinGet\Packages\zig.zig_Microsoft.Winget.Source_8wekyb3d8bbwe\zig-x86_64-windows-0.16.0\zig.exe"
  if not exist "%ZIG%" set "ZIG="
  echo [herdr-debug] building debug binary...
  cargo build --locked
  if errorlevel 1 exit /b 1
)

if not exist "target\debug\herdr.exe" (
  echo [herdr-debug] target\debug\herdr.exe is missing.
  echo [herdr-debug] run: herdr-debug.cmd build
  exit /b 1
)

set "HERDR_SESSION="
set "HERDR_CONFIG_PATH="
set "HERDR_ENV="
set "HERDR_PANE_ID="
set "HERDR_TAB_ID="
set "HERDR_WORKSPACE_ID="
set "HERDR_STARTUP_CWD="
set "HERDR_REMOTE_BINARY="
set "HERDR_REATTACH_COMMAND="
set "HERDR_REMOTE_KEYBINDINGS="
set "HERDR_REMOTE_PREFERENCES_IDENTITY="
set "HERDR_CLIENT_SOCKET_PATH="

if not defined HERDR_DEBUG_SOCKET_DIR set "HERDR_DEBUG_SOCKET_DIR=%LOCALAPPDATA%\herdr-dev\run"
if not exist "%HERDR_DEBUG_SOCKET_DIR%" mkdir "%HERDR_DEBUG_SOCKET_DIR%" >nul 2>&1
set "HERDR_SOCKET_PATH=%HERDR_DEBUG_SOCKET_DIR%\herdr.sock"

"target\debug\herdr.exe" %*
exit /b %ERRORLEVEL%