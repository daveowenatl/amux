# amux compatibility shim: restores ZDOTDIR and sources user's .zlogin.

if [[ -n "${AMUX_ZSH_ZDOTDIR+X}" ]]; then
    builtin export ZDOTDIR="$AMUX_ZSH_ZDOTDIR"
    builtin unset AMUX_ZSH_ZDOTDIR
else
    builtin unset ZDOTDIR
fi

builtin typeset _amux_file="${ZDOTDIR-$HOME}/.zlogin"
[[ ! -r "$_amux_file" ]] || builtin source -- "$_amux_file"
builtin unset _amux_file
