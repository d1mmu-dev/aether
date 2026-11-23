# Sourced by the dev tools: loads KEY=VALUE lines from the PRIVATE tools config (never in the repository). Path:
# $AETHER_TOOLS_ENV, default ${XDG_CONFIG_HOME:-~/.config}/aether/tools.env. Values are taken literally (no shell
# evaluation). Only these six keys are ever read; any other line (LD_PRELOAD, BASH_ENV, NODE_OPTIONS, GIT_DIR,
# npm_config_*, ...) is ignored. A key already present in the environment, even set to an empty value, wins over the
# config. tools/tools-env.mjs applies the same rules for the Node tools.
#   AETHER_UPSTREAM_REFERENCE AETHER_REFERENCE_COPY AETHER_CSS_DEPS AETHER_TOOLS_PATH VERTIGO FFMPEG
AETHER_TOOLS_ENV="${AETHER_TOOLS_ENV:-${XDG_CONFIG_HOME:-$HOME/.config}/aether/tools.env}"
if [ -f "$AETHER_TOOLS_ENV" ]; then
  while IFS= read -r _line || [ -n "$_line" ]; do
    _key="${_line%%=*}"
    case "$_line" in *=*) ;; *) continue ;; esac
    case "$_key" in
      AETHER_UPSTREAM_REFERENCE | AETHER_REFERENCE_COPY | AETHER_CSS_DEPS | AETHER_TOOLS_PATH | VERTIGO | FFMPEG) ;;
      *) continue ;;
    esac
    [ -n "${!_key+set}" ] || export "$_key=${_line#*=}"
  done <"$AETHER_TOOLS_ENV"
  unset _line _key
fi
# AETHER_TOOLS_PATH: extra directories (node, npm, ffmpeg, ...) put in front of PATH.
if [ -n "${AETHER_TOOLS_PATH:-}" ]; then export PATH="$AETHER_TOOLS_PATH:$PATH"; fi
