[CmdletBinding()]
param(
  [Parameter(Mandatory)]
  [string]$PreviousTag
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-Git {
  $result = & git @args
  if ($LASTEXITCODE -ne 0) { throw "Git failed: $($args -join ' ')" }
  return $result
}

if ($PreviousTag -notmatch '^v?\d+\.\d+\.\d+$') {
  throw "The latest release has an invalid tag: $PreviousTag"
}

$mergedCommit = Invoke-Git rev-parse HEAD
$previousCommit = Invoke-Git rev-parse "$PreviousTag^{}"
git merge-base --is-ancestor $mergedCommit $previousCommit
switch ($LASTEXITCODE) {
  0 {
    Write-Host "Release $PreviousTag already includes this commit."
    return
  }
  1 {}
  default { throw "Could not compare the merged commit with $PreviousTag." }
}

$publishedVersion = [version]($PreviousTag.TrimStart('v'))
$highestVersion = $publishedVersion
$tag = $null
foreach ($existingTag in (Invoke-Git tag --list --sort=-version:refname)) {
  if ($existingTag -notmatch '^\d+\.\d+\.\d+$') { continue }
  $existingVersion = [version]$existingTag
  if ($existingVersion -le $publishedVersion) { break }
  if ($existingVersion -gt $highestVersion) { $highestVersion = $existingVersion }

  # A failed publication can leave its tag. Reuse it only for the same merged commit.
  $tagCommit = Invoke-Git log -1 '--format=%P %s' $existingTag
  if ($tagCommit -eq "$mergedCommit release: Prism $existingTag") {
    $tag = $existingTag
    Invoke-Git checkout --detach "$tag^{}"
    break
  }
}

if (-not $tag) {
  $tag = "$($highestVersion.Major).$($highestVersion.Minor).$($highestVersion.Build + 1)"
  $manifestPath = (Resolve-Path Cargo.toml).Path
  $manifest = [IO.File]::ReadAllText($manifestPath)
  $workspaceVersion = [regex]'(?m)^version = "\d+\.\d+\.\d+"(?=\r?$)'
  if ($workspaceVersion.Matches($manifest).Count -ne 1) {
    throw "Could not find a single workspace version in Cargo.toml."
  }

  [IO.File]::WriteAllText($manifestPath, $workspaceVersion.Replace($manifest, "version = `"$tag`""))
  cargo update --workspace
  if ($LASTEXITCODE -ne 0) { throw "Could not update the workspace lockfile." }

  Invoke-Git add -- Cargo.toml Cargo.lock
  $identity = @('-c', 'user.name=github-actions[bot]', '-c', 'user.email=41898282+github-actions[bot]@users.noreply.github.com')
  Invoke-Git @identity commit -m "release: Prism $tag"
  Invoke-Git @identity tag -a $tag -m "Prism $tag"
}

& "$PSScriptRoot/check-version.ps1" -ExpectedTag $tag
"tag=$tag" >> $env:GITHUB_OUTPUT
"previous_tag=$PreviousTag" >> $env:GITHUB_OUTPUT
