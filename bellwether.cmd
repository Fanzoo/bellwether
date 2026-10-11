@echo off
rem Windows twin of the POSIX `bellwether` shim. Runs the brain CLI from the agent
rem directory, passes the caller's directory as BELLWETHER_CWD, and passes the exit
rem code through.
setlocal
where node >nul 2>nul
if errorlevel 1 (
  echo bellwether: node is not installed or not on PATH. Install Node 22.9 or newer: https://nodejs.org 1>&2
  exit /b 3
)
node -e "const [a,b]=process.versions.node.split('.').map(Number);process.exit(a>22||(a===22&&b>=9)?0:1)"
if errorlevel 1 (
  echo bellwether: Node 22.9 or newer is required. https://nodejs.org 1>&2
  exit /b 3
)
set "BELLWETHER_CWD=%CD%"
pushd "%~dp0"
node brain/cli.mjs %*
set BELLWETHER_EXIT=%ERRORLEVEL%
popd
exit /b %BELLWETHER_EXIT%
