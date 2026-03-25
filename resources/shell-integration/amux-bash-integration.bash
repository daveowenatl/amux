# amux shell integration for bash
# Source this from .bashrc: [[ -n "$AMUX_SOCKET_PATH" ]] && source ~/.config/amux/shell/amux-bash-integration.bash

# Guard: only activate inside amux
[[ -n "$AMUX_SOCKET_PATH" ]] || return 0

# ---------------------------------------------------------------------------
# Throttle state
# ---------------------------------------------------------------------------
_AMUX_PWD_LAST=""
_AMUX_GIT_LAST_PWD=""
_AMUX_GIT_LAST_RUN=0
_AMUX_GIT_FORCE=0
_AMUX_PR_POLL_PID=""
_AMUX_PR_POLL_PWD=""
_AMUX_PR_POLL_INTERVAL=45
_AMUX_PR_FORCE=0

# ---------------------------------------------------------------------------
# Git branch + dirty detection
# ---------------------------------------------------------------------------
_amux_report_git() {
    local repo_path="$1"
    [[ -n "$repo_path" ]] || return 0

    git -C "$repo_path" rev-parse --git-dir >/dev/null 2>&1 || {
        amux set-git --clear >/dev/null 2>&1
        return 0
    }

    local branch dirty_flag=""
    branch="$(git -C "$repo_path" branch --show-current 2>/dev/null)"
    if [[ -n "$branch" ]]; then
        local first
        first="$(git -C "$repo_path" status --porcelain -uno 2>/dev/null | head -1)"
        [[ -n "$first" ]] && dirty_flag="--dirty"
        amux set-git --branch "$branch" $dirty_flag >/dev/null 2>&1
    else
        amux set-git --clear >/dev/null 2>&1
    fi
}

# ---------------------------------------------------------------------------
# PR polling
# ---------------------------------------------------------------------------
_amux_report_pr() {
    local repo_path="$1"
    [[ -n "$repo_path" ]] || return 0
    [[ -d "$repo_path" ]] || return 0
    command -v gh >/dev/null 2>&1 || {
        amux set-pr --clear >/dev/null 2>&1
        return 0
    }

    local branch
    branch="$(git -C "$repo_path" branch --show-current 2>/dev/null)"
    [[ -n "$branch" ]] || {
        amux set-pr --clear >/dev/null 2>&1
        return 0
    }

    # Use timeout/gtimeout if available, otherwise run gh directly
    local timeout_cmd=""
    if command -v timeout >/dev/null 2>&1; then
        timeout_cmd="timeout 10"
    elif command -v gtimeout >/dev/null 2>&1; then
        timeout_cmd="gtimeout 10"
    fi

    local gh_output=""
    gh_output="$(
        cd "$repo_path" 2>/dev/null \
            && $timeout_cmd gh pr view "$branch" \
                --json number,title,state \
                --jq '[.number, .title, .state] | @tsv' \
                2>/dev/null
    )" || true

    if [[ -z "$gh_output" ]]; then
        amux set-pr --clear >/dev/null 2>&1
        return 0
    fi

    local IFS=$'\t'
    local number title state
    read -r number title state <<< "$gh_output"
    [[ -n "$number" ]] || return 0

    local state_lower="${state,,}"
    amux set-pr --number "$number" --title "$title" --state "$state_lower" >/dev/null 2>&1
}

_amux_stop_pr_poll() {
    if [[ -n "$_AMUX_PR_POLL_PID" ]]; then
        kill "$_AMUX_PR_POLL_PID" >/dev/null 2>&1 || true
        wait "$_AMUX_PR_POLL_PID" >/dev/null 2>&1 || true
        _AMUX_PR_POLL_PID=""
    fi
}

