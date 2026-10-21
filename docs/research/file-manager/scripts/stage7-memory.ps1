# One run that measures what a search panel costs in private bytes, against
# the budget of stage7-design.md section 3. It writes memory.txt beside it.
param([string] $Exe = 'C:\SynoSoftware\TinyTorrent2\3rdParty\goz\target\release\gozler-ui.exe')
. (Join-Path $PSScriptRoot 'common.ps1')
$root = $PSScriptRoot
$lines = [Collections.Generic.List[string]]::new()
function Note([string] $line) { $lines.Add($line); $line }

function Private-Bytes($process) { $process.Refresh(); $process.PrivateMemorySize64 }

# The number in a status line such as "1,234,567 results on all indexed
# drives".
function Total([string] $text) { [int64](($text -split ' ')[0] -replace '\D', '') }

# Makes the active panel `$side` a search panel of all drives for `*`, and
# fills its cache: the first chunk, then row 20,000's, which is the second.
function Fill($process, $main, $side) {
    $p = $Panel[$side]
    Find $main
    Wait-For { Is-Searching $main $side }
    Look-In $main $side 1
    Wait-For { (Text $main $p.Message) -ne '' }
    $null = Note ("  {0} searching, box empty: {1:N2} MB" -f $side, ((Private-Bytes $process) / 1MB))
    Search-For $main $side '*'
    Wait-For { (Text $main $p.Counts) -like '*on all indexed drives' }
    $total = Total (Text $main $p.Counts)
    if ($total -le 100000) { throw "$side found $total results, not more than 100,000" }
    $list = Item $main $p.List
    Wait-For { [Win]::Cell($process.Id, $list, 0) -ne '' }
    $null = Note ("  {0} first chunk: {1:N2} MB" -f $side, ((Private-Bytes $process) / 1MB))
    [void][Win]::SendMessage($list, $LVM_ENSUREVISIBLE, [IntPtr]20000, [IntPtr]0)
    Wait-For { [Win]::Cell($process.Id, $list, 20000) -ne '' }
    return $total
}

# Leaves `$side`'s search: Esc in its search box shows its folder again.
function Leave($main, $side) {
    Key (Item $main $Panel[$side].Search) 0x1B
    Wait-For { -not (Is-Searching $main $side) -and (Text $main $Panel[$side].Counts) -ne '' }
}

$placement = @{ left = 60; top = 60; right = 1180; bottom = 700; maximized = $false }
$process, $main = Start-Window $Exe (Join-Path $root 'data-memory') @{
    settings = @{ keep_running = $false; check_for_updates = $false }
    window   = @{
        mode = 'two_panels'; placement = $placement
        panels = @{
            left = @{ folder = 'C:\SynoSoftware\goz-file-manager\crates' }
            right = @{ folder = 'C:\SynoSoftware\goz-file-manager\docs' }
            active = 'left'
        }
    }
}
try {
    Note "exe: $Exe"
    # B0: both panels listed and show their free space; Search's hidden
    # list has the first rows of its empty search, which it asks for at start.
    Wait-For {
        (Text $main $Panel.Left.Free) -ne '' -and (Text $main $Panel.Right.Free) -ne '' -and
        (Count (Item $main $IDC_LIST)) -gt 0
    }
    $b0 = Private-Bytes $process
    $total = Fill $process $main 'Left'
    $b1 = Private-Bytes $process
    Note "results of '*' on all drives: $total"
    # Tab in the left search box makes the right panel active.
    Key (Item $main $Panel.Left.Search) 0x09
    [void](Fill $process $main 'Right')
    $b2 = Private-Bytes $process
    foreach ($side in 'Left', 'Right') { Leave $main $side }
    # The right panel is active; Tab from its list makes the left active.
    [void](Fill $process $main 'Right')
    Key (Item $main $Panel.Right.Search) 0x09
    [void](Fill $process $main 'Left')
    $b3 = Private-Bytes $process
    # B4: Search, its empty search scrolled to its last row.
    [void][Win]::PostMessage($main, $WM_COMMAND, [IntPtr]$TWO_PANELS, [IntPtr]0)
    $list = Item $main $IDC_LIST
    Wait-For { [Win]::IsWindowVisible($list) -and (Count $list) -gt 0 }
    $last = (Count $list) - 1
    [void][Win]::SendMessage($list, $LVM_ENSUREVISIBLE, [IntPtr]$last, [IntPtr]0)
    Wait-For { [Win]::Cell($process.Id, $list, $last) -ne '' }
    $b4 = Private-Bytes $process
    $mb = { param($bytes) '{0:N2} MB' -f ($bytes / 1MB) }
    Note ("B0 {0}" -f (& $mb $b0))
    Note ("B1 {0}; B1 - B0 = {1} (budget 7.5 MB)" -f (& $mb $b1), (& $mb ($b1 - $b0)))
    Note ("B2 {0}; B2 - B0 = {1} (budget 15 MB)" -f (& $mb $b2), (& $mb ($b2 - $b0)))
    Note ("B3 {0}; B3 - B2 = {1} (budget under 1 MB)" -f (& $mb $b3), (& $mb ($b3 - $b2)))
    Note ("B4 {0} (DESIGN.md's Search budget: 16 MB)" -f (& $mb $b4))
}
finally {
    Stop-Window $process $main
    $lines | Set-Content (Join-Path $root 'memory.txt') -Encoding utf8NoBOM
}
