Read @CONSTITUTION.md Please

When managing subagebts, make sure their context windows do not exceed 70%. If they do, refresh with a new subagent.

@CODEBASE.md gives a summary of the file structure if you ever need it.

Working on src/adapters/? Read src/adapters/exec/README.md first — it is the adapter execution contract.

Agent sandbox trap: under `--features ui` the EMPTY lib/bin unittest binaries wedge in dyld before
main and never exit — it reads as a slow test run, it is a hang. They hold no tests, so run
`cargo test --features ui --test fitness` instead. Headless is unaffected.
