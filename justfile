# CtrlAssist project tasks
#
# Run `just` (or `just --list`) to see all recipes, grouped by purpose.
# This justfile is the canonical source of truth for project commands:
# prek hooks, editor tasks and CI workflows all dispatch through here.
# A recipe is added in the commit that first gives it something to check.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Default recipe: list all available recipes, grouped.
default:
    @just --list --unsorted

# --- Quality checks (prek hooks) ----------------------------------------

# typos doesn't search `.config/` yet, so the config path is explicit.
#
# Spell-check code, docs and identifiers, enforcing American English
[group('check')]
typos:
    typos --config .config/typos.toml

# Run every local gate
[group('check')]
check: typos
