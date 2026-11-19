@echo off
rem Test runner for the PC unit tests (see .cargo/config.toml). Windows 11 has its own, older
rem onnxruntime.dll in System32, which is found before the one sherpa-onnx needs unless the
rem DLLs sit next to the test program: copy them there, then run it.
setlocal
set "DIR=%~dp1"
if exist "%DIR%..\sherpa-onnx-c-api.dll" copy /y "%DIR%..\*.dll" "%DIR%" >nul
%*
