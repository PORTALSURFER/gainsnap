param(
    [ValidateSet("stable", "rc", "nightly")]
    [string] $Channel = "stable",
    [Parameter(Mandatory = $true)]
    [string] $PackageVersion,
    [Parameter(Mandatory = $true)]
    [string] $PublicationVersion,
    [Parameter(Mandatory = $true)]
    [string] $BuildId,
    [Parameter(Mandatory = $true)]
    [string] $ReleasedAt,
    [Parameter(Mandatory = $true)]
    [string] $SourceSha,
    [string] $Formats = "clap,vst3"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)] [string] $File,
        [Parameter(Mandatory = $true)] [string[]] $Arguments
    )

    & $File @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "${File} exited with status $LASTEXITCODE"
    }
}

function Invoke-NativeOutput {
    param(
        [Parameter(Mandatory = $true)] [string] $File,
        [Parameter(Mandatory = $true)] [string[]] $Arguments
    )

    $output = & $File @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "${File} exited with status $LASTEXITCODE`n$($output -join "`n")"
    }
    return ($output -join "`n").Trim()
}

function Assert-Zip {
    param(
        [Parameter(Mandatory = $true)] [string] $ArchivePath,
        [Parameter(Mandatory = $true)] [string] $ExpectedFile
    )

    $archiveInfo = Get-Item -LiteralPath $ArchivePath
    if ($archiveInfo.Length -le 0) {
        throw "archive is empty: $ArchivePath"
    }

    $zip = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $files = @($zip.Entries | Where-Object { -not $_.FullName.EndsWith("/") })
        if ($files.Count -ne 1) {
            $members = ($files | ForEach-Object { $_.FullName }) -join ", "
            throw "unexpected Windows archive contents in $ArchivePath`: $members"
        }
        if ($files[0].FullName -ne $ExpectedFile) {
            throw "unexpected Windows archive member in $ArchivePath`: $($files[0].FullName)"
        }
        if ($files[0].Length -le 0) {
            throw "archive member is empty: $ExpectedFile"
        }
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName.StartsWith("/") -or $entry.FullName.Contains("..")) {
                throw "archive contains an unsafe member path: $($entry.FullName)"
            }
        }
    }
    finally {
        $zip.Dispose()
    }
}

function Normalize-ZipTimestamps {
    param(
        [Parameter(Mandatory = $true)] [string] $ArchivePath
    )

    $zip = [System.IO.Compression.ZipFile]::Open(
        $ArchivePath,
        [System.IO.Compression.ZipArchiveMode]::Update
    )
    try {
        $epoch = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
        foreach ($entry in $zip.Entries) {
            $entry.LastWriteTime = $epoch
        }
    }
    finally {
        $zip.Dispose()
    }
}

