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
# copy is not ended with the SSH session. The copy keeps the machine from sleeping while the
# process of the run runs, and then removes the task. A start also removes the tasks of earlier
# runs that are not running.
#
# The start is refused when the account has no desktop session, or when its session is
# disconnected or its screen is locked: the start check of the package needs a window that
# draws its first frame on the desktop. A start that is refused or fails writes its message to
# runner.log and its exit code to "finished" of the run, and removes the bundle of the commit.
#
# The file "process" of the run holds the number and the start time of the console host of the
# run. "start-run.ps1 -Ended" ends with exit code 0 when that process no longer runs, so that
# tools/ci/ci.sh can report a run whose processes ended without writing "finished" (a logoff or
# a restart of Windows ends them).
#
# Other modes: "-Check [-User <account>]" prints whether a run of the account can start now.
# "-FunctionsOnly" only defines the functions, for the tests of tools/windows.
#
# Exit codes: 0 when the run has started (or its task is removed); 2 when the account has no
# desktop session that is connected and unlocked; 1 for another failure.
param(
    [Parameter(Mandatory = $true)][string]$Directory,
    [Parameter(Mandatory = $true)][string]$Run,
    [switch]$Remove,
    [int]$After = 0,
    [switch]$Ended,
    [switch]$Check,
    [string]$User = '',
    [switch]$FunctionsOnly
)

$ErrorActionPreference = 'Stop'
$TaskPath = '\junto-ci\'
# The time from the start of the task until start.sh has written "started".
$StartSeconds = 60

if (-not ('JuntoCi.Native' -as [type])) {
    Add-Type -Namespace JuntoCi -Name Native -MemberDefinition @'
[DllImport("wtsapi32.dll", SetLastError = true)]
public static extern bool WTSEnumerateSessionsW(IntPtr server, int reserved, int version, out IntPtr sessions, out int count);
[DllImport("wtsapi32.dll", SetLastError = true)]
public static extern bool WTSQuerySessionInformationW(IntPtr server, int session, int infoClass, out IntPtr buffer, out int bytes);
[DllImport("wtsapi32.dll")]
public static extern void WTSFreeMemory(IntPtr memory);
[DllImport("kernel32.dll")]
public static extern uint SetThreadExecutionState(uint flags);
'@
}

# The sessions of the machine other than session 0, each with its account, from
# WTSQuerySessionInformation with WTSSessionInfoEx: Connected when its connect state is
# WTSActive, Locked when its flags are WTS_SESSIONSTATE_LOCK. These calls need no elevation.
function Get-Session {
    $marshal = [Runtime.InteropServices.Marshal]
    $list = [IntPtr]::Zero
    $count = 0
    if (-not [JuntoCi.Native]::WTSEnumerateSessionsW([IntPtr]::Zero, 0, 1, [ref]$list, [ref]$count)) {
        throw "the sessions cannot be read: $([ComponentModel.Win32Exception]::new().Message)"
    }
    try {
        # WTS_SESSION_INFOW: SessionId, pWinStationName, State; 24 bytes on x64.
        $ids = for ($index = 0; $index -lt $count; $index++) { $marshal::ReadInt32($list, 24 * $index) }
    } finally {
        [JuntoCi.Native]::WTSFreeMemory($list)
    }
    foreach ($id in @($ids | Where-Object { $_ -gt 0 })) {
        $buffer = [IntPtr]::Zero
        $bytes = 0
        if (-not [JuntoCi.Native]::WTSQuerySessionInformationW([IntPtr]::Zero, $id, 25, [ref]$buffer, [ref]$bytes)) {
            continue  # the session ended after the enumeration
        }
        try {
            # WTSINFOEXW: the level, then WTSINFOEX_LEVEL1_W at offset 8 (it holds 8-byte
            # fields): SessionId, SessionState, SessionFlags, WinStationName[33],
            # UserName[21] at offset 86, DomainName[18] at offset 128.
            if ($marshal::ReadInt32($buffer, 0) -ne 1 -or $marshal::ReadInt32($buffer, 8) -ne $id) {
                throw "the state of session $id has an unknown form"
            }
            $name = $marshal::PtrToStringUni([IntPtr]::Add($buffer, 86))
            if (-not $name) { continue }
            [pscustomobject]@{
                Id = $id
                Account = "$($marshal::PtrToStringUni([IntPtr]::Add($buffer, 128)))\$name"
                Connected = ($marshal::ReadInt32($buffer, 12) -eq 0)
                Locked = ($marshal::ReadInt32($buffer, 16) -eq 0)
            }
        } finally {
            [JuntoCi.Native]::WTSFreeMemory($buffer)
        }
    }
}

# The desktop sessions of the account.
function Get-DesktopSession([string]$Account) {
    Get-Session | Where-Object { $_.Account -eq $Account }
}

