# Starts a CI run on the Windows machine in the desktop session of the account, and removes the
# scheduled task of the run when the run has ended (ADR 0905). tools/ci/ci.sh copies this file
# into the folder of the run with run-jobs.sh and start.sh, and runs it over SSH:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File start-run.ps1 -Directory C:/junto-ci -Run <run>
#
# The SSH server starts its commands in session 0, whose window station is not interactive,
# and the window code of the client opens no window there; the start check of the Windows
# package needs a window. A scheduled task with the logon type Interactive runs in the
# desktop session of the account while the account is logged on, and needs no stored
# password. Its program is start.sh of the run in the Bash of the setup, under a console host
# without a window. The task has the rights of the account without elevation, so the run
# cannot remove its own task: Windows requires elevation for that. This script, which runs
# elevated in the SSH session, therefore starts a second copy of itself with -Remove -After
# <process of the run>, through the process creation of the management service, so that the
# copy is not ended with the SSH session. The copy waits until the process of the run has
# ended and removes the task. A start also removes the tasks of earlier runs that are not
# running.
#
# Exit codes: 0 when the run has started (or its task is removed); 2 when the account is not
# logged on to the desktop; 1 for another failure.
param(
    [Parameter(Mandatory = $true)][string]$Directory,
    [Parameter(Mandatory = $true)][string]$Run,
    [switch]$Remove,
    [int]$After = 0
)

$ErrorActionPreference = 'Stop'
$TaskPath = '\junto-ci\'
# The time from the start of the task until start.sh has written "started".
$StartSeconds = 60

if ($Run -notmatch '^[A-Za-z0-9-]+$') { Write-Output "not a run name: $Run"; exit 1 }
$here = Join-Path (Join-Path $Directory 'runs') $Run

if ($Remove) {
    if ($After -gt 0) { Wait-Process -Id $After -ErrorAction SilentlyContinue }
    try {
        Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Confirm:$false
        Add-Content -Path (Join-Path $here 'task.log') -Value "the task of the run is removed"
    } catch {
        Add-Content -Path (Join-Path $here 'task.log') -Value "the task of the run was not removed: $_"
        exit 1
    }
    exit 0
}

$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
# The desktop of the account: its Explorer in a session other than session 0.
$desktop = Get-Process -Name explorer -IncludeUserName -ErrorAction SilentlyContinue |
    Where-Object { $_.UserName -eq $user -and $_.SessionId -gt 0 }
if (-not $desktop) {
    Write-Output "the run $Run cannot start: $user is not logged on to the desktop of this machine, which the Windows job needs (ADR 0905)"
    exit 2
}

# Tasks of earlier runs that ended without removing their task.
Get-ScheduledTask -TaskPath $TaskPath -ErrorAction SilentlyContinue |
    Where-Object { $_.State -ne 'Running' -and $_.TaskName -ne $Run } |
    Unregister-ScheduledTask -Confirm:$false

$bash = Join-Path $Directory 'tools\git\bin\bash.exe'
$console = Join-Path $env:SystemRoot 'System32\conhost.exe'
$arguments = "--headless $bash $here\start.sh"
$action = New-ScheduledTaskAction -Execute $console -Argument $arguments -WorkingDirectory $here
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
try {
    Register-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Action $action -Principal $principal `
        -Settings $settings | Out-Null
    Start-ScheduledTask -TaskPath $TaskPath -TaskName $Run
} catch {
    Write-Output "the run $Run did not start: $_"
    Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Confirm:$false -ErrorAction SilentlyContinue
    exit 1
}
$started = Join-Path $here 'started'
$deadline = (Get-Date).AddSeconds($StartSeconds)
while (-not (Test-Path $started)) {
    if ((Get-Date) -gt $deadline) {
        $info = Get-ScheduledTaskInfo -TaskPath $TaskPath -TaskName $Run
        Write-Output "the run $Run did not start within $StartSeconds s; the task result is $($info.LastTaskResult)"
        Stop-ScheduledTask -TaskPath $TaskPath -TaskName $Run -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Confirm:$false -ErrorAction SilentlyContinue
        exit 1
    }
    Start-Sleep -Milliseconds 500
}

$process = Get-CimInstance Win32_Process -Filter "Name = 'conhost.exe'" |
    Where-Object { $_.CommandLine -like "*$arguments" } | Select-Object -First 1
$remover = "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $here\start-run.ps1 " +
    "-Directory $Directory -Run $Run -Remove -After $(if ($process) { $process.ProcessId } else { 0 })"
$created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
    CommandLine = $remover; CurrentDirectory = $here
}
if ($created.ReturnValue -ne 0) {
    # The run goes on; the next start removes its task.
    Write-Output "the run $Run started, but the removal of its task did not start ($($created.ReturnValue))"
}
exit 0