function New-Archive {
    param(
        [Parameter(Mandatory = $true)] [string] $SourcePath,
        [Parameter(Mandatory = $true)] [bool] $SourceIsDirectory,
        [Parameter(Mandatory = $true)] [string] $ArchivePath,
        [Parameter(Mandatory = $true)] [string] $ExpectedFile
    )

    $stageParent = Join-Path $repoRoot ".tmp"
    New-Item -ItemType Directory -Path $stageParent -Force | Out-Null
    $stageRoot = Join-Path $stageParent ("windows-package-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $stageRoot | Out-Null
    try {
        if ($SourceIsDirectory) {
            $stagePath = Join-Path $stageRoot (Split-Path -Leaf $SourcePath)
            Copy-Item -LiteralPath $SourcePath -Destination $stagePath -Recurse
        }
        else {
            $stagePath = Join-Path $stageRoot (Split-Path -Leaf $SourcePath)
            Copy-Item -LiteralPath $SourcePath -Destination $stagePath
        }

        if (Test-Path -LiteralPath $ArchivePath) {
            throw "refusing to overwrite existing archive: $ArchivePath"
        }
        [System.IO.Compression.ZipFile]::CreateFromDirectory(
            $stageRoot,
            $ArchivePath,
            [System.IO.Compression.CompressionLevel]::Optimal,
            $false
        )
        Normalize-ZipTimestamps -ArchivePath $ArchivePath
        Assert-Zip -ArchivePath $ArchivePath -ExpectedFile $ExpectedFile
    }
    finally {
        if (Test-Path -LiteralPath $stageRoot) {
            Remove-Item -LiteralPath $stageRoot -Recurse -Force
        }
    }
}

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw "the unsigned Windows release lane must run on Windows"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot
$slug = "gainsnap"
$declaredFormats = @(
    "clap,vst3".Split(",", [System.StringSplitOptions]::RemoveEmptyEntries) |
        ForEach-Object { $_.Trim().ToLowerInvariant() } |
        Where-Object { $_ }
)

if ($SourceSha -notmatch "^[0-9a-fA-F]{40}$") {
    throw "SourceSha must be the 40-character checked-out commit SHA"
}

$branch = (& git symbolic-ref --quiet --short HEAD 2>$null)
if ($LASTEXITCODE -eq 0 -and $branch.Trim() -ne "main") {
    throw "Windows release packaging must run from the main branch (found '$($branch.Trim())')"
}
$actualSha = Invoke-NativeOutput -File "git" -Arguments @("rev-parse", "HEAD")
if ($actualSha -ne $SourceSha) {
    throw "checked-out source $actualSha does not match requested source $SourceSha"
}
$status = Invoke-NativeOutput -File "git" -Arguments @("status", "--porcelain")
if ($status) {
    throw "working tree is not clean before Windows packaging:`n$status"
}

$metadataJson = Invoke-NativeOutput -File "cargo" -Arguments @("metadata", "--quiet", "--locked", "--no-deps", "--format-version", "1")
$metadata = $metadataJson | ConvertFrom-Json
$package = @($metadata.packages)[0]
$metadataPackageVersion = [string] $package.version
if ($metadataPackageVersion -cne $PackageVersion) {
    throw "requested package version $PackageVersion does not match Cargo.toml $metadataPackageVersion"
}
$coreVersionPattern = "(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
if ($PackageVersion -cnotmatch "^$coreVersionPattern$") {
    throw "unsupported package version for Windows release: $PackageVersion"
}
$publicationPattern = switch ($Channel) {
    "stable" { "^$coreVersionPattern$"; break }
    "rc" { "^$coreVersionPattern-rc\.[1-9][0-9]*$"; break }
    "nightly" { "^$coreVersionPattern-nightly\.[1-9][0-9]*$"; break }
}
if ($PublicationVersion -cnotmatch $publicationPattern) {
    throw "publication version $PublicationVersion does not match the $Channel release version syntax"
}
$publicationCore = ($PublicationVersion -split "-", 2)[0]
if ($publicationCore -cne $PackageVersion) {
    throw "publication version $PublicationVersion does not match package version $PackageVersion"
}
if ([string]::IsNullOrWhiteSpace($ReleasedAt) -or $ReleasedAt -cnotmatch "(?:Z|[+-][0-9]{2}:?[0-9]{2})$") {
    throw "ReleasedAt must be an RFC3339 timestamp with a timezone"
}
try {
    [DateTimeOffset]::Parse($ReleasedAt, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind) | Out-Null
}
catch {
    throw "ReleasedAt must be an RFC3339 timestamp: $ReleasedAt"
}

$requestedFormats = @(
    $Formats.Split(",", [System.StringSplitOptions]::RemoveEmptyEntries) |
        ForEach-Object { $_.Trim().ToLowerInvariant() } |
        Where-Object { $_ }
)
$uniqueFormats = @($requestedFormats | Sort-Object -Unique)
if ($requestedFormats.Count -eq 0 -or $requestedFormats.Count -ne $uniqueFormats.Count) {
    throw "Formats must contain each requested format exactly once"
}
foreach ($format in $requestedFormats) {
    if ($format -notin @("clap", "vst3")) {
        throw "unsupported Windows format: $format"
    }
    if ($format -notin $declaredFormats) {
        throw "Windows format '$format' is not declared by this product"
    }
}

$bundleStem = "$slug-v$PackageVersion"
$distRoot = Join-Path $repoRoot "dist"
$expectedBuildId = "$slug-v$PublicationVersion-$($SourceSha.Substring(0, 12))"
if ($BuildId -cnotmatch "^[a-z0-9][a-z0-9._-]{1,127}$" -or $BuildId -cne $expectedBuildId) {
    throw "BuildId must be $expectedBuildId"
}
$releaseId = "$BuildId-windows-unsigned"
$releaseRoot = Join-Path $distRoot (Join-Path "releases\windows" $releaseId)
if (Test-Path -LiteralPath $releaseRoot) {
    throw "refusing to overwrite existing Windows release directory: $releaseRoot"
}
New-Item -ItemType Directory -Path $releaseRoot -Force | Out-Null

$vst3SdkDir = [Environment]::GetEnvironmentVariable("VST3_SDK_DIR")
if ($requestedFormats -contains "vst3" -and [string]::IsNullOrWhiteSpace($vst3SdkDir)) {
    throw "VST3_SDK_DIR is required for the Windows VST3 build"
}
if ($requestedFormats -contains "vst3" -and -not (Test-Path -LiteralPath $vst3SdkDir)) {
    throw "VST3_SDK_DIR does not exist: $vst3SdkDir"
}
if ($requestedFormats -contains "vst3" -and -not (Test-Path -LiteralPath (Join-Path $vst3SdkDir "pluginterfaces"))) {
    throw "VST3_SDK_DIR is missing pluginterfaces: $vst3SdkDir"
}

foreach ($format in $requestedFormats) {
    $bundlePath = Join-Path $distRoot "$bundleStem.$format"
    if ($format -eq "vst3") {
        $binaryPath = Join-Path $bundlePath ("Contents\x86_64-win\$bundleStem.vst3")
    }
    else {
        $binaryPath = $bundlePath
    }
    if (Test-Path -LiteralPath $bundlePath) {
        throw "refusing to package stale Windows bundle output: $bundlePath"
    }

    $env:TOYBOX_ACTIVE_ARTIFACT = $format
    $env:CARGO_TARGET_DIR = Join-Path $repoRoot ("target\windows-$format")
    if ($format -eq "vst3") {
        Invoke-Native -File "cargo" -Arguments @("rustc", "--locked", "--release", "--features", "vst3", "--lib")
    }
    else {
        Invoke-Native -File "cargo" -Arguments @("build", "--locked", "--release")
    }

    if (-not (Test-Path -LiteralPath $binaryPath)) {
        throw "Toybox did not produce the expected Windows $format bundle: $binaryPath"
    }
    if ((Get-Item -LiteralPath $binaryPath).Length -le 0) {
        throw "Toybox produced an empty Windows $format binary: $binaryPath"
    }

    $archiveName = "$slug-v$PublicationVersion-windows-unsigned.$format.zip"
    $archivePath = Join-Path $releaseRoot $archiveName
    $expectedMember = if ($format -eq "vst3") {
        "$bundleStem.vst3/Contents/x86_64-win/$bundleStem.vst3"
    }
    else {
        "$bundleStem.clap"
    }
    New-Archive `
        -SourcePath $bundlePath `
        -SourceIsDirectory ($format -eq "vst3") `
        -ArchivePath $archivePath `
        -ExpectedFile $expectedMember
}

Write-Host "Unsigned Windows artifacts passed installability audit:"
Get-ChildItem -LiteralPath $releaseRoot -File | ForEach-Object {
    Write-Host ("  {0} ({1} bytes)" -f $_.Name, $_.Length)
}