# The reason why a run of the account cannot start in these sessions, or $null when one of them
# is connected and unlocked.
function Get-DesktopRefusal([string]$Account, [object[]]$Sessions) {
    $prefix = "the run $Run cannot start:"
    $usable = @($Sessions | Where-Object { $_.Connected -and -not $_.Locked })
    if ($usable.Count -gt 0) { return $null }
    if (@($Sessions).Count -eq 0) {
        return "$prefix $Account is not logged on to the desktop of this machine, which the Windows job needs (ADR 0905)"
    }
    if (@($Sessions | Where-Object { $_.Connected }).Count -eq 0) {
        return "$prefix the desktop session of $Account is disconnected; the Windows job needs a connected session (ADR 0905)"
    }
    return "$prefix the screen of $Account is locked; the Windows job needs an unlocked desktop (ADR 0905)"
}

if ($Run -notmatch '^[A-Za-z0-9-]+$') { Write-Output "not a run name: $Run"; exit 1 }
$here = Join-Path (Join-Path $Directory 'runs') $Run

# Ends a start that did not start the run: the message goes to the output and to runner.log, the
# exit code to "finished", and the bundle is removed.
function Stop-Start([int]$Code, [string]$Message) {
    Write-Output $Message
    Set-Content -Path (Join-Path $here 'runner.log') -Value $Message
    Set-Content -NoNewline -Path (Join-Path $here 'finished') -Value "$Code`n"
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $here 'commit.bundle')
    exit $Code
}

if ($FunctionsOnly) { return }

if ($Check) {
    $account = if ($User) { $User } else { [Security.Principal.WindowsIdentity]::GetCurrent().Name }
    $refusal = Get-DesktopRefusal $account @(Get-DesktopSession $account)
    if ($refusal) { Write-Output $refusal; exit 2 }
    Write-Output "a run of $account can start"
    exit 0
}

if ($Ended) {
    $file = Join-Path $here 'process'
    if (-not (Test-Path $file)) { exit 1 }
    $number, $start = (Get-Content -Raw $file).Trim() -split ' '
    $process = Get-Process -Id ([int]$number) -ErrorAction SilentlyContinue
    if ($process -and $process.StartTime.ToFileTimeUtc() -eq [long]$start) { exit 1 }
    Write-Output "ended"
    exit 0
}

if ($Remove) {
    if ($After -gt 0) {
        # ES_CONTINUOUS | ES_SYSTEM_REQUIRED: Windows does not sleep while this thread waits.
        [JuntoCi.Native]::SetThreadExecutionState(0x80000001) | Out-Null
        Wait-Process -Id $After -ErrorAction SilentlyContinue
    }
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
try {
    $refusal = Get-DesktopRefusal $user @(Get-DesktopSession $user)
} catch {
    Stop-Start 1 "the run $Run did not start: $_"
}
if ($refusal) { Stop-Start 2 $refusal }

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
    Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Confirm:$false -ErrorAction SilentlyContinue
    Stop-Start 1 "the run $Run did not start: $_"
}
$started = Join-Path $here 'started'
$deadline = (Get-Date).AddSeconds($StartSeconds)
while (-not (Test-Path $started)) {
    if ((Get-Date) -gt $deadline) {
        $info = Get-ScheduledTaskInfo -TaskPath $TaskPath -TaskName $Run
        Stop-ScheduledTask -TaskPath $TaskPath -TaskName $Run -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Confirm:$false -ErrorAction SilentlyContinue
        Stop-Start 1 "the run $Run did not start within $StartSeconds s; the task result is $($info.LastTaskResult)"
    }
    Start-Sleep -Milliseconds 500
}

$process = Get-CimInstance Win32_Process -Filter "Name = 'conhost.exe'" |
    Where-Object { $_.CommandLine -like "*$arguments" } | Select-Object -First 1
$running = if ($process) { Get-Process -Id $process.ProcessId -ErrorAction SilentlyContinue }
if ($running) {
    Set-Content -NoNewline -Path (Join-Path $here 'process') `
        -Value "$($running.Id) $($running.StartTime.ToFileTimeUtc())`n"
} else {
    # Without the process the removal cannot wait for the run: the task is removed now, which
    # does not end the run.
    Add-Content -Path (Join-Path $here 'task.log') -Value "the process of the run was not found after its start"
}
$remover = "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $here\start-run.ps1 " +
    "-Directory $Directory -Run $Run -Remove -After $(if ($running) { $running.Id } else { 0 })"
$created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
    CommandLine = $remover; CurrentDirectory = $here
}
if ($created.ReturnValue -ne 0) {
    # The run goes on; the next start removes its task.
    Write-Output "the run $Run started, but the removal of its task did not start ($($created.ReturnValue))"
}
exit 0
