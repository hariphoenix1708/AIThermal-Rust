# AIThermal-Rust Windows build script
$ErrorActionPreference = 'Stop'

# Resolve script directory explicitly
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location -Path $ScriptDir

Write-Host "Building AIThermal-Rust daemon for Android aarch64..."

# Resolve Android NDK path
$ndkPath = $env:ANDROID_NDK_HOME
if ([string]::IsNullOrWhiteSpace($ndkPath)) {
    $ndkPath = $env:ANDROID_NDK_ROOT
}

if ([string]::IsNullOrWhiteSpace($ndkPath)) {
    Write-Warning "ANDROID_NDK_HOME or ANDROID_NDK_ROOT is not set. The build might fail if the linker is not in PATH."
} else {
    # Find the Clang wrapper for Android 14+ (API level 34)
    $linkerPath = Join-Path $ndkPath "toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android34-clang.cmd"
    if (-not (Test-Path $linkerPath)) {
        # Fallback to searching for the highest version available if 34 isn't found
        $binDir = Join-Path $ndkPath "toolchains\llvm\prebuilt\windows-x86_64\bin"
        if (Test-Path $binDir) {
            $compilers = Get-ChildItem -Path $binDir -Filter "aarch64-linux-android*-clang.cmd"
            if ($compilers.Count -gt 0) {
                # Pick the first one we find as fallback
                $linkerPath = $compilers[0].FullName
                Write-Host "API 34 clang not found, falling back to: $linkerPath"
            }
        }
    }

    if (Test-Path $linkerPath) {
        Write-Host "Using Android NDK linker: $linkerPath"
        $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $linkerPath
    } else {
        Write-Warning "Could not find aarch64-linux-android-clang.cmd in NDK. Falling back to default configuration."
    }
}

Set-Location -Path "rust"
cargo build --release --target aarch64-linux-android
cargo build --release --target aarch64-linux-android --bin thermalai-detect
cargo build --release --target aarch64-linux-android --bin thermalair

Write-Host "Preparing module staging directory..."
Set-Location -Path $ScriptDir

function Test-AndroidArm64Elf {
    param([Parameter(Mandatory=$true)][string]$Path)
    if (-not (Test-Path $Path)) {
        throw "Missing binary: $Path"
    }
    $bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $Path))
    if ($bytes.Length -lt 20 -or $bytes[0] -ne 0x7F -or $bytes[1] -ne 0x45 -or $bytes[2] -ne 0x4C -or $bytes[3] -ne 0x46) {
        throw "Invalid ELF magic: $Path"
    }
    $machine = [BitConverter]::ToUInt16($bytes, 18)
    if ($machine -ne 0xB7) {
        throw "Binary is not AArch64 ELF: $Path (e_machine=$machine)"
    }
}

$srcPath = "rust\target\aarch64-linux-android\release\thermalai-daemon"

if (Test-Path $srcPath) {
        Write-Host "Preparing files for zipping..."
        $StagingDir = Join-Path $ScriptDir "staging_zip"
        if (Test-Path $StagingDir) { Remove-Item $StagingDir -Recurse -Force }
        New-Item -ItemType Directory -Path $StagingDir | Out-Null
        New-Item -ItemType Directory -Force -Path (Join-Path $StagingDir "system\bin") | Out-Null

        Copy-Item -Path "META-INF", "config", "module.prop", "service.sh", "customize.sh", "sepolicy.rule", "uninstall.sh" -Destination $StagingDir -Recurse -Force
        Copy-Item -Path "rust\target\aarch64-linux-android\release\thermalai-daemon" -Destination (Join-Path $StagingDir "system\bin\thermalai-daemon") -Force
        Copy-Item -Path "rust\target\aarch64-linux-android\release\thermalai-detect" -Destination (Join-Path $StagingDir "system\bin\thermalai-detect") -Force
        Copy-Item -Path "rust\target\aarch64-linux-android\release\thermalair" -Destination (Join-Path $StagingDir "system\bin\thermalair") -Force

        Test-AndroidArm64Elf (Join-Path $StagingDir "system\bin\thermalai-daemon")
        Test-AndroidArm64Elf (Join-Path $StagingDir "system\bin\thermalai-detect")
        Test-AndroidArm64Elf (Join-Path $StagingDir "system\bin\thermalair")

        Write-Host "Enforcing LF line endings for Android shell scripts..."
        $TextFiles = Get-ChildItem -Path $StagingDir -Recurse -File | Where-Object {
            $_.Extension -in @(".sh", ".prop", ".conf", ".md", ".rule") -or $_.Name -in @("update-binary", "updater-script")
        }
        foreach ($file in $TextFiles) {
            $text = [System.IO.File]::ReadAllText($file.FullName)
            if ($text -match "`r") {
                $text = $text -replace "`r", ""
                # Use UTF8 without BOM
                $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
                [System.IO.File]::WriteAllText($file.FullName, $text, $utf8NoBom)
                Write-Host "  Converted $($file.Name) to LF"
            }
        }

        Write-Host "Zipping module..."
        if (Test-Path "AIThermal-Rust.zip") {
            Remove-Item "AIThermal-Rust.zip" -Force
        }

        $sevenZip = (Get-Command 7z.exe -ErrorAction SilentlyContinue).Source
        if ([string]::IsNullOrWhiteSpace($sevenZip)) {
            $candidate = Join-Path $env:ProgramFiles "7-Zip\7z.exe"
            if (Test-Path $candidate) { $sevenZip = $candidate }
        }
        if ([string]::IsNullOrWhiteSpace($sevenZip)) {
            throw "7-Zip command line executable was not found. Install 7-Zip or add 7z.exe to PATH."
        }

        Write-Host "Using 7-Zip for zip creation: $sevenZip"
        Set-Location -Path $StagingDir
        & $sevenZip a -tzip (Join-Path $ScriptDir "AIThermal-Rust.zip") ".\*" | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "7-Zip failed with exit code $LASTEXITCODE" }
        Set-Location -Path $ScriptDir

        Remove-Item $StagingDir -Recurse -Force
        Write-Host "Build complete."
} else {
    Write-Error "ERROR: Target binary missing after build. Something failed."
    Throw "Build failed"
}
