# Project Guide

## Structure
- `./`: main branch working tree
- `.worktrees/<project>-<branch>/`: branch worktrees
- `.shared/`: shared deps and artifacts
- `.shared/dist/`: build outputs
- `.shared/release/`: release bundles
- `.shared/data/`: datasets/models and other shared data
- `.shared/requirements/`: private requirement and context docs
- `.agents/`: assistant config (tracked)
  - `commands/`, `skills/`, `notes/`

## Conventions
- Store datasets and generated data under `.shared/data`
- Route build outputs to `.shared/dist` or `.shared/release`
