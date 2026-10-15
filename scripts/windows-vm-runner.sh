#!/bin/sh
# cargo's test runner for x86_64-pc-windows-gnu that runs each test binary
# on a REAL Windows machine over SSH, instead of under Wine:
#
#   export IDEALYST_WINDOWS_REMOTE=you@192.168.65.2
#   export IDEALYST_WINDOWS_REMOTE_KEY=~/.ssh/idealyst_vm      # optional
#   CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=scripts/windows-vm-runner.sh \
#     cargo test -p backend-windows --target x86_64-pc-windows-gnu
#
# Same machine setup as `idealyst run windows` on a remote (docs/windows.md).
# The binary runs in the logged-in user's DESKTOP session (through a one-off
# scheduled task) so window, DirectComposition and focus tests behave as they
# do for an app; IDEALYST_WINDOWS_REMOTE_DIRECT=1 runs it in the SSH session
# instead (no desktop). Output is printed when the binary exits, and its exit
# code is this script's. IDEALYST_WINDOWS_REMOTE_TIMEOUT (seconds, default
# 900) bounds a hung run.
set -eu

dest="${IDEALYST_WINDOWS_REMOTE:?set IDEALYST_WINDOWS_REMOTE=user@host (see docs/windows.md)}"
key="${IDEALYST_WINDOWS_REMOTE_KEY:-}"
timeout="${IDEALYST_WINDOWS_REMOTE_TIMEOUT:-900}"
exe="$1"
shift

# "$@" is now the test binary's own arguments.
ssh_opts="-o BatchMode=yes -o ConnectTimeout=10"
if [ -n "$key" ]; then
    ssh_opts="$ssh_opts -i $key -o IdentitiesOnly=yes"
fi

name="$(basename "$exe")"
run_id="$$-$(date +%s)"
remote_dir="C:\\idealyst\\tests\\$run_id"

# One PowerShell script, sent as -EncodedCommand (base64 UTF-16LE) so no
# quoting survives ssh, cmd and PowerShell in between.
ps() {
    # shellcheck disable=SC2086
    ssh $ssh_opts "$dest" "powershell -NoProfile -NonInteractive -EncodedCommand $(printf '%s' "$1" | iconv -t UTF-16LE | base64 | tr -d '\n')"
}

ps "\$ProgressPreference='SilentlyContinue'; New-Item -ItemType Directory -Force -Path '$remote_dir' | Out-Null"
# shellcheck disable=SC2086
scp -q $ssh_opts "$exe" "$dest:$(printf '%s' "$remote_dir" | tr '\\' '/')/$name"

# The test binary's args, quoted for cmd.exe.
args=""
for a in "$@"; do
    args="$args \"$(printf '%s' "$a" | sed 's/"/""/g')\""
done

if [ "${IDEALYST_WINDOWS_REMOTE_DIRECT:-}" = "1" ]; then
    set +e
    ps "\$ProgressPreference='SilentlyContinue'; Set-Location '$remote_dir'; & cmd.exe /c '\"$name\"$args 2>&1'; exit \$LASTEXITCODE"
    code=$?
    set -e
else
    set +e
    ps "
\$ProgressPreference = 'SilentlyContinue'
\$dir = '$remote_dir'
Set-Content -Encoding ascii (Join-Path \$dir 'run.cmd') @'
@echo off
cd /d \"$remote_dir\"
\"$name\"$args > out.txt 2>&1
echo %errorlevel% > code.txt
'@
\$task = 'idealyst-test-$run_id'
\$action = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument ('/c \"' + (Join-Path \$dir 'run.cmd') + '\"')
\$principal = New-ScheduledTaskPrincipal -UserId \$env:USERNAME -LogonType Interactive
Register-ScheduledTask -TaskName \$task -Action \$action -Principal \$principal -Force | Out-Null
Start-ScheduledTask -TaskName \$task
\$deadline = (Get-Date).AddSeconds($timeout)
\$codeFile = Join-Path \$dir 'code.txt'
while (-not (Test-Path \$codeFile) -and (Get-Date) -lt \$deadline) { Start-Sleep -Milliseconds 300 }
Unregister-ScheduledTask -TaskName \$task -Confirm:\$false -ErrorAction SilentlyContinue
if (Test-Path (Join-Path \$dir 'out.txt')) { Get-Content (Join-Path \$dir 'out.txt') }
if (-not (Test-Path \$codeFile)) {
    'windows-vm-runner: no result within $timeout s (is a user logged in to the desktop?)'
    Get-Process -Name '$(basename "$name" .exe)' -ErrorAction SilentlyContinue | Stop-Process -Force
    exit 124
}
Start-Sleep -Milliseconds 200
exit [int]((Get-Content \$codeFile | Select-Object -First 1).Trim())
"
    code=$?
    set -e
fi

ps "Remove-Item -Recurse -Force -ErrorAction SilentlyContinue '$remote_dir'" >/dev/null 2>&1 || true
exit "$code"
