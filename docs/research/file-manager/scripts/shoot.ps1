# Screenshots every screen of the Two panels prototype in look A with headless
# Chrome, as NN-<title>.png in -Out. -Width and -Height set the browser size.
param([string]$Out = "$PSScriptRoot\shots", [int]$Width = 1280, [int]$Height = 1000)
$proto = 'C:\SynoSoftware\goz-file-manager\crates\gozler-ui\two-panels.prototype.html'
$chrome = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
$titles = Select-String -Path $proto -Pattern '\{ t: "([^"]+)"' -AllMatches | ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value }
New-Item -ItemType Directory -Force $Out | Out-Null
$userData = Join-Path $env:TEMP 'proto-chrome'
$i = 0
foreach ($t in $titles) {
  $i++
  $name = '{0:D2}-{1}.png' -f $i, ($t -replace '[^A-Za-z0-9]+', '-')
  $url = 'file:///' + $proto.Replace([char]92, [char]47) + '?variant=A#' + [uri]::EscapeDataString($t)
  & $chrome --headless=new --disable-gpu --hide-scrollbars "--user-data-dir=$userData" "--window-size=$Width,$Height" "--screenshot=$(Join-Path $Out $name)" $url 2>$null | Out-Null
  Write-Output $name
}
