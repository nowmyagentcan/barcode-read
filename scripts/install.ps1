# Installs barcode-read @VERSION@ into %LOCALAPPDATA%\Programs\barcode-read after checking the archive's SHA-256.
# irm https://github.com/nowmyagentcan/barcode-read/releases/latest/download/install.ps1 | iex
& {
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'
  $version = '@VERSION@'
  $base = if ($env:BARCODE_READ_BASE) { $env:BARCODE_READ_BASE } else { "https://github.com/nowmyagentcan/barcode-read/releases/download/$version" }
  $archive = "barcode-read-$version-windows-x64.zip"
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid())
  New-Item -ItemType Directory $tmp | Out-Null
  try {
    Invoke-WebRequest "$base/$archive" -OutFile "$tmp\$archive" -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS" -UseBasicParsing
    $expected = Get-Content "$tmp\SHA256SUMS" | ForEach-Object { $hash, $name = -split $_; if ($name -eq $archive) { $hash } }
    if (-not $expected -or (Get-FileHash "$tmp\$archive" -Algorithm SHA256).Hash -ne $expected) {
      throw "barcode-read: the SHA-256 of $archive doesn't match SHA256SUMS; nothing was installed."
    }
    Expand-Archive "$tmp\$archive" "$tmp\x"
    $new = try { & "$tmp\x\barcode-read.exe" --version } catch { }
    if ("$new" -notlike 'barcode-read *') {
      throw "barcode-read: $archive has no barcode-read.exe that runs here; nothing was installed."
    }
    $dir = Join-Path $env:LOCALAPPDATA 'Programs\barcode-read'
    New-Item -ItemType Directory -Force $dir | Out-Null
    $exe = Join-Path $dir 'barcode-read.exe'
    # A running barcode-read (say an MCP client's server) locks its file but not its name, so
    # move it aside, and back if the new one can't be copied; a copy still running is removed
    # by a later install.
    $old = "$exe.$(Get-Random).old"
    if (Test-Path $exe) { Move-Item $exe $old }
    try { Copy-Item "$tmp\x\barcode-read.exe", "$tmp\x\LICENSE", "$tmp\x\THIRD-PARTY-LICENSES.txt" $dir }
    catch { if (Test-Path $old) { Move-Item -Force $old $exe }; throw }
    Remove-Item "$exe.*.old" -ErrorAction SilentlyContinue
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    $path = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    if (($path -split ';') -notcontains $dir) {
      $key.SetValue('Path', "$path;$dir".TrimStart(';'), 'ExpandString')
      # Clearing a user variable makes Windows tell open programs that the environment changed.
      [Environment]::SetEnvironmentVariable('BARCODE_READ_INSTALL', $null, 'User')
      $env:Path += ";$dir"
    }
    & "$dir\barcode-read.exe" --version
  } finally {
    Remove-Item -Recurse -Force $tmp
  }
}
