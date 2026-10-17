# Prepares a Windows machine for the Windows job of CI (ADR 0905). tools/ci/ci.sh copies this
# file to the machine and runs it with Windows PowerShell 5.1:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File ci-setup.ps1 -Directory C:\junto-ci
#
# Every tool is downloaded from its publisher at the version below, checked by its SHA-256,
# and installed below -Directory, except the parts of the Build Tools that the Visual Studio
# installer places in fixed folders (the installer itself and the Windows SDK). The script
# changes no system setting and no PATH. A tool whose recorded version is the pinned one is
# left as it is, so a second run downloads nothing.
#
# Exit codes: 0 when every tool is ready; 1 when a step failed; 3 when the Build Tools need a
# restart of Windows to finish. The script never restarts Windows.
param([Parameter(Mandatory = $true)][string]$Directory)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# Git for Windows, portable: Git and the Bash in which the job scripts run.
$GitVersion = '2.56.0.2'
$GitUrl = 'https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.2/PortableGit-2.56.0.2-64-bit.7z.exe'
$GitSha256 = '075e158ef8e1f0ab80b347e245405d3eca735c2dc88fd8e032e137d0ca61f61b'
# Python of python.org in its NuGet form, a folder without an installer. 3.12.10 is the last
# release of 3.12, the version of the Python image of tools/ci/pins.sh, with Windows binaries.
$PythonVersion = '3.12.10'
$PythonUrl = 'https://api.nuget.org/v3-flatcontainer/python/3.12.10/python.3.12.10.nupkg'
$PythonSha256 = '0eb85c2dfccccf1b17352de4c397f69194035b7d37149eacc16f1147d93de3b8'
# rustup; the job installs the Rust version of rust-toolchain.toml with it.
$RustupVersion = '1.29.1'
$RustupUrl = 'https://static.rust-lang.org/rustup/archive/1.29.1/x86_64-pc-windows-msvc/rustup-init.exe'
$RustupSha256 = '6f4bef66261261fcb43131be8720bab817d403a09edec7455c371974b90bdb7e'
# Visual Studio Build Tools 2022 by the bootstrapper of one release, which installs the
# components of that release: the MSVC compiler and linker for x64 and the Windows 11 SDK.
$BuildToolsVersion = '17.14.42'
$BuildToolsUrl = 'https://download.visualstudio.microsoft.com/download/pr/c95450be-0a45-4779-b142-33f567ccfd6e/68978bb28916156802e60e04c5ee50b94348d802fe151a3ca24433f27984187b/vs_BuildTools.exe'
$BuildToolsSha256 = '68978bb28916156802e60e04c5ee50b94348d802fe151a3ca24433f27984187b'
$BuildToolsComponents = @(
    'Microsoft.VisualStudio.Workload.VCTools',
    'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    'Microsoft.VisualStudio.Component.Windows11SDK.26100'
)

$Tools = Join-Path $Directory 'tools'
$Downloads = Join-Path $Directory 'downloads'
New-Item -ItemType Directory -Force -Path $Tools, $Downloads | Out-Null

# The file of a download in $Downloads with the pinned SHA-256, downloaded when it is absent
# or differs.
function Get-Pinned([string]$Url, [string]$Sha256) {
    $path = Join-Path $Downloads ($Url -split '/')[-1]
    if ((Test-Path $path) -and (Get-FileHash -Algorithm SHA256 $path).Hash -eq $Sha256) {
        return $path
    }
    $partial = "$path.part"
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $partial
    $found = (Get-FileHash -Algorithm SHA256 $partial).Hash
    if ($found -ne $Sha256) {
        Remove-Item $partial
        throw "the download of $Url has the SHA-256 $found, not the pinned $Sha256"
    }
    Move-Item -Force $partial $path
    return $path
}

# True when the tool in $Folder was installed at $Version by this script.
function Test-Installed([string]$Folder, [string]$Version) {
    $mark = Join-Path $Folder '.junto-version'
    return (Test-Path $mark) -and ((Get-Content -Raw $mark).Trim() -eq $Version)
}

function Set-Installed([string]$Folder, [string]$Version) {
    Set-Content -NoNewline -Path (Join-Path $Folder '.junto-version') -Value $Version
}

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    $process = Start-Process -FilePath $Program -ArgumentList $Arguments -NoNewWindow -Wait -PassThru
    return $process.ExitCode
}

