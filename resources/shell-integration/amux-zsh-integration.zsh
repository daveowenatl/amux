# amux shell integration for zsh
# Source this from .zshrc: [[ -n "$AMUX_SOCKET_PATH" ]] && source ~/.config/amux/shell/amux-zsh-integration.zsh

# Guard: only activate inside amux
[[ -n "$AMUX_SOCKET_PATH" ]] || return 0

# ---------------------------------------------------------------------------
# Throttle state
# ---------------------------------------------------------------------------
typeset -g _AMUX_PWD_LAST=""
typeset -g _AMUX_GIT_LAST_PWD=""
typeset -g _AMUX_GIT_LAST_RUN=0
typeset -g _AMUX_GIT_FORCE=0
typeset -g _AMUX_GIT_HEAD_LAST_PWD=""
typeset -g _AMUX_GIT_HEAD_PATH=""
typeset -g _AMUX_GIT_HEAD_SIGNATURE=""
typeset -g _AMUX_GIT_HEAD_WATCH_PID=""
typeset -g _AMUX_PR_POLL_PID=""
typeset -g _AMUX_PR_POLL_PWD=""
typeset -g _AMUX_PR_POLL_INTERVAL=45
typeset -g _AMUX_PR_FORCE=0

# ---------------------------------------------------------------------------
# Git HEAD resolution (no git subprocess — fast)
# ---------------------------------------------------------------------------
_amux_git_resolve_head_path() {
    local dir="$PWD"
    while true; do
        if [[ -d "$dir/.git" ]]; then
            print -r -- "$dir/.git/HEAD"
            return 0
        fi
        if [[ -f "$dir/.git" ]]; then
            local line gitdir
            line="$(<"$dir/.git")"
            if [[ "$line" == gitdir:* ]]; then
                gitdir="${line#gitdir:}"
                gitdir="${gitdir## }"
                gitdir="${gitdir%% }"
                [[ -n "$gitdir" ]] || return 1
                [[ "$gitdir" != /* ]] && gitdir="$dir/$gitdir"
                print -r -- "$gitdir/HEAD"
                return 0
            fi
        fi
        [[ "$dir" == "/" || -z "$dir" ]] && break
        dir="${dir:h}"
    done
    return 1
}

_amux_git_head_signature() {
    local head_path="$1"
    [[ -n "$head_path" && -r "$head_path" ]] || return 1
    local line=""
    if IFS= read -r line < "$head_path"; then
        print -r -- "$line"
        return 0
    fi
    return 1
}

# ---------------------------------------------------------------------------
# Git branch + dirty detection
# ---------------------------------------------------------------------------
_amux_report_git() {
    local repo_path="$1"
    [[ -n "$repo_path" ]] || return 0

    # Not in a git repo? Clear.
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
        builtin cd "$repo_path" 2>/dev/null \
            && ${=timeout_cmd} gh pr view "$branch" \
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

    local state_lower="${state:l}"
    amux set-pr --number "$number" --title "$title" --state "$state_lower" >/dev/null 2>&1
}

_amux_stop_pr_poll() {
    if [[ -n "$_AMUX_PR_POLL_PID" ]]; then
        kill "$_AMUX_PR_POLL_PID" >/dev/null 2>&1 || true
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
    {
        while true; do
            kill -0 "$shell_pid" >/dev/null 2>&1 || break
            _amux_report_pr "$watch_pwd" || true
            sleep "$interval"
        done
    } >/dev/null 2>&1 &!
    _AMUX_PR_POLL_PID=$!
}

# ---------------------------------------------------------------------------
# HEAD file watcher (detects branch changes during long commands)
# ---------------------------------------------------------------------------
_amux_stop_head_watch() {
    if [[ -n "$_AMUX_GIT_HEAD_WATCH_PID" ]]; then
        kill "$_AMUX_GIT_HEAD_WATCH_PID" >/dev/null 2>&1 || true
        _AMUX_GIT_HEAD_WATCH_PID=""
    fi
}

_amux_start_head_watch() {
    local watch_pwd="$PWD"
    local watch_head_path
    watch_head_path="$(_amux_git_resolve_head_path 2>/dev/null || true)"
    [[ -n "$watch_head_path" ]] || return 0

    local watch_head_sig
    watch_head_sig="$(_amux_git_head_signature "$watch_head_path" 2>/dev/null || true)"

    _AMUX_GIT_HEAD_LAST_PWD="$watch_pwd"
    _AMUX_GIT_HEAD_PATH="$watch_head_path"
    _AMUX_GIT_HEAD_SIGNATURE="$watch_head_sig"

    _amux_stop_head_watch
    {
        local last_sig="$watch_head_sig"
        while true; do
            sleep 1
            local sig
            sig="$(_amux_git_head_signature "$watch_head_path" 2>/dev/null || true)"
            if [[ -n "$sig" && "$sig" != "$last_sig" ]]; then
                last_sig="$sig"
                _amux_report_git "$watch_pwd"
            fi
        done
    } >/dev/null 2>&1 &!
    _AMUX_GIT_HEAD_WATCH_PID=$!
}

# ---------------------------------------------------------------------------
# Hook: preexec (before command runs)
# ---------------------------------------------------------------------------
_amux_preexec() {
    # Force git refresh after git-related commands
    local cmd="${1## }"
    case "$cmd" in
        git\ *|git|gh\ *|lazygit|lazygit\ *|tig|tig\ *|gitui|gitui\ *|stg\ *|jj\ *)
            _AMUX_GIT_FORCE=1
            _AMUX_PR_FORCE=1 ;;
    esac

    _amux_stop_pr_poll
    _amux_start_head_watch
}

# ---------------------------------------------------------------------------
# Hook: precmd (before prompt)
# ---------------------------------------------------------------------------
_amux_precmd() {
    _amux_stop_head_watch

    local now=$EPOCHSECONDS
    local pwd="$PWD"

    # CWD: report on change
    if [[ "$pwd" != "$_AMUX_PWD_LAST" ]]; then
        _AMUX_PWD_LAST="$pwd"
        { amux set-cwd "$pwd" >/dev/null 2>&1 } &!
    fi

    # Git branch: refresh on directory change, force flag, or every ~3s
    local should_git=0
    local git_head_changed=0

    # Detect HEAD changes (branch switch via alias, tool, etc.)
    if [[ "$pwd" != "$_AMUX_GIT_HEAD_LAST_PWD" ]]; then
        _AMUX_GIT_HEAD_LAST_PWD="$pwd"
        _AMUX_GIT_HEAD_PATH="$(_amux_git_resolve_head_path 2>/dev/null || true)"
        _AMUX_GIT_HEAD_SIGNATURE=""
    fi
    if [[ -n "$_AMUX_GIT_HEAD_PATH" ]]; then
        local head_sig
        head_sig="$(_amux_git_head_signature "$_AMUX_GIT_HEAD_PATH" 2>/dev/null || true)"
        if [[ -n "$head_sig" ]]; then
            if [[ -z "$_AMUX_GIT_HEAD_SIGNATURE" ]]; then
                _AMUX_GIT_HEAD_SIGNATURE="$head_sig"
            elif [[ "$head_sig" != "$_AMUX_GIT_HEAD_SIGNATURE" ]]; then
                _AMUX_GIT_HEAD_SIGNATURE="$head_sig"
                git_head_changed=1
                _AMUX_GIT_FORCE=1
                _AMUX_PR_FORCE=1
                should_git=1
            fi
        fi
    fi

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
        { _amux_report_git "$pwd" } >/dev/null 2>&1 &!
    fi

    # PR polling: restart on directory/branch change
    local should_restart_pr=0
    local pr_context_changed=0
    if [[ -n "$_AMUX_PR_POLL_PWD" && "$pwd" != "$_AMUX_PR_POLL_PWD" ]]; then
        pr_context_changed=1
    elif (( git_head_changed )); then
        pr_context_changed=1
    fi
    if [[ "$pwd" != "$_AMUX_PR_POLL_PWD" ]]; then
        should_restart_pr=1
    elif (( _AMUX_PR_FORCE )); then
        should_restart_pr=1
    elif [[ -z "$_AMUX_PR_POLL_PID" ]] || ! kill -0 "$_AMUX_PR_POLL_PID" 2>/dev/null; then
        should_restart_pr=1
    fi

    if (( should_restart_pr )); then
        _AMUX_PR_FORCE=0
        if (( pr_context_changed )); then
            { amux set-pr --clear >/dev/null 2>&1 } &!
        fi
        _amux_start_pr_poll "$pwd" 1
    fi
}

# ---------------------------------------------------------------------------
# Cleanup on shell exit
# ---------------------------------------------------------------------------
_amux_zshexit() {
    _amux_stop_head_watch
    _amux_stop_pr_poll
}

# ---------------------------------------------------------------------------
# Register hooks
# ---------------------------------------------------------------------------
autoload -Uz add-zsh-hook
add-zsh-hook preexec _amux_preexec
add-zsh-hook precmd _amux_precmd
add-zsh-hook zshexit _amux_zshexit
