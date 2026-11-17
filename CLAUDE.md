# kustomize-rs

All project instructions live in AGENTS.md, shared by every coding agent:

@AGENTS.md

Claude-specific: `.claude/settings.json` denies git and GitHub write commands.
Don't work around a denial (no `sh -c "git ..."`, no aliases, no other tools).
Stop and tell the maintainer what's ready instead.
