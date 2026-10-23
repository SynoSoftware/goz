#Requires -Version 7
# Runs the Windows half of CI's test job, cheapest check first. The full run
# goes on after a failing step, so one run reports every error; it then exits
# with the first failure's code. Cargo runs at below-normal priority, which every
# rustc and linker process inherits, so a build does not slow interactive work
# on the machine. Warnings are denied through clippy and RUSTDOCFLAGS, not
# RUSTFLAGS, because a RUSTFLAGS change rebuilds every dependency and drops the
# static CRT link set in .cargo/config.toml.
# Every mode builds with --workspace, because a narrower -p resolves dependency
# features differently and rebuilds much of the dependency graph.
param(
    # Runs only the tests whose names contain this filter, to recheck a fix.
    [string] $Test,
    # Runs only the criterion benchmarks whose names match this filter, such as
    # query_2M. --bench '*' selects the bench targets alone; plain cargo bench
    # would also build every library as a bench target.
    [string] $Bench,
    # Runs an ignored synthetic timing test in an optimized build.
    [string] $Timing,
    # Builds the release executables, which the probe measures, because the
    # memory budget is measured on what ships.
    [switch] $Release,
    # With -Release, adds the window's pseudo-language qps-ploc, which the
    # probe's screenshots use to show where a longer translation clips.
    [switch] $Pseudo
)

$ErrorActionPreference = 'Stop'

function Invoke-Cargo {
    Write-Host "cargo $args"
    & cargo @args
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$failures = [System.Collections.Generic.List[string]]::new()
$exitCode = 0

# Runs one step of the full run and records a failure instead of stopping. A
# compile error then shows again in each later step; that costs seconds, while
# stopping hid every test result behind one formatting or lint error.
function Invoke-CargoStep {
    Write-Host "cargo $args"
    & cargo @args
    if ($LASTEXITCODE -ne 0) {
        $script:failures.Add("cargo $args")
        if ($script:exitCode -eq 0) { $script:exitCode = $LASTEXITCODE }
    }
}

$process = Get-Process -Id $PID
$priority = $process.PriorityClass
$rustdocFlags = $env:RUSTDOCFLAGS
Push-Location $PSScriptRoot
try {
    $process.PriorityClass = 'BelowNormal'
    if ($Timing) {
        Invoke-Cargo test --release --workspace --no-run $Timing
        $process.PriorityClass = $priority
        Invoke-Cargo test --release --workspace --no-fail-fast $Timing '--' --ignored --nocapture --test-threads=1
    }
    elseif ($Test) {
        Invoke-Cargo test --workspace --no-fail-fast $Test
    }
    elseif ($Release) {
        $features = if ($Pseudo) { @('--features', 'gozler-ui/pseudo-language') } else { @() }
        Invoke-Cargo build --release --workspace @features
    }
    elseif ($Bench) {
        Invoke-Cargo bench --workspace --bench '*' --no-run
        # Measured at the caller's priority: below normal, any other load on
        # the machine would take the CPU first and decide the timings.
        $process.PriorityClass = $priority
        Invoke-Cargo bench --workspace --bench '*' $Bench
    }
    else {
        Invoke-CargoStep fmt --all --check
        # A bare -- never reaches a PowerShell function, so it is quoted.
        Invoke-CargoStep clippy --workspace --all-targets '--' -D warnings
        # --no-fail-fast runs every test binary, so one run reports every
        # failing test instead of only the first failing binary's.
        Invoke-CargoStep test --workspace --no-fail-fast
        # Set only for the doc step, as CI does, so doctests run as in CI.
        $env:RUSTDOCFLAGS = '-D warnings'
        Invoke-CargoStep doc --workspace --no-deps --document-private-items
        if ($failures.Count -gt 0) {
            Write-Host "Failed steps:"
            $failures | ForEach-Object { Write-Host "  $_" }
            exit $exitCode
        }
    }
}
finally {
    # Restored because a caller in the same session keeps this process.
    $process.PriorityClass = $priority
    $env:RUSTDOCFLAGS = $rustdocFlags
    Pop-Location
}
