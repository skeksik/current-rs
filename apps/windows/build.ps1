# Publishes the Windows app self-contained for x64 into apps/windows/out/app (docs/notes/windows-shell.md). Run
# bindings/csharp/build.ps1 first: it generates the C# bindings this app compiles against. This script then builds current_ffi.dll
# again with the libmpv engine (ADR 0019 task 5) and ships our libmpv build's DLLs and licence texts next to Current.exe.
param(
    # The app's version; defaults to the workspace version in Cargo.toml.
    [string]$Version,
    # Our libmpv build (D6, docs/notes/windows-mpv-build.md, "Output": bin\ with the DLL set, lib\mpv.lib, licences\ and
    # components.json), as the "libmpv-windows-x64" artifact unpacks. Defaults to the folder above MPV_LIB_DIR (its lib\).
    [string]$Mpv
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$root = Resolve-Path "$PSScriptRoot/../.."
$target = 'x86_64-pc-windows-msvc'
$lib = Join-Path $root "target/$target/release"
$out = Join-Path $PSScriptRoot 'out/app'

if (-not $Version) {
    $match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if (-not $match) { throw 'no workspace version in Cargo.toml' }
    $Version = $match.Matches[0].Groups[1].Value
}
if (-not (Test-Path (Join-Path $root 'bindings/csharp/Current.Bindings/Generated'))) {
    throw 'the generated C# bindings are missing: run bindings/csharp/build.ps1'
}

if (-not $Mpv) {
    if (-not $env:MPV_LIB_DIR) { throw 'pass -Mpv <libmpv build folder> or set MPV_LIB_DIR to its lib folder' }
    $Mpv = Split-Path $env:MPV_LIB_DIR
}
$Mpv = (Resolve-Path $Mpv).Path
$mpvDlls = @(Get-ChildItem (Join-Path $Mpv 'bin') -Filter '*.dll' -File -ErrorAction SilentlyContinue)
if (-not (Test-Path (Join-Path $Mpv 'lib/mpv.lib')) -or -not ($mpvDlls.Name -contains 'mpv-2.dll')) {
    throw "$Mpv is not a libmpv build: it needs lib\mpv.lib and bin\mpv-2.dll"
}

# The core the app ships: the libmpv engine, linked against mpv.lib (crates/mpv/build.rs), and never the test engine. It replaces the
# one bindings/csharp/build.ps1 left at the same path; the interface, and so the generated C#, is the same.
Push-Location $root
try {
    $env:MPV_LIB_DIR = Join-Path $Mpv 'lib'
    cargo build --release --locked -p current-ffi --target $target --features libmpv
}
finally {
    Pop-Location
}

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
# From this folder, so that its global.json picks the SDK.
Push-Location $PSScriptRoot
try {
    dotnet publish 'Current.Shell/Current.Shell.csproj' -c Release -r win-x64 -p:Platform=x64 `
        "-p:CoreLibDir=$lib" "-p:Version=$Version" -o $out
}
finally {
    Pop-Location
}

# The published core must not have the test engine (crates/app/src/player/test_engine.rs), which only the adapter tests use: its
# library would hold the engine's marker. bindings/csharp/test.ps1 checks that the test runner's core does.
$published = Join-Path $out 'current_ffi.dll'
if (-not (Test-Path $published)) { throw "no current_ffi.dll in $out" }
if ([System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($published)).Contains('current-test-engine')) {
    throw 'the published current_ffi.dll has the test engine'
}

# A cursor without a visual in Current.exe's resources (RT_GROUP_CURSOR 101, its image RT_CURSOR 1), which the player shows to hide
# the pointer in full screen (Current.Shell/Player/PlayerView.xaml.cs, HideCursor): WinUI loads cursors from the executable's resources
# (InputDesktopResourceCursor) and has no empty cursor of its own. Written with Windows' UpdateResource after publishing, so the build
# needs no resource compiler and the manifest and version resources the SDK writes stay as they are. The image: a 32x32 monochrome
# cursor whose AND mask is all ones and XOR mask all zeros, which leaves the screen unchanged (the cursor and icon resource formats:
# "LOCALHEADER", "NEWHEADER", "RESDIR", learn.microsoft.com, not re-read for this; checked on the owner's PC).
Add-Type -Namespace CurrentBuild -Name Resources -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern System.IntPtr BeginUpdateResource(string file, bool deleteExisting);
[System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true)]
public static extern bool UpdateResource(System.IntPtr update, System.IntPtr type, System.IntPtr name, ushort language, byte[] data, uint size);
[System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true)]
public static extern bool EndUpdateResource(System.IntPtr update, bool discard);
'@
function Add-NoCursor([string]$exe) {
    $image = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter $image
    $writer.Write([uint16]0); $writer.Write([uint16]0)                       # hotspot
    $writer.Write([uint32]40); $writer.Write([int32]32); $writer.Write([int32]64) # BITMAPINFOHEADER: 32 wide, XOR and AND masks high
    $writer.Write([uint16]1); $writer.Write([uint16]1)                       # planes, bits per pixel
    foreach ($field in 1..6) { $writer.Write([uint32]0) }                    # compression, sizes, resolution, colours
    $writer.Write([byte[]](0, 0, 0, 0, 255, 255, 255, 0))                    # black, white
    $writer.Write([byte[]]::new(128))                                        # XOR mask: nothing drawn
    $writer.Write([byte[]](,255 * 128))                                      # AND mask: the screen kept
    $writer.Flush()
    $cursor = $image.ToArray()
    $group = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter $group
    $writer.Write([uint16]0); $writer.Write([uint16]2); $writer.Write([uint16]1) # NEWHEADER: cursors, one
    $writer.Write([uint16]32); $writer.Write([uint16]64); $writer.Write([uint16]1); $writer.Write([uint16]1)
    $writer.Write([uint32]$cursor.Length); $writer.Write([uint16]1)          # RESDIR: its size and its RT_CURSOR id
    $writer.Flush()
    $directory = $group.ToArray()
    $update = [CurrentBuild.Resources]::BeginUpdateResource($exe, $false)
    if ($update -eq [IntPtr]::Zero) { throw "BeginUpdateResource failed on $exe" }
    $ok = [CurrentBuild.Resources]::UpdateResource($update, [IntPtr]1, [IntPtr]1, 0, $cursor, $cursor.Length) -and
        [CurrentBuild.Resources]::UpdateResource($update, [IntPtr]12, [IntPtr]101, 0, $directory, $directory.Length)
    if (-not [CurrentBuild.Resources]::EndUpdateResource($update, -not $ok) -or -not $ok) { throw "adding the cursor resource to $exe failed" }
}
Add-NoCursor (Join-Path $out 'Current.exe')

# The libmpv DLL set next to Current.exe, the application's folder, where Windows looks first for current_ffi.dll's imports ("Dynamic
# link library search order", learn.microsoft.com; not re-read for this change). Each LGPL library stays its own DLL, replaceable
# by the user (ADR 0003).
foreach ($dll in $mpvDlls) {
    if (Test-Path (Join-Path $out $dll.Name)) { throw "$($dll.Name) from libmpv would replace a published file" }
    Copy-Item $dll.FullName $out
}
Write-Host "libmpv DLLs: $($mpvDlls.Name -join ', ')"

# The app's PRI file holds its compiled XAML; without it the main window cannot load.
if (-not (Get-ChildItem $out -File | Where-Object Name -in 'Current.pri', 'resources.pri')) { throw "no app PRI file in $out" }

# The licence and notice files of the runtimes in the app (docs/notes/windows-shell.md, ADR 0003), shipped next to it in licences\:
# those of every Windows App SDK component and of WebView2 as restored, and of the .NET runtime pack.
$packages = (dotnet nuget locals global-packages --list) -replace '^global-packages:\s*', ''
$runtime = (Get-Content (Join-Path $out 'Current.runtimeconfig.json') | ConvertFrom-Json).runtimeOptions.includedFrameworks |
    Where-Object name -eq 'Microsoft.NETCore.App'
$assets = Get-Content (Join-Path $PSScriptRoot 'Current.Shell/obj/project.assets.json') -Raw | ConvertFrom-Json
$restored = $assets.libraries.PSObject.Properties.Name |
    Where-Object { $_ -like 'Microsoft.WindowsAppSDK.*/*' -or $_ -like 'Microsoft.Web.WebView2/*' }
$licences = Join-Path $out 'licences'
New-Item -ItemType Directory -Force $licences | Out-Null
foreach ($library in $restored) {
    $name, $libraryVersion = $library -split '/'
    $folder = Join-Path $packages "$($name.ToLowerInvariant())/$libraryVersion"
    $files = Get-ChildItem $folder -File | Where-Object Name -match '^(license|notice)\.txt$'
    if (-not $files) { throw "$library has no licence or notice file" }
    foreach ($file in $files) { Copy-Item $file.FullName (Join-Path $licences "$name-$libraryVersion-$($file.Name)") }
}
$dotnet = Join-Path $packages "microsoft.netcore.app.runtime.win-x64/$($runtime.version)"
Copy-Item (Join-Path $dotnet 'LICENSE.TXT') (Join-Path $licences "Microsoft.NETCore.App-$($runtime.version)-LICENSE.txt")
# The Windows SDK projection's package has no licence file: ship our pointer to its terms.
Copy-Item (Join-Path $PSScriptRoot 'licences/WindowsSDK-projection.txt') $licences
# The Windows Community Toolkit packages (SettingsControls and what it needs) carry only an SPDX expression, MIT: ship the texts of
# their two repositories, copied at the packages' commits (docs/notes/windows-shell.md).
Copy-Item (Join-Path $PSScriptRoot 'licences/CommunityToolkit-Windows-License.md') $licences
Copy-Item (Join-Path $PSScriptRoot 'licences/CommunityToolkit-dotnet-License.md') $licences
Copy-Item (Join-Path $dotnet 'THIRD-PARTY-NOTICES.TXT') (Join-Path $licences "Microsoft.NETCore.App-$($runtime.version)-THIRD-PARTY-NOTICES.txt")
Write-Host "Runtime packages: $($restored -join ', '); Microsoft.NETCore.App $($runtime.version)"

# The libmpv build's licence texts in licences\libmpv: those of every component it marks shipped (a DLL, or linked into one; its build
# tools are not), the LGPL-2.1 text, and components.md with each component's version, licence and source (docs/notes/
# windows-mpv-build.md, "Licence check" and "Publishing the corresponding source").
$mpvLicences = Join-Path $licences 'libmpv'
New-Item -ItemType Directory -Force $mpvLicences | Out-Null
$components = Get-Content (Join-Path $Mpv 'components.json') -Raw | ConvertFrom-Json
foreach ($component in $components | Where-Object shipped) {
    $folder = Join-Path $Mpv "licences/$($component.name)"
    if (-not (Get-ChildItem $folder -File -ErrorAction SilentlyContinue)) { throw "libmpv's $($component.name) has no licence files" }
    Copy-Item $folder (Join-Path $mpvLicences $component.name) -Recurse
}
Copy-Item (Join-Path $Mpv 'licences/LGPL-2.1.txt') $mpvLicences
Copy-Item (Join-Path $Mpv 'components.md') $mpvLicences
Write-Host "libmpv components: $(($components | Where-Object shipped).name -join ', ')"

# What ships, file by file, with the package each file comes from (from Current.deps.json), for the licence list in
# docs/notes/windows-shell.md. Files no package claims are the app's own, the licence files, or listed as "(not in deps.json)".
$deps = Get-Content (Join-Path $out 'Current.deps.json') -Raw | ConvertFrom-Json
$owner = @{}
foreach ($target in $deps.targets.PSObject.Properties) {
    foreach ($library in $target.Value.PSObject.Properties) {
        foreach ($kind in 'runtime', 'native', 'resources') {
            $assets = $library.Value.$kind
            if (-not $assets) { continue }
            foreach ($asset in $assets.PSObject.Properties.Name) {
                $owner[(Split-Path $asset -Leaf).ToLowerInvariant()] = $library.Name
            }
        }
    }
}
Get-ChildItem $out -Recurse -File | Sort-Object FullName | ForEach-Object {
    $path = $_.FullName.Substring($out.Length + 1)
    $package = $owner[$_.Name.ToLowerInvariant()]
    if (-not $package) { $package = '(not in deps.json)' }
    '{0,12}  {1}  {2}' -f $_.Length, $path, $package
} | Tee-Object -FilePath (Join-Path $PSScriptRoot 'out/files.txt')
Write-Host "Published Current $Version to $out"
