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
# runs that have ended; the task of a start in progress stays.
#
# The start is refused when the account has no desktop session, or when its session is
# disconnected or its screen is locked: the start check of the package needs a window that
# draws its first frame on the desktop. A start that is refused or fails before the run has
# written "started" writes its message to runner.log and its exit code to "finished" of the run,
# and removes the bundle of the commit. A failure after "started" is written to task.log; the
# run goes on.
#
# The file "process" of the run holds the number and the creation time of the console host of
# the run. "start-run.ps1 -Ended" ends with exit code 0 when the run no longer runs, so that
# tools/ci/ci.sh can report a run whose processes ended without writing "finished" (a logoff or
# a restart of Windows ends them); see Test-RunEnded.
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
[DllImport("kernel32.dll", SetLastError = true)]
static extern IntPtr OpenProcess(uint access, bool inherit, int id);
[DllImport("kernel32.dll", SetLastError = true)]
static extern bool GetProcessTimes(IntPtr process, out long creation, out long exit, out long kernel, out long user);
[DllImport("kernel32.dll")]
static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
[DllImport("kernel32.dll")]
static extern bool CloseHandle(IntPtr handle);
// The creation time of a running process as Windows stores it (UTC, in 100 ns units since
// 1601), or 0 when no process of that number runs.
public static long CreationTime(int id) {
    IntPtr process = OpenProcess(0x00101000, false, id); // SYNCHRONIZE, PROCESS_QUERY_LIMITED_INFORMATION
    if (process == IntPtr.Zero) return 0;
    try {
        long creation, exit, kernel, user;
        if (WaitForSingleObject(process, 0) == 0 || !GetProcessTimes(process, out creation, out exit, out kernel, out user)) return 0;
        return creation;
    } finally {
        CloseHandle(process);
    }
}
'@
}

# The connect state and the lock state of a session from SessionState and SessionFlags of
# WTSINFOEX_LEVEL1_W: Connected when the state is WTSActive (0); Lock "unlocked" when the flags
# are WTS_SESSIONSTATE_UNLOCK (1), "locked" for WTS_SESSIONSTATE_LOCK (0), and "unknown" for
# WTS_SESSIONSTATE_UNKNOWN (0xFFFFFFFF) and any other value.
function Get-SessionState([int]$State, [int]$Flags) {
    [pscustomobject]@{
        Connected = ($State -eq 0)
        Lock = switch ($Flags) { 0 { 'locked' } 1 { 'unlocked' } default { 'unknown' } }
    }
}

# The sessions of the machine other than session 0, each with its account and its state, from
# WTSQuerySessionInformation with WTSSessionInfoEx. These calls need no elevation.
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
            $state = Get-SessionState ($marshal::ReadInt32($buffer, 12)) ($marshal::ReadInt32($buffer, 16))
            [pscustomobject]@{
                Id = $id
                Account = "$($marshal::PtrToStringUni([IntPtr]::Add($buffer, 128)))\$name"
                Connected = $state.Connected
                Lock = $state.Lock
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
    $connected = @($Sessions | Where-Object { $_.Connected })
    if (@($connected | Where-Object { $_.Lock -eq 'unlocked' }).Count -gt 0) { return $null }
    if (@($Sessions).Count -eq 0) {
        return "$prefix $Account is not logged on to the desktop of this machine, which the Windows job needs (ADR 0905)"
    }
    if ($connected.Count -eq 0) {
        return "$prefix the desktop session of $Account is disconnected; the Windows job needs a connected session (ADR 0905)"
    }
    if (@($connected | Where-Object { $_.Lock -eq 'locked' }).Count -gt 0) {
        return "$prefix the screen of $Account is locked; the Windows job needs an unlocked desktop (ADR 0905)"
    }
    return "$prefix Windows does not state whether the screen of $Account is locked; the Windows job needs an unlocked desktop (ADR 0905)"
}

# The scheduled task of a run, or $null.
function Get-RunTask([string]$Name) {
    Get-ScheduledTask -TaskPath $TaskPath -TaskName $Name -ErrorAction SilentlyContinue
}

# The processes of this script that start the run (the command of tools/ci/ci.sh).
function Get-RunStart([string]$Name) {
    Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" |
        Where-Object { $_.CommandLine -like "*start-run.ps1 -Directory * -Run $Name" }
}

# Whether the run of a folder no longer runs. With the file "process": when the recorded
# console host no longer runs (no process of that number with that creation time). Without it
# (the host was not found, or the start ended before it wrote the file): when no start of the run
# runs and its task does not run. The start is read first: it runs until the task runs.
function Test-RunEnded([string]$Folder, [string]$Name) {
    $file = Join-Path $Folder 'process'
    if (Test-Path $file) {
        $number, $created = (Get-Content -Raw $file).Trim() -split ' '
        return [JuntoCi.Native]::CreationTime([int]$number) -ne [long]$created
    }
    if (Get-RunStart $Name) { return $false }
    $task = Get-RunTask $Name
    return -not ($task -and $task.State -eq 'Running')
}

