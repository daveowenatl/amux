# amux ZDOTDIR bootstrap for zsh.
#
# amux sets ZDOTDIR to this directory so that shell integration is loaded
# automatically. We restore the user's real ZDOTDIR immediately so that:
# - /etc/zshrc sets HISTFILE relative to the real ZDOTDIR/HOME (shared history)
# - zsh loads the user's real .zprofile/.zshrc normally (no wrapper recursion)

if [[ -n "${AMUX_ZSH_ZDOTDIR+X}" ]]; then
    builtin export ZDOTDIR="$AMUX_ZSH_ZDOTDIR"
    builtin unset AMUX_ZSH_ZDOTDIR
else
    builtin unset ZDOTDIR
fi

{
    # zsh treats unset ZDOTDIR as if it were HOME. We do the same.
    builtin typeset _amux_file="${ZDOTDIR-$HOME}/.zshenv"
    [[ ! -r "$_amux_file" ]] || builtin source -- "$_amux_file"
} always {
    if [[ -o interactive ]]; then
        # Load amux integration (unless disabled)
        if [[ "${AMUX_SHELL_INTEGRATION:-1}" != "0" && -n "${AMUX_SHELL_INTEGRATION_DIR:-}" ]]; then
            builtin typeset _amux_integ="$AMUX_SHELL_INTEGRATION_DIR/amux-zsh-integration.zsh"
            [[ -r "$_amux_integ" ]] && builtin source -- "$_amux_integ"
        fi
    fi

    builtin unset _amux_file _amux_integ
}
