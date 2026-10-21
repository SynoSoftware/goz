# Screenshots of stage 7's search panel, for the owner to review: a search
# panel beside a folder panel, "not indexed", the empty box's text, no
# results with its second line, and All drives with the Folder column.
param([string] $Exe = 'C:\SynoSoftware\TinyTorrent2\3rdParty\goz\target\release\gozler-ui.exe')
. (Join-Path $PSScriptRoot 'common.ps1')
$root = $PSScriptRoot

# The left panel's folder, whose report files lie in subfolders, so the
# Folder column shows where each is.
$fixture = Join-Path $root 'fixture'
Remove-Item -Recurse -Force $fixture -ErrorAction SilentlyContinue
foreach ($folder in 'Invoices', 'Projects\Accounts', 'Projects\Website', 'Scans') {
    New-Item -ItemType Directory (Join-Path $fixture $folder) -Force | Out-Null
}
$files = [ordered]@{
    'Invoices\report 2026-09.pdf' = 1200000; 'Projects\Accounts\quarterly report.xlsx' = 84000
    'Projects\Website\report.html' = 12000; 'Scans\report scan.png' = 220000
    'notes.txt' = 4000; 'report.pdf' = 812000; 'report-draft.pdf' = 790000
}
foreach ($file in $files.Keys) { [IO.File]::WriteAllBytes((Join-Path $fixture $file), [byte[]]::new($files[$file])) }

$placement = @{ left = 60; top = 60; right = 1180; bottom = 700; maximized = $false }
$choices = @{ keep_running = $false; check_for_updates = $false }
$process, $main = Start-Window $Exe (Join-Path $root 'data') @{
    settings = $choices
    window   = @{
        mode = 'two_panels'; placement = $placement
        panels = @{ left = @{ folder = $fixture }; right = @{ folder = 'C:\SynoSoftware\goz-file-manager\crates' }; active = 'left' }
    }
}
try {
    $L = $Panel.Left
    Wait-For { (Text $main $L.Counts) -ne '' -and (Text $main $Panel.Right.Counts) -ne '' }
    # 3. The empty box's text, before anything is typed.
    Find $main
    Wait-For { (Text $main $L.Message) -like 'Type a name*' }
    [Win]::Save((Join-Path $root '03-type-a-name.png'), $main)
    # 1. Results in this folder and its subfolders, beside a folder panel.
    Search-For $main 'Left' 'report'
    Wait-For { (Text $main $L.Counts) -like '6 results in*' }
    Wait-For { [Win]::Cell($process.Id, (Item $main $L.List), 5) -ne '' }
    [Win]::Save((Join-Path $root '01-search-panel-beside-folder-panel.png'), $main)
    # 4. No results, with the way to search every drive.
    Search-For $main 'Left' 'zzqx-nothing'
    Wait-For { (Text $main $L.Message) -like '*Choose All drives*' }
    [Win]::Save((Join-Path $root '04-no-results.png'), $main)
    # 5. All drives: the Folder column's full folders and the status line's
    # folder of the focused result.
    Search-For $main 'Left' 'report-draft'
    Look-In $main 'Left' 1
    Wait-For { (Text $main $L.Counts) -like '*on all indexed drives' -and (Text $main $L.Folder) -ne '' }
    [Win]::Save((Join-Path $root '05-all-drives.png'), $main)
}
finally { Stop-Window $process $main }

# 2. A folder on a drive that gozlerd does not index: the card reader D:,
# which has no card, so the panel opens on it as not ready.
$process, $main = Start-Window $Exe (Join-Path $root 'data-not-indexed') @{
    settings = $choices
    window   = @{
        mode = 'two_panels'; placement = $placement
        panels = @{ left = @{ folder = 'D:\' }; right = @{ folder = $fixture }; active = 'left' }
    }
}
try {
    Wait-For { (Text $main $Panel.Left.Message) -ne '' -and (Text $main $Panel.Right.Counts) -ne '' }
    Find $main
    Wait-For { (Text $main $Panel.Left.Message) -like '*is not indexed*' }
    [Win]::Save((Join-Path $root '02-not-indexed.png'), $main)
}
finally { Stop-Window $process $main }
