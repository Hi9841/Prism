[CmdletBinding()]
param(
  [Parameter(Mandatory)]
  [string]$PreviousTag
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PreviousTag -notmatch '^v?(\d+\.\d+\.\d+)$') {
  throw "Invalid previous release tag '$PreviousTag'."
}
$previousVersion = [version]$Matches[1]
$source = git rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw 'Could not resolve the source commit.' }
$previousCommit = git rev-parse --verify "refs/tags/$PreviousTag^{commit}"
if ($LASTEXITCODE -ne 0) { throw "Could not resolve release tag '$PreviousTag'." }
git merge-base --is-ancestor $source $previousCommit
if ($LASTEXITCODE -eq 0) {
  Write-Host "Release $PreviousTag already includes commit $source."
  return
}
if ($LASTEXITCODE -ne 1) { throw 'Could not compare the source and release commits.' }

$manifestPath = (Resolve-Path -LiteralPath Cargo.toml).Path
$manifest = [System.IO.File]::ReadAllText($manifestPath)
$pattern = [regex]'(?m)(?<=^version = ")\d+\.\d+\.\d+(?="\r?$)'
if ($pattern.Matches($manifest).Count -ne 1) { throw 'Could not identify the workspace version.' }
$workspaceVersion = [version]$pattern.Match($manifest).Value
$tag = $workspaceVersion.ToString()
if ($workspaceVersion -le $previousVersion) {
  $tag = "$($previousVersion.Major).$($previousVersion.Minor).$($previousVersion.Build + 1)"
  [System.IO.File]::WriteAllText($manifestPath, $pattern.Replace($manifest, $tag))
  cargo update --workspace | Out-Host
  if ($LASTEXITCODE -ne 0) { throw 'Could not update the workspace version in Cargo.lock.' }
}
& (Join-Path $PSScriptRoot 'check-version.ps1') -ExpectedTag $tag

$releaseCommit = $source
if ($tag -ne $workspaceVersion.ToString()) {
  git add -- Cargo.toml Cargo.lock
  if ($LASTEXITCODE -ne 0) { throw 'Could not stage the release version.' }
  $tree = git write-tree
  if ($LASTEXITCODE -ne 0) { throw 'Could not write the release tree.' }
  $gitIdentity = @(
    '-c', 'user.name=github-actions[bot]',
    '-c', 'user.email=41898282+github-actions[bot]@users.noreply.github.com'
  )
  $releaseCommit = git @gitIdentity commit-tree $tree -p $source -m "Release Prism $tag"
  if ($LASTEXITCODE -ne 0) { throw 'Could not create the release commit.' }
}

$tagRef = "refs/tags/$tag"
git show-ref --verify --quiet $tagRef
if ($LASTEXITCODE -eq 0) {
  $tagCommit = git rev-parse --verify "${tagRef}^{commit}"
  if ($LASTEXITCODE -ne 0) { throw "Could not resolve release tag '$tag'." }
  if ($tagCommit -ne $releaseCommit) {
    $tagTree = git rev-parse "${tagRef}^{tree}"
    $tagParent = git rev-parse "${tagRef}^1"
    if ($tag -eq $workspaceVersion.ToString() -or $tagTree -ne $tree -or $tagParent -ne $source) {
      throw "Tag $tag already points to another release commit."
    }
  }
} elseif ($LASTEXITCODE -eq 1) {
  git -c 'user.name=github-actions[bot]' -c 'user.email=41898282+github-actions[bot]@users.noreply.github.com' tag -a $tag -m "Prism $tag" $releaseCommit
  if ($LASTEXITCODE -ne 0) { throw 'Could not create the release tag.' }
} else {
  throw "Could not check release tag '$tag'."
}

$tag
