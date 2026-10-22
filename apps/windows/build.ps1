# Publishes the Windows app self-contained for x64 into apps/windows/out/app (docs/notes/windows-shell.md). Run
# bindings/csharp/build.ps1 first: it builds current_ffi.dll and generates the C# bindings this app compiles against.
param(
    # The app's version; defaults to the workspace version in Cargo.toml.
    [string]$Version
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$root = Resolve-Path "$PSScriptRoot/../.."
$lib = Join-Path $root 'target/x86_64-pc-windows-msvc/release'
$out = Join-Path $PSScriptRoot 'out/app'

if (-not $Version) {
    $match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if (-not $match) { throw 'no workspace version in Cargo.toml' }
    $Version = $match.Matches[0].Groups[1].Value
}
if (-not (Test-Path (Join-Path $lib 'current_ffi.dll'))) { throw "current_ffi.dll is missing in ${lib}: run bindings/csharp/build.ps1" }

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
Copy-Item (Join-Path $dotnet 'THIRD-PARTY-NOTICES.TXT') (Join-Path $licences "Microsoft.NETCore.App-$($runtime.version)-THIRD-PARTY-NOTICES.txt")
Write-Host "Runtime packages: $($restored -join ', '); Microsoft.NETCore.App $($runtime.version)"

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
