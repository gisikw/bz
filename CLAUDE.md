## Issue Tracking (Beads)

This project uses `bd` (beads) for issue tracking. Run `bd prime` for full workflow context, or `bd onboard` if starting fresh.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --status in_progress  # Claim work
bd close <id>         # Complete work
bd sync               # Sync with git
```

### Session Completion

When wrapping up a work session, follow this workflow to avoid stranded work:

1. **File issues for remaining work** — Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) — Tests, linters, builds
3. **Update issue status** — Close finished work, update in-progress items
4. **Push to remote** — This ensures work isn't stranded locally:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # Should show "up to date with origin"
   ```
5. **Clean up** — Clear stashes, prune remote branches
6. **Hand off** — Provide context for the next session

The key thing: work isn't really "done" until it's pushed. Committing locally but not pushing means the next session (or another agent) can't see it.
