@echo off
rem Deprecated: the command is `bellwether` now. Says so on stderr and forwards
rem everything to bellwether.cmd, passing its exit code through.
echo pasture: this command is now `bellwether` (same arguments); `pasture` will be removed in a later release. 1>&2
call "%~dp0bellwether.cmd" %*
exit /b %ERRORLEVEL%
