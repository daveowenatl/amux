# amux shell integration for PowerShell 7+
#
# This script is auto-sourced by a bootstrap command amux passes to `pwsh`
# via `-Command` when spawning a shell inside an amux pane. The bootstrap
# loads the user's $PROFILE first (so any prompt override they define is
# in place), then dot-sources this file so we can wrap that prompt.
#
# Features:
# - One-shot scrollback restore from $env:AMUX_RESTORE_SCROLLBACK_FILE
# - OSC 133 semantic prompt marks (D = command finished, A = prompt start)
# - CWD reporting to the sidebar on directory change
# - Git branch + dirty status reporting, throttled to ~3s
# - Exit cleanup (clear git state when the shell exits)
#
# Features intentionally deferred to follow-ups (see issue #166):
# - PR polling (would require Start-Job; adds runspace overhead)
# - Preexec hooks (force git refresh after git commands) — handled partially
#   by the 3s throttle until PSReadLine-based preexec is viable
# - OSC 133 B/C marks (input done / output start) — used by some terminals
#   for command-boundary features, not required for amux's save-side prompt
#   row trimming

# Guard: only activate inside an amux pane.
if (-not $env:AMUX_SOCKET_PATH) { return }

# Resolve the amux CLI binary. AMUX_BIN is set by amux-app; fall back to
# whatever pwsh resolves via PATH. We cache this once at source time so
# every prompt call doesn't re-walk PATH.
$script:AmuxBin = $null
if ($env:AMUX_BIN -and (Test-Path -LiteralPath $env:AMUX_BIN)) {
    $script:AmuxBin = $env:AMUX_BIN
} else {
    $cmd = Get-Command amux -ErrorAction SilentlyContinue
    if ($cmd) { $script:AmuxBin = $cmd.Source }
}

# ---------------------------------------------------------------------------
# Session scrollback restore (one-shot, runs before first prompt)
# ---------------------------------------------------------------------------
if ($env:AMUX_RESTORE_SCROLLBACK_FILE) {
    $scrollbackFile = $env:AMUX_RESTORE_SCROLLBACK_FILE
    # Clear the env var immediately so a nested shell can't double-restore.
    Remove-Item Env:AMUX_RESTORE_SCROLLBACK_FILE -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $scrollbackFile) {
        try {
            # Use Get-Content -Raw + [Console]::Write to emit the saved
            # scrollback verbatim, with no added encoding or formatting —
            # analogous to the `/bin/cat` call in the bash/zsh integration.
            $content = Get-Content -Raw -LiteralPath $scrollbackFile -ErrorAction Stop
            [Console]::Write($content)
        } catch {
            # Restore is best-effort; a failure must not block the shell.
        }
        Remove-Item -LiteralPath $scrollbackFile -Force -ErrorAction SilentlyContinue
    }
}

# ---------------------------------------------------------------------------
# Throttle state
# ---------------------------------------------------------------------------
$script:AmuxPwdLast = $null
$script:AmuxGitLastPwd = $null
$script:AmuxGitLastRun = [datetime]::MinValue

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Invoke the amux CLI swallowing stdout, stderr, and any terminating errors.
# Shell integration must never fail visibly to the user.
function script:Invoke-AmuxQuiet {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]] $Remaining)
    if (-not $script:AmuxBin) { return }
    try {
        & $script:AmuxBin @Remaining *>&1 | Out-Null
    } catch {
        # Intentionally swallow.
    }
}

# Return @{ Branch = <string or $null>; Dirty = <bool> } for the given repo
# path. Runs two git subprocesses; uses `branch --show-current` for branch
# and `status --porcelain -uno` + a first-match check for dirty.
function script:Get-AmuxGitState {
    param([string]$RepoPath)
    $result = @{ Branch = $null; Dirty = $false }
    try {
        $branch = (& git -C $RepoPath branch --show-current 2>$null)
        if ($null -ne $branch) { $branch = $branch.Trim() }
        if (-not [string]::IsNullOrEmpty($branch)) {
            $result.Branch = $branch
            $first = & git -C $RepoPath status --porcelain -uno 2>$null | Select-Object -First 1
            $result.Dirty = -not [string]::IsNullOrEmpty($first)
        }
    } catch {
        # git not installed or not a repo — treat as "no branch".
    }
    return $result
}

# Report current git state to the sidebar.
function script:Update-AmuxGit {
    param([string]$RepoPath)
    $state = Get-AmuxGitState -RepoPath $RepoPath
    if ($state.Branch) {
        if ($state.Dirty) {
            Invoke-AmuxQuiet set-git --branch $state.Branch --dirty
        } else {
            Invoke-AmuxQuiet set-git --branch $state.Branch
        }
    } else {
        Invoke-AmuxQuiet set-git --clear
    }
}

# ---------------------------------------------------------------------------
# Prompt override
# ---------------------------------------------------------------------------

# Save whatever prompt function is currently defined (the built-in default
# or the user's $PROFILE override, since the bootstrap loads $PROFILE first)
# so our override can chain to it.
$script:AmuxOriginalPrompt = $function:prompt

function global:prompt {
    # Preserve $LASTEXITCODE — our helper calls would otherwise clobber it
    # before the original prompt function gets to read it.
    $lastExit = $global:LASTEXITCODE
    if ($null -eq $lastExit) { $lastExit = 0 }

    # OSC 133;D — command finished (with exit code). `e is ESC, `a is BEL.
    # Matching the bash/zsh format used in amux-bash-integration.bash.
    [Console]::Write("`e]133;D;$lastExit`a")
    # OSC 133;A — prompt starts.
    [Console]::Write("`e]133;A`a")

    $pwdStr = $PWD.Path

    # CWD: report on change.
    if ($pwdStr -ne $script:AmuxPwdLast) {
        $script:AmuxPwdLast = $pwdStr
        Invoke-AmuxQuiet set-cwd $pwdStr
    }

    # Git branch: refresh on directory change or every ~3 seconds.
    $now = Get-Date
    $shouldGit = $false
    if ($pwdStr -ne $script:AmuxGitLastPwd) {
        $shouldGit = $true
    } elseif (($now - $script:AmuxGitLastRun).TotalSeconds -ge 3) {
        $shouldGit = $true
    }
    if ($shouldGit) {
        $script:AmuxGitLastPwd = $pwdStr
        $script:AmuxGitLastRun = $now
        Update-AmuxGit -RepoPath $pwdStr
    }

    # Restore $LASTEXITCODE and chain to whatever prompt function existed
    # before we overrode. The chained prompt is responsible for producing
    # the actual prompt string the user sees.
    $global:LASTEXITCODE = $lastExit
    & $script:AmuxOriginalPrompt
}

# ---------------------------------------------------------------------------
# Cleanup on shell exit
# ---------------------------------------------------------------------------
#
# Pass the resolved amux binary path via -MessageData so the action block
# can read it from $Event.MessageData. Using $script:AmuxBin directly inside
# the action block is unreliable: the block is stored and executed later
# when the engine exit event fires, and the `script:` scope prefix inside
# a Register-EngineEvent -Action scriptblock is not guaranteed to resolve
# to this script's top-level scope. Passing via -MessageData makes the
# capture explicit and independent of scope semantics.
Register-EngineEvent -SourceIdentifier PowerShell.Exiting `
    -SupportEvent `
    -MessageData $script:AmuxBin `
    -Action {
        $amuxBin = $Event.MessageData
        if ($amuxBin) {
            try { & $amuxBin set-git --clear *>&1 | Out-Null } catch {}
        }
    } | Out-Null