function Install-Git {
    $folder = Join-Path $Tools 'git'
    if (Test-Installed $folder $GitVersion) { return }
    $archive = Get-Pinned $GitUrl $GitSha256
    if (Test-Path $folder) { Remove-Item -Recurse -Force $folder }
    # The self-extracting 7-Zip archive takes the destination and "yes to all".
    $code = Invoke-Checked $archive @("-o`"$folder`"", '-y')
    if ($code -ne 0 -or -not (Test-Path (Join-Path $folder 'bin\bash.exe'))) {
        throw "the extraction of Git for Windows ended with exit code $code"
    }
    Set-Installed $folder $GitVersion
}

function Install-Python {
    $folder = Join-Path $Tools 'python'
    if (Test-Installed $folder $PythonVersion) { return }
    $package = Get-Pinned $PythonUrl $PythonSha256
    if (Test-Path $folder) { Remove-Item -Recurse -Force $folder }
    $unpacked = Join-Path $Downloads 'python-package'
    if (Test-Path $unpacked) { Remove-Item -Recurse -Force $unpacked }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($package, $unpacked)
    # The package holds the Python folder under tools\.
    Move-Item (Join-Path $unpacked 'tools') $folder
    Remove-Item -Recurse -Force $unpacked
    # The job scripts call python3; python.exe finds its library next to itself under any name.
    Copy-Item (Join-Path $folder 'python.exe') (Join-Path $folder 'python3.exe')
    Set-Installed $folder $PythonVersion
}

# rustup replaces itself with its latest release when a command installs a toolchain, unless
# its self-update mode, stored in RUSTUP_HOME, is "disable". The version is read from rustup
# itself, so a rustup that replaced itself before this mode was set is installed again.
# Runs rustup and returns its standard output; rustup writes its notes to the error output,
# which Windows PowerShell would turn into an error under 'Stop'.
function Invoke-Rustup([string]$Rustup, [string[]]$Arguments) {
    $ErrorActionPreference = 'Continue'
    $output = & $Rustup @Arguments 2>$null
    if ($LASTEXITCODE -ne 0) { throw "rustup $($Arguments -join ' ') ended with exit code $LASTEXITCODE" }
    return ($output -join ' ')
}

function Install-Rustup {
    $cargo = Join-Path $Directory 'cargo'
    $env:CARGO_HOME = $cargo
    $env:RUSTUP_HOME = Join-Path $Directory 'rustup'
    $rustup = Join-Path $cargo 'bin\rustup.exe'
    $pinned = "^rustup $([regex]::Escape($RustupVersion)) "
    $found = if (Test-Path $rustup) { Invoke-Rustup $rustup @('--version') } else { '' }
    if (-not (Test-Installed $cargo $RustupVersion) -or $found -notmatch $pinned) {
        $installer = Get-Pinned $RustupUrl $RustupSha256
        $code = Invoke-Checked $installer @('-y', '--no-modify-path', '--no-update-default-toolchain',
            '--default-toolchain', 'none', '--profile', 'minimal')
        if ($code -ne 0) { throw "rustup-init ended with exit code $code" }
        Set-Installed $cargo $RustupVersion
    }
    Invoke-Rustup $rustup @('set', 'auto-self-update', 'disable') | Out-Null
    $found = Invoke-Rustup $rustup @('--version')
    if ($found -notmatch $pinned) { throw "rustup states '$found', not $RustupVersion" }
}

function Install-BuildTools {
    $folder = Join-Path $Directory 'vs'
    if (Test-Installed $folder $BuildToolsVersion) { return }
    $bootstrapper = Get-Pinned $BuildToolsUrl $BuildToolsSha256
    $arguments = @('--quiet', '--wait', '--norestart', '--nocache', '--installPath', "`"$folder`"")
    foreach ($component in $BuildToolsComponents) { $arguments += @('--add', $component) }
    $code = Invoke-Checked $bootstrapper $arguments
    # 3010: installed, and Windows must restart before the tools can be used.
    if ($code -eq 3010) {
        Write-Output "the Build Tools $BuildToolsVersion are installed and need a restart of Windows"
        exit 3
    }
    if ($code -ne 0) { throw "the installer of the Build Tools ended with exit code $code" }
    $found = Get-ChildItem (Join-Path $folder 'VC\Tools\MSVC') -Directory -ErrorAction SilentlyContinue
    if (-not $found) { throw "the Build Tools installed no MSVC compiler in $folder" }
    Set-Installed $folder $BuildToolsVersion
}

try {
    Install-Git
    Install-Python
    Install-Rustup
    Install-BuildTools
} catch {
    Write-Output "the setup failed: $_"
    exit 1
}
Write-Output "ready: Git $GitVersion, Python $PythonVersion, rustup $RustupVersion, Build Tools $BuildToolsVersion in $Directory"
exit 0