# Removes the tasks of earlier runs that have ended: the folder of the run no longer exists, or
# it has "finished", or Test-RunEnded. The task of a start in progress stays. The removal copy of
# a run can remove the same task at the same time.
function Remove-EndedTask {
    foreach ($task in @(Get-ScheduledTask -TaskPath $TaskPath -ErrorAction SilentlyContinue)) {
        $name = $task.TaskName
        if ($name -eq $Run -or $name -notmatch '^[A-Za-z0-9-]+$') { continue }
        $folder = Join-Path (Join-Path $Directory 'runs') $name
        if ((Test-Path $folder) -and -not (Test-Path (Join-Path $folder 'finished')) -and
            -not (Test-RunEnded $folder $name)) { continue }
        Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
    }
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

# Starts the run: everything after the check of the session, until the removal copy runs.
function Start-Run([string]$User) {
    $refusal = Get-DesktopRefusal $User @(Get-DesktopSession $User)
    if ($refusal) { Stop-Start 2 $refusal }
    Remove-EndedTask

    $bash = Join-Path $Directory 'tools\git\bin\bash.exe'
    $console = Join-Path $env:SystemRoot 'System32\conhost.exe'
    $arguments = "--headless $bash $here\start.sh"
    $action = New-ScheduledTaskAction -Execute $console -Argument $arguments -WorkingDirectory $here
    $principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Limited
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Action $action -Principal $principal `
        -Settings $settings | Out-Null
    Start-ScheduledTask -TaskPath $TaskPath -TaskName $Run
    $deadline = (Get-Date).AddSeconds($StartSeconds)
    while (-not (Test-Path (Join-Path $here 'started'))) {
        if ((Get-Date) -gt $deadline) {
            $info = Get-ScheduledTaskInfo -TaskPath $TaskPath -TaskName $Run -ErrorAction SilentlyContinue
            $result = if ($info) { $info.LastTaskResult } else { 'not known' }
            Stop-RunTask
            Stop-Start 1 "the run $Run did not start within $StartSeconds s; the task result is $result"
        }
        Start-Sleep -Milliseconds 500
    }

    $process = Get-CimInstance Win32_Process -Filter "Name = 'conhost.exe'" |
        Where-Object { $_.CommandLine -like "*$arguments" } | Select-Object -First 1
    $created = if ($process) { [JuntoCi.Native]::CreationTime([int]$process.ProcessId) } else { 0 }
    if ($created -eq 0) {
        # Without the process the removal cannot wait for the run. The task stays, so that
        # Test-RunEnded reads the end of the run from its state; a later start removes it.
        Add-Content -Path (Join-Path $here 'task.log') -Value "the process of the run was not found after its start"
        return
    }
    Set-Content -NoNewline -Path (Join-Path $here 'process') -Value "$($process.ProcessId) $created`n"
    $remover = "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $here\start-run.ps1 " +
        "-Directory $Directory -Run $Run -Remove -After $($process.ProcessId)"
    $result = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine = $remover; CurrentDirectory = $here
    }
    if ($result.ReturnValue -ne 0) {
        # The run goes on; a later start removes its task.
        Write-Output "the run $Run started, but the removal of its task did not start ($($result.ReturnValue))"
    }
}

# Stops and removes the task of this run.
function Stop-RunTask {
    Stop-ScheduledTask -TaskPath $TaskPath -TaskName $Run -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $Run -Confirm:$false -ErrorAction SilentlyContinue
}

# Start-Run with the handling of every error: before "started" the run has not started, and its
# end is recorded; after it the run goes on.
function Invoke-Start {
    try {
        Start-Run ([Security.Principal.WindowsIdentity]::GetCurrent().Name)
    } catch {
        if (Test-Path (Join-Path $here 'started')) {
            $message = "the run $Run started, but its start did not complete: $_"
            Add-Content -Path (Join-Path $here 'task.log') -Value $message
            Write-Output $message
            exit 0
        }
        Stop-RunTask
        Stop-Start 1 "the run $Run did not start: $_"
    }
    exit 0
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
    if (-not (Test-RunEnded $here $Run)) { exit 1 }
    Write-Output "ended"
    exit 0
}

if ($Remove) {
    if ($After -gt 0) {
        try {
            # ES_CONTINUOUS | ES_SYSTEM_REQUIRED (0x80000001, which PowerShell would read as a
            # negative Int32): Windows does not sleep while this thread waits.
            [JuntoCi.Native]::SetThreadExecutionState([uint32]2147483649) | Out-Null
            Wait-Process -Id $After -ErrorAction SilentlyContinue
        } catch {
            Add-Content -Path (Join-Path $here 'task.log') -Value "the wait for the process of the run failed: $_"
        }
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

Invoke-Start