_amux_start_pr_poll() {
    local watch_pwd="${1:-$PWD}"
    local force="${2:-0}"

    if [[ "$force" != "1" && "$watch_pwd" == "$_AMUX_PR_POLL_PWD" && -n "$_AMUX_PR_POLL_PID" ]] \
        && kill -0 "$_AMUX_PR_POLL_PID" 2>/dev/null; then
        return 0
    fi

    _amux_stop_pr_poll
    _AMUX_PR_POLL_PWD="$watch_pwd"

    local interval="${_AMUX_PR_POLL_INTERVAL:-45}"
    local shell_pid="$$"
    (
        while true; do
            kill -0 "$shell_pid" >/dev/null 2>&1 || break
            _amux_report_pr "$watch_pwd" || true
            sleep "$interval"
        done
    ) &
    _AMUX_PR_POLL_PID=$!
    disown "$_AMUX_PR_POLL_PID" 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# PROMPT_COMMAND hook
# ---------------------------------------------------------------------------
_amux_prompt_command() {
    local now
    now="$(date +%s)"
    local pwd="$PWD"

    # CWD: report on change
    if [[ "$pwd" != "$_AMUX_PWD_LAST" ]]; then
        _AMUX_PWD_LAST="$pwd"
        amux set-cwd "$pwd" >/dev/null 2>&1 &
        disown $! 2>/dev/null || true
    fi

    # Git branch: refresh on directory change, force flag, or every ~3s
    local should_git=0
    if [[ "$pwd" != "$_AMUX_GIT_LAST_PWD" ]]; then
        should_git=1
    elif (( _AMUX_GIT_FORCE )); then
        should_git=1
    elif (( now - _AMUX_GIT_LAST_RUN >= 3 )); then
        should_git=1
    fi

    if (( should_git )); then
        _AMUX_GIT_FORCE=0
        _AMUX_GIT_LAST_PWD="$pwd"
        _AMUX_GIT_LAST_RUN=$now
        _amux_report_git "$pwd" &
        disown $! 2>/dev/null || true
    fi

    # PR polling
    local should_restart_pr=0
    if [[ "$pwd" != "$_AMUX_PR_POLL_PWD" ]]; then
        should_restart_pr=1
    elif (( _AMUX_PR_FORCE )); then
        should_restart_pr=1
    elif [[ -z "$_AMUX_PR_POLL_PID" ]] || ! kill -0 "$_AMUX_PR_POLL_PID" 2>/dev/null; then
        should_restart_pr=1
    fi

    if (( should_restart_pr )); then
        _AMUX_PR_FORCE=0
        if [[ -n "$_AMUX_PR_POLL_PWD" && "$pwd" != "$_AMUX_PR_POLL_PWD" ]]; then
            amux set-pr --clear >/dev/null 2>&1 &
            disown $! 2>/dev/null || true
        fi
        _amux_start_pr_poll "$pwd" 1
    fi
}

# ---------------------------------------------------------------------------
# DEBUG trap for preexec equivalent
# ---------------------------------------------------------------------------
_amux_preexec_trap() {
    # Only trigger for interactive commands, not PROMPT_COMMAND itself
    [[ "$BASH_COMMAND" == "_amux_prompt_command" ]] && return
    [[ "$BASH_COMMAND" == "$PROMPT_COMMAND" ]] && return

    local cmd="$BASH_COMMAND"
    case "$cmd" in
        git\ *|git|gh\ *|lazygit|lazygit\ *|tig|tig\ *|gitui|gitui\ *|stg\ *|jj\ *)
            _AMUX_GIT_FORCE=1
            _AMUX_PR_FORCE=1 ;;
    esac
}

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------
_amux_cleanup() {
    _amux_stop_pr_poll
}

# ---------------------------------------------------------------------------
# Register hooks
# ---------------------------------------------------------------------------
if [[ -z "$PROMPT_COMMAND" ]]; then
    PROMPT_COMMAND="_amux_prompt_command"
elif [[ "$PROMPT_COMMAND" != *"_amux_prompt_command"* ]]; then
    PROMPT_COMMAND="_amux_prompt_command;$PROMPT_COMMAND"
fi

trap '_amux_preexec_trap' DEBUG
trap '_amux_cleanup' EXIT
